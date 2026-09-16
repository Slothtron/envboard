#!/usr/bin/env python3
"""envboard 注入器 —— 一个文件、纯标准库（外加 mitmproxy 本身），职责只有四件事。

1. **读 `<自身目录>/config.json`**（管理器写下、热重载的配置通道）；
2. **读 `<自身目录>/envboard.rules`**（固定名软链，指向当前绑定的规则库文件）；
3. **在 `server_connect` 里改写上连目标**（只改"往哪连"，不改请求内容、Host、SNI 基准）；
4. **按域名放宽上游证书校验**：名单内的域名在 `tls_start_server` 里用 `VERIFY_NONE`
   自建上游 context，名单外的一律走 mitmproxy 的严格校验；
   外加**回写状态文件**，让管理器能判断"配置到底有没有生效"。

它**不参与环境管理**：不知道有哪些兄弟实例，也不碰任何状态目录（除了自己的状态文件）。
这样才能"随时可以丢掉"。

## 配置与热重载

`config.json` 与规则软链都在注入器**旁边**，所以"配置放在哪"不需要参数：

* 配置按**内容哈希**判变化，规则按链 + 目标的 `(mtime_ns, size)` 判变化；
* 变了就整份重建一个**不可变快照**（`Policy`），hooks 单次取用 —— 单条连接内配置一致；
* 热重载失败**保留旧快照**并写 `config_error`（不中断代理）；启动首读失败则抛错退出
  （不留半配置实例）。

## 两种用法

* **作为 addon**：`mitmdump -s envboard_mitmproxy.py --set confdir=… --set listen_host=…`
  （由 `envboard-core-mitmproxy` 物化到每个环境自己的目录里再拉起）。
  没有 `envboard_*` 选项：那些值全在 `config.json` 里。
* **作为 CLI**：`python3 envboard_mitmproxy.py --render --fixture <case.json>`
  —— 把规范化文本打到 stdout，供跨语言对拍使用。
  这个模式**不需要装 mitmproxy**，因为它的解析与渲染只依赖标准库。

## 为什么不用 mitmproxy 的 `dns_resolver`

HTTP 代理模式的上连根本不走 mitmproxy 的 resolver（`proxy/server.py` 直接
`asyncio.open_connection(*address)`），per-host 覆盖只有"改连到哪"这一条路。
"""

# 注意：**不要**在这里写 `from __future__ import annotations`。
# mitmdump 用 `-s` 加载脚本时不会把模块登记进 `sys.modules`，而 `@dataclass` 在处理
# 字符串注解时要去 `sys.modules[cls.__module__]` 查命名空间 —— 结果就是启动即崩
# （`AttributeError: 'NoneType' object has no attribute '__dict__'`）。
# 实测踩过并据此去掉：当时那句 `from __future__ import annotations` 就是这么崩的。
import argparse
import hashlib
import ipaddress
import json
import os
import re
import sys
import threading
import time
from dataclasses import dataclass, field
from typing import Any

# 作为 addon 时由 mitmdump 提供 ctx；作为 CLI 时没有它，此时模块仍可正常导入。
try:  # pragma: no cover - 取决于运行方式
    from mitmproxy import ctx
except Exception:  # noqa: BLE001 - 任何导入失败都退化为 CLI 模式
    ctx = None  # type: ignore[assignment]

AGENT_VERSION = "0.3.0"
STATUS_MIN_INTERVAL = 1.0

#: host 的 label 规则，与 `core/spec/rules.md` §3.1 一字不差。
LABEL_RE = re.compile(r"^(?!-)[A-Za-z0-9_-]{1,63}(?<!-)$")

RULES_SUFFIX = ".rules"


# --------------------------------------------------------------------------- #
# 契约：解析（core/spec/rules.md）
# --------------------------------------------------------------------------- #


def normalize_host(value: str) -> str:
    """去空白 → 转小写 → 去尾部根点 → 剥掉**一个**前导 `*.`。

    `*.` 只是被剥掉，**不做通配后缀匹配**：`*.wild.example.com` 的语义是
    "wild.example.com 这一个域名"。
    """
    host = value.strip().lower().rstrip(".")
    if host.startswith("*."):
        host = host[2:]
    return host


def parse_ip(token: str) -> str | None:
    """裸 IP 字面量 → 剥掉方括号后的**原始文本**（不做规范化）；不合法返回 None。

    刻意拒绝带 zone id 的 IPv6（`fe80::1%eth0`）：Python 的 `ipaddress` 会接受它，
    而 Rust 的 `IpAddr::from_str` 不接受 —— 契约按更严的一侧取（`core/spec/rules.md` §3.2），
    所以这里必须自己判 `%`，不能依赖 `ipaddress` 的默认行为。
    """
    value = token.strip()
    if value.startswith("[") and value.endswith("]"):
        value = value[1:-1]
    if not value or "%" in value:
        return None
    try:
        ipaddress.ip_address(value)
    except ValueError:
        return None
    return value


def is_ip_literal(token: str) -> bool:
    return parse_ip(token) is not None


def validate_host(token: str) -> str | None:
    host = normalize_host(token)
    if not host or len(host) > 253:
        return None
    for label in host.split("."):
        if not LABEL_RE.match(label):
            return None
    return host


def split_lines(text: str) -> list[str]:
    """按契约承认的三种行终止符切分：`\\n`、`\\r\\n`、`\\r`，末尾不产生空行。

    **不能**直接用 `str.splitlines()`：它还会在 `\\v`/`\\f`/`\\x1c`–`\\x1e`/`\\x85`/
    `\\u2028`/`\\u2029` 断行，那属于 v1 的实现细节、不在 v2 契约内。
    """
    text = text.lstrip("\ufeff")
    lines: list[str] = []
    current: list[str] = []
    index = 0
    length = len(text)
    while index < length:
        char = text[index]
        if char == "\n":
            lines.append("".join(current))
            current = []
        elif char == "\r":
            if index + 1 < length and text[index + 1] == "\n":
                index += 1
            lines.append("".join(current))
            current = []
        else:
            current.append(char)
        index += 1
    if current:
        lines.append("".join(current))
    return lines


@dataclass(frozen=True)
class SkippedItem:
    line: int
    text: str
    reason: str
    detail: str = ""


@dataclass(frozen=True)
class HostConflict:
    host: str
    dropped: str
    kept: str


@dataclass
class RulesImport:
    entries: dict[str, str] = field(default_factory=dict)
    skipped: list[SkippedItem] = field(default_factory=list)
    conflicts: list[HostConflict] = field(default_factory=list)
    total_lines: int = 0
    blank_lines: int = 0
    comment_lines: int = 0
    data_lines: int = 0

    @property
    def accepted(self) -> int:
        return len(self.entries)

    @property
    def ips(self) -> int:
        return len(set(self.entries.values()))

    def stats_json(self) -> dict[str, int]:
        return {
            "total_lines": self.total_lines,
            "blank_lines": self.blank_lines,
            "comment_lines": self.comment_lines,
            "data_lines": self.data_lines,
            "accepted": self.accepted,
            "skipped": len(self.skipped),
            "conflicts": len(self.conflicts),
        }

    def to_json(self) -> dict[str, Any]:
        return {
            "entries": dict(self.entries),
            "accepted": self.accepted,
            "ips": self.ips,
            "conflicts": [[c.host, c.dropped, c.kept] for c in self.conflicts],
            "skipped": [
                {"line": s.line, "text": s.text, "reason": s.reason, "detail": s.detail}
                for s in self.skipped
            ],
            "stats": self.stats_json(),
        }


def parse_hosts_text(text: str) -> RulesImport:
    """解析 hosts 风格文本。**不抛异常**：非法内容记进 `skipped`。"""
    result = RulesImport()
    for index, raw in enumerate(split_lines(text)):
        result.total_lines += 1
        body = raw.split("#", 1)[0].strip()
        if not body:
            if raw.strip().startswith("#"):
                result.comment_lines += 1
            else:
                result.blank_lines += 1
            continue
        result.data_lines += 1
        line = index + 1
        tokens = body.split()

        if len(tokens) < 2:
            result.skipped.append(
                SkippedItem(line, body, "too-few-tokens", tokens[0] if tokens else "")
            )
            continue

        if is_ip_literal(tokens[0]):
            ip_token, host_tokens = tokens[0], tokens[1:]
        elif is_ip_literal(tokens[-1]):
            ip_token, host_tokens = tokens[-1], tokens[:-1]
        else:
            result.skipped.append(SkippedItem(line, body, "no-ip-literal", ""))
            continue

        ip_text = parse_ip(ip_token)
        if ip_text is None:  # 防御分支：上面已经判过
            result.skipped.append(SkippedItem(line, body, "no-ip-literal", ip_token))
            continue

        for token in host_tokens:
            if is_ip_literal(token):
                result.skipped.append(SkippedItem(line, body, "invalid-host", token))
                continue
            host = validate_host(token)
            if host is None:
                result.skipped.append(SkippedItem(line, body, "invalid-host", token))
                continue
            previous = result.entries.get(host)
            if previous is not None and previous != ip_text:
                result.conflicts.append(HostConflict(host, previous, ip_text))
            result.entries[host] = ip_text  # 后出现者胜

    return result


# --------------------------------------------------------------------------- #
# 契约：渲染（core/spec/rules.md §6）—— 与 Rust 侧逐字节一致
# --------------------------------------------------------------------------- #

_HEADER = (
    "# envboard rules file — normalized `ip host...` lines.",
    "# Generated by envboard; re-import the source instead of editing this by hand.",
)


def _ip_sort_key(ip: str) -> tuple[int, bytes]:
    try:
        address = ipaddress.ip_address(ip)
    except ValueError:
        return (255, b"")
    return (address.version, address.packed)


def render_rules(entries: dict[str, str], source: str = "") -> str:
    """同一份 entries + source 必须渲染出逐字节相同的文本。"""
    normalized: dict[str, str] = {}
    for host, ip in entries.items():
        host = normalize_host(host)
        if host in normalized:
            raise ValueError(f"two entries normalize to the same host {host!r}")
        normalized[host] = ip

    by_ip: dict[str, list[str]] = {}
    for host, ip in normalized.items():
        by_ip.setdefault(ip, []).append(host)

    lines = list(_HEADER)
    if source:
        lines.append(f"# source: {source}")
    lines.append(f"# entries: {len(normalized)}  ip: {len(by_ip)}")
    lines.append("")
    for ip in sorted(by_ip, key=_ip_sort_key):
        lines.append(f"{ip} {' '.join(sorted(by_ip[ip]))}")
    return "\n".join(lines) + "\n"


# --------------------------------------------------------------------------- #
# 每环境配置（Rust ←→ 注入器之间的唯一通道）
# --------------------------------------------------------------------------- #

#: 由管理器写在注入器**旁边**的配置文件名。
CONFIG_FILE_NAME = "config.json"
#: 规则文件的固定名软链 —— 绑定哪份规则由它指向谁决定。
RULES_LINK_NAME = "envboard.rules"
#: `config.json` 的格式版本；不认识的版本一律拒绝。
CONFIG_VERSION = 1
#: 放行域名清单的条数上限（与 Rust 侧同一个数字）。
INSECURE_HOSTS_MAX = 200
NAME_RE = re.compile(r"^[a-z][a-z0-9_-]{0,31}$")
CONFIG_KEYS = frozenset(
    {
        "version",
        "env",
        "status_file",
        "rules",
        "insecure_hosts",
        "launch_expected",
        "reload_interval_secs",
        "annotate",
    }
)


class ConfigError(Exception):
    """配置不可用。

    两条路径的行为刻意不同：**启动**时首读失败即抛错退出（不留半配置实例）；
    **热重载**时失败只记 `config_error` 并保留上一份快照（不中断代理）。
    """


def agent_dir() -> str:
    """注入器自己所在的目录 —— 配置与规则软链都在它旁边。"""
    return os.path.dirname(os.path.abspath(__file__))


# --------------------------------------------------------------------------- #
# 契约：insecure_hosts（按域名放宽上游校验）
#
# 与 rules 的 host 归一化**刻意不同**：不剥 `*.`，出现 `*` / `?` 一律非法；
# 匹配是**完全相等**（无子域继承、无后缀匹配）。
# --------------------------------------------------------------------------- #


def normalize_insecure_host(value: str) -> str:
    return value.strip().lower().rstrip(".")


def validate_insecure_host(value: str) -> str | None:
    """归一化 + 校验；非法（通配符 / label 不合法）返回 None。"""
    host = normalize_insecure_host(value)
    if "*" in host or "?" in host:
        return None
    return validate_host(host)


def insecure_candidate(sni: Any, address: Any) -> str | None:
    """命中判定的候选：归一化后的 SNI；没有 SNI 时退回上连地址。

    SNI 存在但不在名单里**不**退回地址 —— 退回会让一次命名失配变成一次静默放行。
    """
    raw = sni if sni else address
    if not raw:
        return None
    host = normalize_insecure_host(str(raw))
    return host or None


def insecure_match(case_input: dict[str, Any]) -> dict[str, Any]:
    """契约 fixture（`insecure.hosts`）的参考实现 —— 与 Rust 侧逐条对齐。"""
    hosts = case_input.get("hosts") or []
    candidate = insecure_candidate(case_input.get("sni"), case_input.get("address"))
    return {"match": candidate is not None and candidate in set(hosts)}


# --------------------------------------------------------------------------- #
# 配置解析（强 schema）
# --------------------------------------------------------------------------- #


def parse_config(raw: bytes) -> dict[str, Any]:
    """把 `config.json` 的字节解析成一份校验过的配置。

    **未知键即非法**：配置文件是契约，宽松解析会让"写错了键名"变成静默失效。
    """
    try:
        doc = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, ValueError) as exc:
        raise ConfigError(f"{CONFIG_FILE_NAME} is not valid JSON: {exc}") from exc
    if not isinstance(doc, dict):
        raise ConfigError(f"{CONFIG_FILE_NAME} must be a JSON object")

    unknown = sorted(set(doc) - CONFIG_KEYS)
    if unknown:
        raise ConfigError(f"{CONFIG_FILE_NAME} has unknown keys: {unknown}")

    version = doc.get("version")
    if version != CONFIG_VERSION:
        raise ConfigError(
            f"unsupported {CONFIG_FILE_NAME} version {version!r} (expected {CONFIG_VERSION})"
        )

    env = doc.get("env")
    if not isinstance(env, str) or not env:
        raise ConfigError("config.env must be a non-empty string")

    status_file = doc.get("status_file")
    if not isinstance(status_file, str) or not status_file:
        raise ConfigError("config.status_file must be a non-empty string")

    rules = doc.get("rules")
    if rules is not None and (not isinstance(rules, str) or not NAME_RE.match(rules)):
        raise ConfigError("config.rules must be null or a rules name (^[a-z][a-z0-9_-]{0,31}$)")

    raw_hosts = doc.get("insecure_hosts")
    if not isinstance(raw_hosts, list):
        raise ConfigError("config.insecure_hosts must be an array")
    if len(raw_hosts) > INSECURE_HOSTS_MAX:
        raise ConfigError(
            f"config.insecure_hosts has {len(raw_hosts)} entries; the limit is {INSECURE_HOSTS_MAX}"
        )
    hosts: list[str] = []
    for index, item in enumerate(raw_hosts):
        host = validate_insecure_host(item) if isinstance(item, str) else None
        if host is None:
            raise ConfigError(
                f"config.insecure_hosts[{index}] is not a complete domain name "
                "(wildcards are rejected: list every domain explicitly)"
            )
        if host not in hosts:
            hosts.append(host)

    expected = doc.get("launch_expected")
    if not isinstance(expected, dict) or not all(
        isinstance(key, str) and isinstance(value, str) for key, value in expected.items()
    ):
        raise ConfigError("config.launch_expected must be an object of strings")

    interval = doc.get("reload_interval_secs")
    if not isinstance(interval, int) or isinstance(interval, bool) or interval < 1:
        raise ConfigError("config.reload_interval_secs must be an integer >= 1")

    annotate = doc.get("annotate")
    if not isinstance(annotate, bool):
        raise ConfigError("config.annotate must be a boolean")

    return {
        "env": env,
        "status_file": status_file,
        "rules": rules,
        "insecure_hosts": sorted(hosts),
        "launch_expected": dict(expected),
        "reload_interval_secs": interval,
        "annotate": annotate,
    }


def resolve_rules(directory: str, config: dict[str, Any]) -> tuple[str | None, str | None]:
    """把"固定名软链"解析成规则文件路径。

    返回 `(路径或 None, 错误或 None)`。三种情形：
    * 链不存在 / 悬空 → `(None, None)`：该环境**不覆盖任何域名**（契约语义，不是错误）；
    * 链指向的名字与 `config.rules` 不一致 → `(None, 错误)`：响亮报错，**不猜**用哪一个；
    * 正常 → `(链路径, None)`。
    """
    link = os.path.join(directory, RULES_LINK_NAME)
    if not os.path.lexists(link):
        return None, None

    try:
        target = os.readlink(link)
    except OSError as exc:  # noqa: BLE001 - 读不动链就没什么可猜的
        return None, f"cannot read the rules link {link}: {exc}"

    base = os.path.basename(target)
    bound = config["rules"]
    if bound is None:
        return None, (
            f"the rules link points at {base!r} but the configuration binds no rules "
            "(the manager did not clean it up)"
        )
    if base != f"{bound}{RULES_SUFFIX}":
        return None, (
            f"the rules link points at {base!r} but the configuration binds {bound!r}; "
            "refusing to guess which one is in effect"
        )
    if not os.path.exists(link):
        # 悬空链：目标文件不在，与"链不存在"一样 = 不覆盖。
        return None, None
    return link, None


def load_rules(path: str) -> tuple[dict[str, str], int, str | None]:
    """读并解析规则文件；失败时返回空表 + 错误说明（调用方保留旧快照）。"""
    try:
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
    except OSError as exc:
        return {}, 0, f"cannot read the rules file {path}: {exc}"
    import_result = parse_hosts_text(text)
    return dict(import_result.entries), import_result.accepted, None


# --------------------------------------------------------------------------- #
# 运行时状态
# --------------------------------------------------------------------------- #


class Policy:
    """**不可变**的生效配置快照。

    hooks 只读它，热重载做的是 `self._policy = new` 这一次属性替换 ——
    所以单个连接内看到的一定是同一份配置（不会读到"改了一半"的状态）。
    """

    __slots__ = (
        "env",
        "annotate",
        "insecure_hosts",
        "entries",
        "rules_path",
        "rules_count",
        "config_hash",
        "reload_interval_secs",
        "launch_expected",
    )

    def __init__(
        self,
        *,
        env: str,
        annotate: bool,
        insecure_hosts: frozenset[str],
        entries: dict[str, str],
        rules_path: str | None,
        rules_count: int,
        config_hash: str,
        reload_interval_secs: int,
        launch_expected: dict[str, str],
    ) -> None:
        self.env = env
        self.annotate = annotate
        self.insecure_hosts = insecure_hosts
        self.entries = entries
        self.rules_path = rules_path
        self.rules_count = rules_count
        self.config_hash = config_hash
        self.reload_interval_secs = reload_interval_secs
        self.launch_expected = launch_expected


def build_policy(
    directory: str, config: dict[str, Any], raw: bytes
) -> tuple[Policy, str | None]:
    """从一份已校验的配置构建快照。绑定的链不一致时抛 [`ConfigError`]。"""
    rules_path, link_error = resolve_rules(directory, config)
    if link_error:
        raise ConfigError(link_error)
    entries: dict[str, str] = {}
    count = 0
    rules_error: str | None = None
    if rules_path:
        entries, count, rules_error = load_rules(rules_path)

    policy = Policy(
        env=config["env"],
        annotate=config["annotate"],
        insecure_hosts=frozenset(config["insecure_hosts"]),
        entries=entries,
        rules_path=rules_path,
        rules_count=count,
        config_hash=hashlib.sha256(raw).hexdigest(),
        reload_interval_secs=config["reload_interval_secs"],
        launch_expected=config["launch_expected"],
    )
    return policy, rules_error


def link_stamp(link: str) -> tuple[Any, ...]:
    """软链的观测指纹：链指向谁 + 目标文件的 `(mtime_ns, size)`。

    为什么要带 `readlink` 的结果：换绑定可能让链的 mtime 分辨率不够，
    但目标名一定变；而"目标内容被重写"则改 mtime/size。两件事都覆盖到了。
    """
    try:
        target = os.readlink(link)
    except OSError:
        return ("absent",)
    try:
        stat = os.stat(link)
    except OSError:
        return (target, "dangling")
    return (target, stat.st_mtime_ns, stat.st_size)


class StatusWriter:
    """周期性原子回写状态文件（管理器**以它为主**判定健康）。"""

    def __init__(self, path: str, payload: dict[str, Any]) -> None:
        self.path = path
        self.payload = payload
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None

    def write(self, **changes: Any) -> None:
        self.payload.update(changes)
        self.payload["updated_at"] = int(time.time())
        if not self.path:
            return
        directory = os.path.dirname(self.path)
        if directory:
            os.makedirs(directory, exist_ok=True)
        tmp = f"{self.path}.tmp"
        with open(tmp, "w", encoding="utf-8") as handle:
            json.dump(self.payload, handle, ensure_ascii=False)
        os.replace(tmp, self.path)

    def start(self) -> None:
        self._thread = threading.Thread(target=self._loop, name="envboard-status", daemon=True)
        self._thread.start()

    def _loop(self) -> None:
        # 只刷新时间戳：让"新鲜度"持续成立，同时把真实变化留给显式 write()。
        while not self._stop.wait(STATUS_MIN_INTERVAL):
            self.write()

    def stop(self) -> None:
        self._stop.set()
        if self.path:
            try:
                os.unlink(self.path)
            except OSError:
                pass


# --------------------------------------------------------------------------- #
# TLS：按域名放宽上游校验
# --------------------------------------------------------------------------- #


def _unverified_server_context(tls_start: Any, server: Any, host: str) -> Any:
    """为本条连接自建一个 `VERIFY_NONE` 的上游 context。

    主体与 mitmproxy 自己的 `tlsconfig.tls_start_server` 逐项对齐（同一批
    `ctx.options`），**唯一的语义差别**是 `verify=VERIFY_NONE`：
    证书链不校验、主机名不校验，其余（ALPN、cipher、TLS 版本、ECDH 曲线）保持原样。

    实测到的两条边界（踩过才知道）：
    * `-s` 脚本的 `tls_start_server` **先于** `tlsconfig` 执行 —— 所以这里能抢先
      提供 `ssl_conn`，后者看到非 None 就直接返回；
    * `server.sni` 必须显式设置，否则测试机按 SNI 选不到 vhost（客户端可不发 SNI）。
    """
    from OpenSSL import SSL
    from mitmproxy.net import tls as net_tls

    try:
        from mitmproxy.addons.tlsconfig import _default_ciphers
    except ImportError:  # pragma: no cover - 私有 API 漂移时的兜底
        _default_ciphers = None

    client = tls_start.context.client
    if not server.alpn_offers:
        if client.alpn_offers:
            server.alpn_offers = (
                tuple(client.alpn_offers)
                if ctx.options.http2
                else tuple(item for item in client.alpn_offers if item != b"h2")
            )
        else:
            server.alpn_offers = []

    cipher_list = server.cipher_list or (
        ctx.options.ciphers_server.split(":")
        if ctx.options.ciphers_server
        else (
            _default_ciphers(net_tls.Version[ctx.options.tls_version_server_min])
            if _default_ciphers
            else None
        )
    )

    ssl_ctx = net_tls.create_proxy_server_context(
        method=net_tls.Method.TLS_CLIENT_METHOD,
        min_version=net_tls.Version[ctx.options.tls_version_server_min],
        max_version=net_tls.Version[ctx.options.tls_version_server_max],
        cipher_list=tuple(cipher_list) if cipher_list else None,
        ecdh_curve=net_tls.get_curve(ctx.options.tls_ecdh_curve_server),
        verify=net_tls.Verify.VERIFY_NONE,
        ca_path=None,
        ca_pemfile=None,
        client_cert=None,
        legacy_server_connect=True,
    )
    ssl_conn = SSL.Connection(ssl_ctx)
    server.sni = host
    try:
        ipaddress.ip_address(host)
    except ValueError:
        ssl_conn.set_tlsext_host_name(host.encode("idna"))
    if server.alpn_offers:
        ssl_conn.set_alpn_protos(list(server.alpn_offers))
    ssl_conn.set_connect_state()
    return ssl_conn


# --------------------------------------------------------------------------- #
# addon
# --------------------------------------------------------------------------- #


class Injector:
    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._policy = Policy(
            env="",
            annotate=False,
            insecure_hosts=frozenset(),
            entries={},
            rules_path=None,
            rules_count=0,
            config_hash="",
            reload_interval_secs=5,
            launch_expected={},
        )
        self._config_error: str | None = None
        self._rules_error: str | None = None
        self._seen_hash = ""
        self._rules_stamp: tuple[Any, ...] = ()
        self._interval = 5.0
        self._directory = ""
        self._listen_host = ""
        self._listen_port = 0
        self._echo: dict[str, str] = {}
        self.status: StatusWriter | None = None
        self.rewrites = 0
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None

    # -- 生命周期 ---------------------------------------------------------- #

    def running(self) -> None:
        """首读配置并建立运行时。

        **启动路径不留半配置实例**：读不到、读不懂、链对不上，一律抛错退出 ——
        管理器等不到新鲜的状态文件，就会把这次启动判定为失败并收掉进程。
        """
        self._directory = agent_dir()
        options = ctx.options  # type: ignore[union-attr]
        self._listen_host = str(options.listen_host)
        self._listen_port = int(options.listen_port)

        config_path = os.path.join(self._directory, CONFIG_FILE_NAME)
        try:
            with open(config_path, "rb") as handle:
                raw = handle.read()
        except OSError as exc:
            raise RuntimeError(
                f"envboard: cannot read {config_path}: {exc} "
                "(the manager writes it before launch)"
            ) from exc

        config = parse_config(raw)
        policy, rules_error = build_policy(self._directory, config, raw)
        self._policy = policy
        self._rules_error = rules_error
        self._seen_hash = policy.config_hash
        self._rules_stamp = link_stamp(os.path.join(self._directory, RULES_LINK_NAME))
        self._interval = float(policy.reload_interval_secs)
        self._echo = self._verify_expected(options, policy.launch_expected)

        try:
            from mitmproxy import version as mitmproxy_version

            core_version = mitmproxy_version.VERSION
        except Exception:  # noqa: BLE001
            core_version = "unknown"

        self.status = StatusWriter(
            config["status_file"],
            {
                "env_name": policy.env,
                "pid": os.getpid(),
                "listen": {"host": self._listen_host, "port": self._listen_port},
                "rules_path": policy.rules_path,
                "rules_count": policy.rules_count,
                "insecure_hosts_count": len(policy.insecure_hosts),
                "config_hash": policy.config_hash,
                "config_error": None,
                "reload_interval_secs": policy.reload_interval_secs,
                "core_version": core_version,
                "agent_version": AGENT_VERSION,
                "updated_at": int(time.time()),
                "last_error": self._rules_error,
                "options_echo": self._echo,
            },
        )
        self.status.write()
        self.status.start()

        self._thread = threading.Thread(target=self._loop, name="envboard-reload", daemon=True)
        self._thread.start()
        ctx.log.info(  # type: ignore[union-attr]
            f"envboard injector ready: env={policy.env} rules={policy.rules_count} "
            f"insecure_hosts={len(policy.insecure_hosts)}"
        )

    @staticmethod
    def _verify_expected(options: Any, expected: dict[str, str]) -> dict[str, str]:
        """逐项核对"管理器下发的启动契约键"有没有被宿主接受。

        为什么需要：**mitmproxy 对未知或拼错的 `--set` 是静默忽略的**（实测），
        所以"命令没报错"不能当作配置生效。`proxyauth` 是安全控制 ——
        被忽略就等于代理在"以为开了鉴权"的状态下裸奔，必须让管理器看得见。
        """
        verdicts: dict[str, str] = {}
        for key, value in expected.items():
            try:
                actual = getattr(options, key)
            except Exception:  # noqa: BLE001 - 未注册的键各版本抛的异常不一样
                verdicts[key] = (
                    "unknown option: mitmproxy silently ignores unknown --set keys, "
                    "so this setting never took effect"
                )
                continue
            verdicts[key] = "ok" if str(actual) == str(value) else f"expected {value!r}, got {actual!r}"
        return verdicts

    def done(self) -> None:
        self._stop.set()
        if self.status:
            self.status.stop()

    # -- 热重载 ------------------------------------------------------------ #

    def _loop(self) -> None:
        while not self._stop.wait(self._interval):
            try:
                self._refresh()
            except Exception as exc:  # noqa: BLE001 - 轮询线程绝不能死
                with self._lock:
                    self._config_error = f"hot reload failed: {exc}"
                self._publish()

    def _refresh(self) -> None:
        """`config.json` 与规则目标有变化就整份重读；失败保留旧快照。"""
        config_path = os.path.join(self._directory, CONFIG_FILE_NAME)
        try:
            with open(config_path, "rb") as handle:
                raw = handle.read()
        except OSError as exc:
            with self._lock:
                self._config_error = f"cannot read {CONFIG_FILE_NAME}: {exc}"
            self._publish()
            return

        # 配置按**内容哈希**判变化（比 mtime 稳，且文件只有几百字节）；
        # 规则按链 + 目标的 `(mtime_ns, size)` 判 —— 换绑定与改内容都覆盖到。
        digest = hashlib.sha256(raw).hexdigest()
        stamp = link_stamp(os.path.join(self._directory, RULES_LINK_NAME))
        with self._lock:
            if digest == self._seen_hash and stamp == self._rules_stamp:
                return
            self._seen_hash = digest
            self._rules_stamp = stamp

        try:
            config = parse_config(raw)
            policy, rules_error = build_policy(self._directory, config, raw)
        except ConfigError as exc:
            # 响亮但**不致命**：代理继续按上一份快照干活，状态文件里报出来。
            with self._lock:
                self._config_error = str(exc)
            self._publish()
            return

        with self._lock:
            self._policy = policy
            self._config_error = None
            self._rules_error = rules_error
            self._interval = float(policy.reload_interval_secs)
        self._publish()
        ctx.log.info(  # type: ignore[union-attr]
            f"envboard injector reloaded: rules={policy.rules_count} "
            f"insecure_hosts={len(policy.insecure_hosts)}"
        )

    def _publish(self) -> None:
        if self.status is None:
            return
        with self._lock:
            policy = self._policy
            config_error = self._config_error
            rules_error = self._rules_error
        self.status.write(
            rules_path=policy.rules_path,
            rules_count=policy.rules_count,
            insecure_hosts_count=len(policy.insecure_hosts),
            config_hash=policy.config_hash,
            config_error=config_error,
            reload_interval_secs=policy.reload_interval_secs,
            last_error=rules_error,
        )

    # -- 核心：改写上连目标 ------------------------------------------------- #

    def server_connect(self, data) -> None:
        """只改"往哪连"：不动请求内容、不动 Host 头、不动 SNI 基准。

        钩子里**只做一次字典查找**（微秒级）。`server_connect` 是 blocking hook
        （`StartHook.blocking = True`），但 blocking 的含义是"本层暂停并缓冲事件"，
        真正的禁忌是在钩子里做 I/O —— 这里没有 I/O。
        """
        policy = self._policy
        address = data.server.address
        if not address:
            return
        host, port = address
        target = policy.entries.get(host.lower().rstrip("."))
        if not target:
            return
        self.rewrites += 1
        data.server.address = (target, port)

    def tls_start_server(self, tls_start) -> None:
        """名单内的域名放宽上游证书校验；其余一律严格。

        单次取快照：一条连接内看到的配置是一致的。判定是**精确相等**
        （契约见 `insecure_hosts` 一节），没有通配、没有后缀匹配。
        """
        policy = self._policy
        if not policy.insecure_hosts:
            return
        server = tls_start.conn
        try:
            from mitmproxy import connection
        except Exception:  # noqa: BLE001 - 拿不到类型就没法安全判定
            return
        if not isinstance(server, connection.Server) or not server.address:
            return
        if tls_start.ssl_conn is not None:
            # 已有 addon 提供了 context（或我们不是第一个），不抢。
            return

        host = insecure_candidate(tls_start.context.client.sni, server.address[0])
        if host is None or host not in policy.insecure_hosts:
            return

        try:
            # 只在最后一步赋值：中途出错就什么都不改，让 mitmproxy 走**严格**校验。
            # 宁可连不上，也不能在出错时把校验静默关掉。
            ssl_conn = _unverified_server_context(tls_start, server, host)
        except Exception as exc:  # noqa: BLE001
            ctx.log.warn(  # type: ignore[union-attr]
                f"envboard: cannot relax upstream verification for {host}: {exc}"
            )
            return
        tls_start.ssl_conn = ssl_conn

    def request(self, flow) -> None:
        policy = self._policy
        if policy.annotate and policy.env:
            flow.comment = f"[env:{policy.env}]"

    # -- 给对拍用的自省 ---------------------------------------------------- #

    def status_payload(self) -> dict[str, Any]:
        return dict(self.status.payload) if self.status else {}


# --------------------------------------------------------------------------- #
# CLI 模式：--render（跨语言对拍用，不需要 mitmproxy）
# --------------------------------------------------------------------------- #


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="envboard injector")
    parser.add_argument("--render", action="store_true", help="render a fixture and exit")
    parser.add_argument("--fixture", help="path to a core/spec/fixtures case")
    args = parser.parse_args(argv[1:])

    if not args.render:
        parser.print_help()
        return 2
    if not args.fixture:
        print("--render requires --fixture", file=sys.stderr)
        return 2

    with open(args.fixture, encoding="utf-8") as handle:
        case = json.load(handle)
    text = (case.get("input") or {}).get("text") or ""
    source = case.get("source") or ""
    parsed = parse_hosts_text(text)
    sys.stdout.write(render_rules(parsed.entries, source))
    return 0


# mitmdump 以 `-s` 加载时不会执行 main()；直接运行时才走 CLI。
if ctx is not None:
    addons = [Injector()]  # type: ignore[assignment]
else:
    addons = []


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))

#!/usr/bin/env python3
"""envboard 注入器 —— 一个文件、纯标准库（外加 mitmproxy 本身），职责只有三件事。

1. **读 + 解析 hosts 规则**（契约：`core/spec/rules.md` 的 BNF）；
2. **在 `server_connect` 里改写上连目标**（只改"往哪连"，不改请求内容、Host、SNI 基准）；
3. **回写状态文件**，让管理器能判断"配置到底有没有生效"。

它**不参与环境管理**：不知道自己是哪个环境（`envboard_env_name` 只是展示数据），
不知道有哪些兄弟实例，也不碰任何状态目录。这样才能"随时可以丢掉"。

## 两种用法

* **作为 addon**：`mitmdump -s envboard_mitmproxy.py --set envboard_rules=...`
  （由 `envboard-core-mitmproxy` 物化并拉起）；
* **作为 CLI**：`python3 envboard_mitmproxy.py --render --fixture <case.json>`
  —— 把规范化文本打到 stdout，供跨语言对拍使用。
  这个模式**不需要装 mitmproxy**，因为它的解析与渲染只依赖标准库。

## 为什么不用 mitmproxy 的 `dns_resolver`

HTTP 代理模式的上连根本不走 mitmproxy 的 resolver（`proxy/server.py` 直接
`asyncio.open_connection(*address)`），per-host 覆盖只有"改连到哪"这一条路。
证据与替代方案对比见。
"""

# 注意：**不要**在这里写 `from __future__ import annotations`。
# mitmdump 用 `-s` 加载脚本时不会把模块登记进 `sys.modules`，而 `@dataclass` 在处理
# 字符串注解时要去 `sys.modules[cls.__module__]` 查命名空间 —— 结果就是启动即崩
# （`AttributeError: 'NoneType' object has no attribute '__dict__'`）。
# 实测踩过并据此去掉：当时那句 `from __future__ import annotations` 就是这么崩的。
import argparse
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

AGENT_VERSION = "0.2.0"
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
# 运行时状态
# --------------------------------------------------------------------------- #


class RuleTable:
    """规则表 + mtime 热重载。

    轮询而不是 inotify：注入器要保持"单文件、纯标准库、跨平台"，而 mtime 轮询
    是这三条下唯一不引入依赖的做法（延迟等于轮询间隔，默认 5s，够用）。
    """

    def __init__(self, path: str, interval: float) -> None:
        self.path = path
        self.interval = max(float(interval), 1.0)
        self._lock = threading.Lock()
        self._entries: dict[str, str] = {}
        self._mtime: float | None = None
        self._count = 0
        self._error: str | None = None
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None

    @property
    def entries(self) -> dict[str, str]:
        with self._lock:
            return self._entries

    @property
    def count(self) -> int:
        with self._lock:
            return self._count

    @property
    def error(self) -> str | None:
        with self._lock:
            return self._error

    def load(self) -> None:
        if not self.path:
            with self._lock:
                self._entries, self._count, self._error = {}, 0, None
            return
        try:
            stat = os.stat(self.path)
            with self._lock:
                if self._mtime == stat.st_mtime:
                    return
            with open(self.path, encoding="utf-8") as handle:
                text = handle.read()
            parsed = parse_hosts_text(text)
            with self._lock:
                self._entries = dict(parsed.entries)
                self._count = parsed.accepted
                self._mtime = stat.st_mtime
                self._error = None
        except OSError as exc:
            with self._lock:
                self._error = f"cannot read rules file {self.path}: {exc}"

    def start(self) -> None:
        self.load()
        self._thread = threading.Thread(target=self._loop, name="envboard-reload", daemon=True)
        self._thread.start()

    def _loop(self) -> None:
        while not self._stop.wait(self.interval):
            self.load()

    def stop(self) -> None:
        self._stop.set()


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
# addon
# --------------------------------------------------------------------------- #


class Injector:
    def __init__(self) -> None:
        self.rules: RuleTable | None = None
        self.status: StatusWriter | None = None
        self.rewrites = 0
        self._annotate = False
        self._env_name = ""

    # -- 生命周期 ---------------------------------------------------------- #

    def load(self, loader) -> None:
        loader.add_option("envboard_rules", str, "", "path to the rules file (absolute)")
        loader.add_option("envboard_status_file", str, "", "path to the status file (absolute)")
        # 类型必须是 mitmproxy 认的那几种（bool / int / str / Sequence[str]）。
        # 实测：`float` 会让 mitmdump **启动即失败**（`unsupported option type: float`），
        # 所以间隔用整秒 —— 契约里写的也是 `--set envboard_reload_interval=5`。
        loader.add_option(
            "envboard_reload_interval", int, 5, "seconds between rules file mtime checks"
        )
        loader.add_option("envboard_annotate_flow", bool, False, "tag flows with the env name")
        loader.add_option("envboard_env_name", str, "", "display-only environment name")
        loader.add_option(
            "envboard_expect",
            str,
            "",
            "JSON of options the manager intends to set; the injector verifies each one",
        )

    def running(self) -> None:
        options = ctx.options  # type: ignore[union-attr]
        self._annotate = bool(options.envboard_annotate_flow)
        self._env_name = str(options.envboard_env_name)
        listen_host = str(options.listen_host)
        listen_port = int(options.listen_port)

        self.rules = RuleTable(str(options.envboard_rules), int(options.envboard_reload_interval))
        self.rules.start()

        try:
            from mitmproxy import version as mitmproxy_version

            core_version = mitmproxy_version.VERSION
        except Exception:  # noqa: BLE001
            core_version = "unknown"

        echo = self._verify_expected_options(options)
        self.status = StatusWriter(
            str(options.envboard_status_file),
            {
                "env_name": self._env_name,
                "pid": os.getpid(),
                "listen": {"host": listen_host, "port": listen_port},
                "rules_path": str(options.envboard_rules) or None,
                "rules_count": self.rules.count,
                "reload_interval_secs": int(options.envboard_reload_interval),
                "core_version": core_version,
                "agent_version": AGENT_VERSION,
                "updated_at": int(time.time()),
                "last_error": self.rules.error,
                "options_echo": echo,
            },
        )
        self.status.write()
        self.status.start()
        ctx.log.info(  # type: ignore[union-attr]
            f"envboard injector ready: rules={self.rules.count} from {options.envboard_rules!r}"
        )

    @staticmethod
    def _verify_expected_options(options) -> dict[str, str]:
        """逐项核对"管理器下发的选项"有没有被宿主接受。

        为什么需要：**mitmproxy 对未知或拼错的 `--set` 是静默忽略的**（实测），
        所以"命令没报错"不能当作配置生效。注入器是唯一能回答这个问题的位置 ——
        它把结论写进状态文件，管理器据此判定 `config_mismatch`。
        """
        verdicts: dict[str, str] = {}
        raw = str(getattr(options, "envboard_expect", "") or "")
        if not raw:
            return verdicts
        try:
            expected = json.loads(raw)
        except ValueError as exc:
            verdicts["<envboard_expect>"] = f"cannot parse expect payload: {exc}"
            return verdicts
        if not isinstance(expected, dict):
            verdicts["<envboard_expect>"] = "expect payload must be a JSON object"
            return verdicts

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
        if self.rules:
            self.rules.stop()
        if self.status:
            self.status.stop()

    # -- 核心：改写上连目标 ------------------------------------------------- #

    def server_connect(self, data) -> None:
        """只改"往哪连"：不动请求内容、不动 Host 头、不动 SNI 基准。

        钩子里**只做一次字典查找**（微秒级）。`server_connect` 是 blocking hook
        （`StartHook.blocking = True`，`proxy/commands.py:121`），但 blocking 的含义是
        "本层暂停并缓冲事件"，真正的禁忌是在钩子里做 I/O —— 这里没有 I/O。
        """
        if self.rules is None:
            return
        address = data.server.address
        if not address:
            return
        host, port = address
        target = self.rules.entries.get(host.lower().rstrip("."))
        if not target:
            return
        self.rewrites += 1
        data.server.address = (target, port)

    def request(self, flow) -> None:
        if self._annotate and self._env_name:
            flow.comment = f"[env:{self._env_name}]"

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

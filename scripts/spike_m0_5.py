#!/usr/bin/env python3
"""M0.5 spike —— 用真 mitmdump 把风险表里排名最高的几个未知测掉。

**这是一个 spike，不是产品代码**：它的价值是"证据"，跑完就该能回答
"上连改写、共享 CA、连接复用、孤儿进程到底成不成立"。因此它刻意：

* 自包含（注入器与测试服务器的源码内嵌在下面，运行时物化到 /tmp）；
* 只用标准库 + mitmdump + openssl/curl，不依赖 Rust 侧任何东西；
* 每个断言都打印**可复核的证据**，而不是只报 PASS/FAIL。

跑法：`python3 scripts/spike_m0_5.py`（需要 mitmdump、openssl、curl 在本机可用）。
结果记录在 `docs/acceptance/m0.5-spike.md`。

重跑时请把输出一并更新到那份证据文件里 —— 否则文档会悄悄过期。
"""

from __future__ import annotations

#: spike 用的最小注入器：配置与规则都从"自己旁边"读（`config.json` + 固定名软链），
#: 行为只有两条 —— `server_connect` 改写上连地址、`tls_start_server` 按域名放宽上游校验。
#: 它**不注册任何 mitmproxy 选项**：所有值都走配置文件，和产品注入器同款通道。
INJECTOR_SOURCE = r'''
"""M0.5 spike 用的最小注入器 —— 配置与规则都在自己旁边，行为只有两条。

* `<本文件目录>/config.json`：唯一配置通道（强 schema，未知键即非法）；
* `<本文件目录>/envboard.rules`：固定名软链，指向真正生效的规则文件；
* `server_connect`：只改"往哪连"，不动请求内容、不动 Host、不动 SNI 基准；
* `tls_start_server`：`insecure_hosts` 里的域名用 `VERIFY_NONE` 自建上游 context，
  名单外的一律走 mitmproxy 自己的严格校验。

刻意最小：不做热重载（那是产品注入器的事）、不引任何第三方包。spike 要回答的是
"机制成不成立"，不是"产品做完了没有"。观测事件写成 JSONL 到 `<本文件目录>/events.jsonl`。
"""

from __future__ import annotations

import ipaddress
import json
import os
import time

from mitmproxy import ctx

HERE = os.path.dirname(os.path.abspath(__file__))
CONFIG_PATH = os.path.join(HERE, "config.json")
RULES_LINK = os.path.join(HERE, "envboard.rules")
EVENTS_PATH = os.path.join(HERE, "events.jsonl")

#: 与产品注入器逐键一致的严格 schema（多一个键就是写错了，必须响亮失败）。
CONFIG_KEYS = frozenset({
    "version", "env", "status_file", "rules",
    "insecure_hosts", "launch_expected", "reload_interval_secs", "annotate",
})

POLICY = {"env": "", "rules": {}, "insecure_hosts": frozenset(), "annotate": False}


def _log(event: str, **fields) -> None:
    record = {"t": round(time.time(), 3), "event": event, **fields}
    with open(EVENTS_PATH, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(record, ensure_ascii=False) + "\n")


def _normalize(host) -> str:
    return str(host).strip().lower().rstrip(".")


def _parse_config() -> dict:
    with open(CONFIG_PATH, "rb") as handle:
        doc = json.loads(handle.read().decode("utf-8"))
    unknown = sorted(set(doc) - CONFIG_KEYS)
    if unknown:
        raise RuntimeError(f"config.json has unknown keys: {unknown}")
    if doc.get("version") != 1:
        raise RuntimeError(f"unsupported config.json version {doc.get('version')!r}")
    return doc


def _resolve_rules(name) -> str | None:
    """固定名软链 → 规则文件路径。

    链不存在 / 悬空 = 该实例不覆盖任何域名（不是错误）；链指向的名字与
    `config.rules` 不一致则响亮报错，**不猜**用哪一个。
    """
    if not os.path.lexists(RULES_LINK):
        return None
    target = os.path.basename(os.readlink(RULES_LINK))
    if name is None or target != f"{name}.rules":
        raise RuntimeError(
            f"envboard.rules points at {target!r} but config.rules binds {name!r}; "
            "refusing to guess which one is in effect"
        )
    return RULES_LINK if os.path.exists(RULES_LINK) else None


def _read_rules(path) -> dict:
    rules: dict[str, str] = {}
    if not path:
        return rules
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            body = line.split("#", 1)[0].strip()
            if not body:
                continue
            tokens = body.split()
            if len(tokens) < 2:
                continue
            for host in tokens[1:]:
                rules[_normalize(host)] = tokens[0]
    return rules


def _write_status(config: dict, rules_path, rules_count: int) -> None:
    """回执一次状态文件，证明 `config.json` 确实被读进去了（产品侧是周期回写）。"""
    path = config.get("status_file")
    if not path:
        return
    payload = {
        "env_name": config.get("env"),
        "pid": os.getpid(),
        "rules_path": rules_path,
        "rules_count": rules_count,
        "insecure_hosts_count": len(POLICY["insecure_hosts"]),
        "updated_at": int(time.time()),
        "config_error": None,
    }
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(payload, handle, ensure_ascii=False)


def _unverified_context(tls_start, server, host: str):
    """名单内域名专供：自建 `VERIFY_NONE` 的上游 context。

    除 `verify` 外逐项抄 mitmproxy 自己的 `tlsconfig.tls_start_server`（ALPN、TLS 版本、
    ECDH 曲线、`legacy_server_connect`），且**必须显式设置 `server.sni`** —— 否则测试机
    按 SNI 选不到 vhost。
    """
    from OpenSSL import SSL
    from mitmproxy.net import tls as net_tls

    client = tls_start.context.client
    if not server.alpn_offers:
        server.alpn_offers = (
            tuple(client.alpn_offers)
            if ctx.options.http2
            else tuple(item for item in client.alpn_offers if item != b"h2")
        )
    ssl_ctx = net_tls.create_proxy_server_context(
        method=net_tls.Method.TLS_CLIENT_METHOD,
        min_version=net_tls.Version[ctx.options.tls_version_server_min],
        max_version=net_tls.Version[ctx.options.tls_version_server_max],
        cipher_list=None,
        ecdh_curve=net_tls.get_curve(ctx.options.tls_ecdh_curve_server),
        verify=net_tls.Verify.VERIFY_NONE,
        ca_path=None,
        ca_pemfile=None,
        client_cert=None,
        legacy_server_connect=True,
    )
    conn = SSL.Connection(ssl_ctx)
    server.sni = host
    try:
        ipaddress.ip_address(host)
    except ValueError:
        conn.set_tlsext_host_name(host.encode("idna"))
    if server.alpn_offers:
        conn.set_alpn_protos(list(server.alpn_offers))
    conn.set_connect_state()
    return conn


class Injector:
    def running(self) -> None:
        config = _parse_config()
        rules_path = _resolve_rules(config.get("rules"))
        POLICY["env"] = config.get("env") or ""
        POLICY["annotate"] = bool(config.get("annotate"))
        POLICY["insecure_hosts"] = frozenset(
            _normalize(item) for item in config.get("insecure_hosts") or []
        )
        POLICY["rules"] = _read_rules(rules_path)
        _write_status(config, rules_path, len(POLICY["rules"]))
        _log(
            "loaded",
            env=POLICY["env"],
            rules=dict(POLICY["rules"]),
            insecure_hosts=sorted(POLICY["insecure_hosts"]),
            pid=os.getpid(),
        )

    def server_connect(self, data) -> None:
        host, port = data.server.address
        target = POLICY["rules"].get(_normalize(host))
        _log(
            "server_connect",
            host=host,
            port=port,
            rewrite=target,
            sni=getattr(data.client, "sni", None),
        )
        if target:
            data.server.address = (target, port)

    def tls_start_server(self, tls_start) -> None:
        hosts = POLICY["insecure_hosts"]
        if not hosts or tls_start.ssl_conn is not None:
            return
        server = tls_start.conn
        if not getattr(server, "address", None):
            return
        sni = tls_start.context.client.sni
        host = _normalize(sni) if sni else _normalize(server.address[0])
        _log("tls_start_server", host=host, insecure=host in hosts)
        if host not in hosts:
            return
        # 只在最后一步赋值：中途出错就什么都不改，让 mitmproxy 走**严格**校验。
        tls_start.ssl_conn = _unverified_context(tls_start, server, host)


addons = [Injector()]
'''

#: 本地测试服务器：记录每条请求的 Host 头与客户端发来的 SNI。
SERVER_SOURCE = r'''
"""本地 HTTPS/HTTP 测试服务器：记录每条请求的 Host 头与客户端发来的 SNI。"""

from __future__ import annotations

import http.server
import json
import ssl
import sys
import threading

LAST_SNI: dict[str, str | None] = {"value": None}


def sni_callback(_socket, server_name, _context):
    LAST_SNI["value"] = server_name
    return None


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self) -> None:  # noqa: N802
        record = {
            "path": self.path,
            "host_header": self.headers.get("Host"),
            "sni": LAST_SNI["value"],
        }
        with open(sys.argv[3], "a", encoding="utf-8") as handle:
            handle.write(json.dumps(record) + "\n")
        body = b"ok"
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args) -> None:  # silence stderr noise
        pass


def main() -> int:
    port = int(sys.argv[1])
    mode = sys.argv[2]
    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler)
    if mode == "https":
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain("cert.pem", "key.pem")
        context.sni_callback = sni_callback
        server.socket = context.wrap_socket(server.socket, server_side=True)
    print(f"listening on 127.0.0.1:{port} ({mode})", flush=True)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    threading.Event().wait()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
'''


import json
import os
import pathlib
import shutil
import signal
import socket
import subprocess
import sys
import textwrap
import time

WORK = pathlib.Path("/tmp/spike/work")
MITMDUMP = shutil.which("mitmdump") or os.path.expanduser("~/.local/bin/mitmdump")
PROXY_BASE = 27_100
HTTP_PORT = 27_098
HTTPS_PORT = 27_099
REQUESTS = 4

#: 注入器旁边那三个固定名字：注入器自身、配置通道、规则软链（外加观测用的事件流）。
AGENT_INJECTOR = "envboard_mitmproxy.py"
RULES_LINK = "envboard.rules"
EVENTS_NAME = "events.jsonl"

RESULTS: list[tuple[str, bool, str]] = []


def check(name: str, ok: bool, evidence: str) -> None:
    RESULTS.append((name, ok, evidence))
    print(f"{'PASS' if ok else 'FAIL'}  {name}\n      {evidence}")


def run(command: list[str], **kwargs) -> subprocess.CompletedProcess:
    return subprocess.run(command, capture_output=True, text=True, **kwargs)


def wait_port(port: int, timeout: float = 15.0) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        with socket.socket() as probe:
            probe.settimeout(0.2)
            if probe.connect_ex(("127.0.0.1", port)) == 0:
                return True
        time.sleep(0.1)
    return False


def read_log(path: pathlib.Path) -> list[dict]:
    if not path.exists():
        return []
    records = []
    for line in path.read_text(encoding="utf-8").splitlines():
        try:
            records.append(json.loads(line))
        except ValueError:
            pass
    return records


def materialize_agent(name: str, rules: pathlib.Path, insecure_hosts: list[str]) -> pathlib.Path:
    """物化一个"注入器目录"：注入器 + `config.json` + 固定名规则软链。

    这就是产品契约里的配置下发方式（见 core/spec 的「配置下发与热重载」）：
    配置与规则都在注入器**旁边**，所以注入器不需要任何 `--set` 路径参数，
    也不需要知道自己是谁。spike 只是把它缩到最小。
    """
    agent = WORK / name
    agent.mkdir(parents=True, exist_ok=True)
    (agent / AGENT_INJECTOR).write_text(INJECTOR_SOURCE, encoding="utf-8")
    link = agent / RULES_LINK
    if os.path.lexists(link):
        link.unlink()
    link.symlink_to(rules)
    (agent / "config.json").write_text(
        json.dumps(
            {
                "version": 1,
                "env": name,
                "status_file": str(agent / "status.json"),
                # 规则名（不是路径）：软链必须指向 `<rules>.rules`，注入器会核对
                "rules": rules.stem,
                "insecure_hosts": list(insecure_hosts),
                "launch_expected": {},
                "reload_interval_secs": 1,
                "annotate": False,
            },
            ensure_ascii=False,
            indent=2,
        ) + "\n",
        encoding="utf-8",
    )
    return agent


def start_proxy(port: int, confdir: pathlib.Path, agent: pathlib.Path,
                wait: bool = True) -> subprocess.Popen:
    """拉起一个代理实例：`-s <agent>/envboard_mitmproxy.py` + core 自己的选项。

    刻意**没有任何 envboard_* 的 `--set`** —— 那些值全在 `<agent>/config.json` 里。
    `wait=False` 让调用方能先全部拉起再一起等（第 1 步的并发首启要这个顺序）。
    """
    command = [
        MITMDUMP,
        "-s", str(agent / AGENT_INJECTOR),
        "--set", f"confdir={confdir}",
        "--set", "listen_host=127.0.0.1",
        "--set", f"listen_port={port}",
        "--set", "termlog_verbosity=warn",
        "--set", "flow_detail=0",
    ]
    process = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    if wait and not wait_port(port):
        raise RuntimeError(f"proxy on {port} did not start: {process.stderr.read()[:400]!r}")
    return process


def agent_events(agent: pathlib.Path) -> list[dict]:
    """取回注入器写在**自己旁边**的观测事件流（JSONL）。"""
    return read_log(agent / EVENTS_NAME)


def leaf_certificate(proxy_port: int, host: str, port: int) -> str | None:
    """通过代理发起 CONNECT，取回 mitmproxy 对客户端出示的叶子证书（PEM）。"""
    result = run(
        [
            "openssl", "s_client",
            "-proxy", f"127.0.0.1:{proxy_port}",
            "-connect", f"{host}:{port}",
            "-servername", host,
            "-showcerts",
        ],
        input="",
        timeout=20,
    )
    text = result.stdout
    begin = text.find("-----BEGIN CERTIFICATE-----")
    if begin < 0:
        return None
    end = text.find("-----END CERTIFICATE-----", begin)
    return text[begin:end + len("-----END CERTIFICATE-----")] + "\n"


def verify_with_ca(leaf_pem: str, ca_pem: pathlib.Path) -> tuple[bool, str]:
    leaf = WORK / "leaf.pem"
    leaf.write_text(leaf_pem, encoding="utf-8")
    result = run(["openssl", "verify", "-CAfile", str(ca_pem), str(leaf)])
    return result.returncode == 0, (result.stdout + result.stderr).strip().splitlines()[-1]


def prematerialize_ca(confdir: pathlib.Path) -> None:
    """先把共享 CA 物化出来，再让多个实例去用它 —— 这是**产品契约的顺序**。

    为什么必须先物化：并发**首次**启动时，mitmproxy 的 CA 生成没有任何锁
    （`certs.py` 全文件无 `threading`/`Lock`），四个进程会各自生成一张 CA 并互相覆盖 ——
    有的实例内存里留着那张**已被覆盖**的 CA，它签出来的叶子就永远对不上磁盘上的那张。
    管理器因此在拉起任何实例之前先把 CA 准备好。

    单独起一个实例来物化，等两个关键文件出现后收掉：这也是"预物化"最直白的等价物。
    """
    port = PROXY_BASE + 5
    process = subprocess.Popen(
        [
            MITMDUMP,
            "--set", f"confdir={confdir}",
            "--set", "listen_host=127.0.0.1",
            "--set", f"listen_port={port}",
            "--set", "termlog_verbosity=warn",
            "--set", "flow_detail=0",
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    wait_port(port, timeout=15)
    deadline = time.time() + 30
    while time.time() < deadline:
        if (confdir / "mitmproxy-ca.pem").exists() and (confdir / "mitmproxy-ca-cert.pem").exists():
            break
        time.sleep(0.2)
    process.terminate()
    process.wait(timeout=10)


def verify_leaf_against_ca(
    proxy_port: int, host: str, port: int, ca_pem: pathlib.Path, attempts: int = 3
) -> tuple[bool, str]:
    """取叶子证书并对着磁盘 CA 验证，取不到就重采（最多 `attempts` 次）。

    为什么要重试：`openssl s_client` 偶尔会在握手中途拿不全证书（没有 BEGIN/END 对，
    或拿到半截）—— 那是客户端采样的抖动，不是 "CA 不一致"。
    这条断言要判的是"实例出示的叶子是不是磁盘 CA 签的"，
    所以只有"取到了完整证书却验证不过"才算失败；取不到就重采。
    """
    last = "no attempt"
    for _ in range(max(1, attempts)):
        leaf = leaf_certificate(proxy_port, host, port)
        if leaf is None:
            last = "no leaf cert (sampling hiccup, retried)"
            continue
        ok, message = verify_with_ca(leaf, ca_pem)
        if ok:
            return True, message
        last = message
    return False, last


def main() -> int:
    if WORK.exists():
        shutil.rmtree(WORK)
    WORK.mkdir(parents=True)
    # 注入器不再物化在 WORK 根上：每个实例有自己的"注入器目录"（见 materialize_agent）
    (WORK / "server.py").write_text(SERVER_SOURCE, encoding="utf-8")

    print(f"mitmdump: {run([MITMDUMP, '--version']).stdout.splitlines()[0]}")
    print(f"work dir: {WORK}\n")

    # --- 证书与本地服务 ---------------------------------------------------- #
    run([
        "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
        "-keyout", str(WORK / "key.pem"), "-out", str(WORK / "cert.pem"),
        "-days", "1", "-subj", "/CN=spike.test",
        "-addext", "subjectAltName=DNS:spike.test",
    ], cwd=WORK)
    server_log = WORK / "server.log"
    https = subprocess.Popen(
        [sys.executable, str(WORK / "server.py"), str(HTTPS_PORT), "https", str(server_log)],
        cwd=WORK, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    http = subprocess.Popen(
        [sys.executable, str(WORK / "server.py"), str(HTTP_PORT), "http", str(server_log)],
        cwd=WORK, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    wait_port(HTTPS_PORT)
    wait_port(HTTP_PORT)

    # 规则名 `spike` + 物化文件 `spike.rules`：注入器会核对软链指向的名字
    # 与 config.json 里的 `rules` 是否一致（不一致就响亮报错，不猜）。
    # other.test 也改写到同一个本地服务，但它**不**在 insecure_hosts 里 ——
    # 第 2e 步用它证明"放宽是按域名精确命中"，不是"整个环境关校验"。
    rules = WORK / "spike.rules"
    rules.write_text(
        f"127.0.0.1 spike.test\n127.0.0.1 reuse.test\n127.0.0.1 other.test\n",
        encoding="utf-8",
    )

    proxies: list[subprocess.Popen] = []
    try:
        # ================================================================== #
        # 1. 共享 CA 并发首启
        # ================================================================== #
        print("=== 1. 共享 CA 并发首启 ===")
        confdir = WORK / "conf"
        ports = [PROXY_BASE + index for index in range(4)]
        # **先物化 CA，再并发拉起** —— 这是产品契约的顺序。
        # 反过来（空 confdir 上并发首启）会让个别实例用一张已被覆盖的 CA 签叶子，
        # 那条原始竞态在第 1e 步单独**测量**，不作为 PASS/FAIL 判据。
        prematerialize_ca(confdir)
        for port in ports:  # 并发启动：先全部拉起，再一起等
            agent = materialize_agent(f"agent-ca-{port}", rules, [])
            proxies.append(start_proxy(port, confdir, agent, wait=False))
        started = [port for port in ports if wait_port(port)]
        ca_cert = confdir / "mitmproxy-ca-cert.pem"
        ca_key = confdir / "mitmproxy-ca.pem"
        check(
            "1a 预物化 CA 后四个实例都起来了，confdir 里 CA 齐备",
            len(started) == len(ports) and ca_cert.exists() and ca_key.exists(),
            f"up={started} ca={ca_cert.exists()} files={sorted(p.name for p in confdir.glob('*.pem'))}",
        )

        # 私钥与证书必须配对（并发覆盖会破坏这一点）
        key_pub = run(["openssl", "pkey", "-in", str(ca_key), "-pubout"]).stdout
        cert_pub = run(["openssl", "x509", "-in", str(ca_cert), "-pubkey", "-noout"]).stdout
        check(
            "1b CA 私钥与证书配对（并发首启没有把 CA 写坏）",
            bool(key_pub.strip()) and key_pub.strip() == cert_pub.strip(),
            f"key_pub_sha={run(['sha256sum'], input=key_pub).stdout[:12]} "
            f"cert_pub_sha={run(['sha256sum'], input=cert_pub).stdout[:12]}",
        )

        # 每个实例出示的叶子证书都必须能被**同一张**磁盘 CA 验证。
        # 这里之所以是确定性的：CA 已经在拉起实例**之前**物化好了（产品契约的顺序）。
        verified = {}
        for port in started:
            ok, message = verify_leaf_against_ca(port, "spike.test", HTTPS_PORT, ca_cert)
            verified[port] = f"{'ok' if ok else 'INVALID'} ({message})"
        all_ok = all(value.startswith("ok") for value in verified.values())
        check(
            "1c 四个实例都在用同一张 CA（预物化之后，叶子逐一通过磁盘 CA 校验）",
            all_ok,
            json.dumps(verified, ensure_ascii=False),
        )

        # 负例：把证书改坏，说明"文件都在"并不等于"CA 可用"
        broken = WORK / "broken-cert"
        broken.write_text("-----BEGIN CERTIFICATE-----\nnot-a-cert\n-----END CERTIFICATE-----\n")
        shutil.copy(broken, confdir / "mitmproxy-ca-cert.pem")
        leaf = leaf_certificate(started[0], "spike.test", HTTPS_PORT)
        ok_broken, message = verify_with_ca(leaf or "", confdir / "mitmproxy-ca-cert.pem")
        check(
            "1d 负例：证书被改坏后校验失败（证明 1c 的断言有区分力）",
            not ok_broken,
            f"verify → {message}；说明「文件齐全」不足以判定 CA 可用，就绪判据必须校验配对",
        )
        # 恢复
        shutil.copy(confdir / "mitmproxy-ca.pem", confdir / "mitmproxy-ca-cert.pem")
        for process in proxies:
            process.terminate()
        for process in proxies:
            process.wait(timeout=10)
        proxies.clear()

        # 多轮重复：一轮没触发竞态，说明不了问题不存在 —— 竞态需要特定的交错。
        #
        # 每轮同时**测量**两件事：
        #   * 磁盘上的 key/cert 是否配对（这是 PASS/FAIL 判据，逻辑上"整对写入"就成立）；
        #   * **有几个实例其实在用一张已经被覆盖的 CA 签叶子** —— 这是证据（不是判据）：
        #     它说明"预物化"不是多余动作。没有预物化时这个数会大于 0，而且**不稳定**
        #     （取决于覆盖的交错），所以拿它当 PASS/FAIL 会变成随机红灯。
        rounds = 5
        broken_rounds = 0
        mismatched_leaves = 0
        details = []
        for round_index in range(rounds):
            round_conf = WORK / f"conf-round{round_index}"
            round_cert = round_conf / "mitmproxy-ca-cert.pem"
            round_ports = [PROXY_BASE + 20 + round_index * 4 + offset for offset in range(4)]
            # 带上 spike 注入器：叶子要走同一套改写才取得到（与第 1 步的实例一致）
            processes = [
                start_proxy(
                    port,
                    round_conf,
                    materialize_agent(f"agent-round{round_index}-{port}", rules, []),
                    wait=False,
                )
                for port in round_ports
            ]
            for port in round_ports:
                wait_port(port, timeout=15)
            # 趁实例还活着采叶子：空 confdir 并发首启时，个别实例内存里的 CA 已被别人覆盖
            for port in round_ports:
                ok, _ = verify_leaf_against_ca(port, "spike.test", HTTPS_PORT, round_cert)
                if not ok:
                    mismatched_leaves += 1
            for process in processes:
                process.terminate()
            for process in processes:
                process.wait(timeout=10)
            round_key = round_conf / "mitmproxy-ca.pem"
            if not (round_key.exists() and round_cert.exists()):
                broken_rounds += 1
                details.append(f"round{round_index}: CA 文件不齊")
                continue
            key_pub = run(["openssl", "pkey", "-in", str(round_key), "-pubout"]).stdout.strip()
            cert_pub = run(["openssl", "x509", "-in", str(round_cert), "-pubkey", "-noout"]).stdout.strip()
            if not key_pub or key_pub != cert_pub:
                broken_rounds += 1
                details.append(f"round{round_index}: 私钥与证书不配对")
        check(
            f"1e 空 confdir 并发首启 {rounds} 轮 × 4 实例：CA 配对是否被写坏",
            broken_rounds == 0,
            f"broken={broken_rounds}/{rounds} {details if details else '(全部配对成功)'}"
            f"；同一批实例里有 {mismatched_leaves}/{rounds * 4} 张叶子对不上磁盘 CA"
            " —— 这就是「必须先物化 CA 再拉起实例」的实测依据（0 次也不代表竞态不存在）",
        )

        # ================================================================== #
        # 2. HTTPS 改写：SNI / Host / 上游校验（按域名放宽）
        # ================================================================== #
        print("\n=== 2. HTTPS 改写（CONNECT + server_connect + 按域名放宽校验）===")
        proxy_port = PROXY_BASE + 10
        # 严格实例：insecure_hosts 为空 = 全部走 mitmproxy 自己的严格校验
        strict_agent = materialize_agent("agent-strict", rules, [])
        proxies.append(start_proxy(proxy_port, confdir, strict_agent))
        trusted = [f"--cacert={ca_cert}"]

        strict = run([
            "curl", "-sS", "-o", "/dev/null", "-w", "%{http_code}",
            "-x", f"http://127.0.0.1:{proxy_port}",
            *trusted, f"https://spike.test:{HTTPS_PORT}/strict",
        ], timeout=25)
        check(
            "2a spike.test 不在 insecure_hosts → 严格校验 → 上游自签证书被拒 → 502",
            strict.stdout.strip() == "502",
            f"http_code={strict.stdout.strip()!r} stderr={strict.stderr.strip()[:120]!r}"
            "（改写本身生效，502 是上游链路校验失败的必然代价）",
        )

        # 用第二个实例只放行 spike.test 更干净：避免复用同一实例的配置。
        # 这也是"按域名放宽"的核心对照 —— 它只对 spike.test 放宽，不是全局关校验。
        insecure_port = PROXY_BASE + 11
        relaxed_agent = materialize_agent("agent-relaxed", rules, ["spike.test"])
        proxies.append(start_proxy(insecure_port, confdir, relaxed_agent))
        relaxed = run([
            "curl", "-sS", "-o", "/dev/null", "-w", "%{http_code}",
            "-x", f"http://127.0.0.1:{insecure_port}",
            *trusted, f"https://spike.test:{HTTPS_PORT}/relaxed",
        ], timeout=25)

        server_records = read_log(server_log)
        relaxed_record = next(
            (r for r in server_records if r["path"] == "/relaxed"), None
        )
        check(
            "2b insecure_hosts=[\"spike.test\"] → 放宽上游校验 → 200，且改写生效（请求真的到了本地服务）",
            relaxed.stdout.strip() == "200" and relaxed_record is not None,
            f"http_code={relaxed.stdout.strip()!r} server={relaxed_record}",
        )
        check(
            "2c 上游收到的 SNI 仍是原域名 spike.test（不是被改写的 127.0.0.1）",
            bool(relaxed_record) and relaxed_record.get("sni") == "spike.test",
            f"sni={relaxed_record.get('sni') if relaxed_record else None!r}",
        )
        check(
            "2d Host 头未被改写（客户端看到的一切不变）",
            bool(relaxed_record) and relaxed_record.get("host_header") == f"spike.test:{HTTPS_PORT}",
            f"host_header={relaxed_record.get('host_header') if relaxed_record else None!r}",
        )

        # 粒度对照：**同一个**放宽实例里，other.test 也被规则改写到同一个本地服务，
        # 但它不在 insecure_hosts 里 —— 必须仍然严格校验、仍然失败。
        control = run([
            "curl", "-sS", "-o", "/dev/null", "-w", "%{http_code}",
            "-x", f"http://127.0.0.1:{insecure_port}",
            *trusted, f"https://other.test:{HTTPS_PORT}/control",
        ], timeout=25)
        rewrites = [
            record for record in agent_events(relaxed_agent)
            if record["event"] == "server_connect" and record["host"] == "other.test"
        ]
        control_records = [r for r in read_log(server_log) if r["path"] == "/control"]
        check(
            "2e 粒度：other.test 同样被改写、但不在名单里 → 仍严格校验 → 502（本地服务没收到请求）",
            control.stdout.strip() == "502"
            and bool(rewrites) and rewrites[0]["rewrite"] == "127.0.0.1"
            and not control_records,
            f"http_code={control.stdout.strip()!r} "
            f"rewrite={rewrites[0]['rewrite'] if rewrites else 'no hook'} "
            f"local_hits={len(control_records)} stderr={control.stderr.strip()[:80]!r} —— "
            "同一个实例里 spike.test 200 / other.test 502，证明放宽是「按域名精确命中」",
        )

        # ================================================================== #
        # 3. 连接复用
        # ================================================================== #
        print("\n=== 3. 上游连接复用===")
        reuse_port = PROXY_BASE + 12
        reuse_agent = materialize_agent("agent-reuse", rules, [])
        proxies.append(start_proxy(reuse_port, confdir, reuse_agent))
        urls_plain = [f"http://localhost:{HTTP_PORT}/{index}" for index in range(REQUESTS)]
        urls_rewritten = [f"http://reuse.test:{HTTP_PORT}/{index}" for index in range(REQUESTS)]

        run(["curl", "-sS", "-o", "/dev/null", "-x", f"http://127.0.0.1:{reuse_port}", *urls_plain], timeout=25)
        run(["curl", "-sS", "-o", "/dev/null", "-x", f"http://127.0.0.1:{reuse_port}", *urls_rewritten], timeout=25)

        events = [r for r in agent_events(reuse_agent) if r["event"] == "server_connect"]
        plain_conns = [r for r in events if r["host"] == "localhost"]
        rewritten_conns = [r for r in events if r["host"] == "reuse.test"]
        check(
            f"3a 未被改写：{REQUESTS} 个请求复用 1 条上游连接",
            len(plain_conns) == 1,
            f"localhost server_connect={len(plain_conns)}",
        )
        check(
            f"3b 被改写：{REQUESTS} 个请求各建一条上游连接（复用丧失，符合预期）",
            len(rewritten_conns) == REQUESTS,
            f"reuse.test server_connect={len(rewritten_conns)}（写进文档的代价，不是缺陷）",
        )

        # ================================================================== #
        # 4. PR_SET_PDEATHSIG
        # ================================================================== #
        print("\n=== 4. PR_SET_PDEATHSIG（孤儿清理）===")
        parent = subprocess.run(
            [sys.executable, "-c", textwrap.dedent("""
                import ctypes, subprocess, sys, os
                libc = ctypes.CDLL("libc.so.6", use_errno=True)
                def preexec():
                    # 父进程一死，子进程收到 SIGTERM（PR_SET_PDEATHSIG = 1）
                    libc.prctl(1, 15, 0, 0, 0)
                child = subprocess.Popen(["sleep", "30"], preexec_fn=preexec)
                print(child.pid, flush=True)
            """)],
            capture_output=True, text=True,
        )
        child_pid = int(parent.stdout.strip() or 0)
        time.sleep(0.5)
        alive = pathlib.Path(f"/proc/{child_pid}").exists() if child_pid else False
        check(
            "4a 父进程退出后，带 PDEATHSIG 的子进程随之结束",
            child_pid > 0 and not alive,
            f"child pid={child_pid} alive_after_parent_exit={alive}",
        )
    finally:
        for process in proxies:
            process.terminate()
        for process in proxies:
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
        for process in (https, http):
            process.terminate()
        print("\n=== 5. Windows 侧端口可达性 ===")
        print(
            "UNVERIFIED  Windows → Linux 的连接方向测不了：本机 /etc/wsl.conf 里 "
            "[interop] enabled = false，Windows 可执行文件（netsh/cmd/curl.exe）全部跑不起来。"
        )
        print(
            "            可确认的只有反方向（Linux → Windows 共享 loopback 上独有的 135/445 "
            "可达），以及 Linux 侧绑定 16xxx 全部成功。这条记为未知。"
        )

    print("\n=== 汇总 ===")
    failed = [name for name, ok, _ in RESULTS if not ok]
    for name, ok, _ in RESULTS:
        print(f"  {'PASS' if ok else 'FAIL'}  {name}")
    print(f"\n{len(RESULTS) - len(failed)}/{len(RESULTS)} passed")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())

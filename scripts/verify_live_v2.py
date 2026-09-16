#!/usr/bin/env python3
"""v2 的实机验收（脚本化）—— 补上"M2/M3 只有文档说验过了"这个缺口。

v1 立下的规矩是"断言必须可重跑"，而 M2/M3 之前的实机结论散在验收文档与手工 curl 里。
这个脚本把能脚本化的部分固化下来，`ci/verify.sh live` 会调它：

1. **两个环境同时可用、且结果不同**（端到端一行）—— 同一个 URL 经不同
   端口出去，结果必须不同：这正是"换端口即换环境"的可证伪形式；
2. **规则热重载**：导入新规则后不重启实例即生效；
3. **安全**：Host 头校验（DNS rebinding）、变更类路由的自定义头（CSRF）；
4. **CSP 与资产形态**：无 `unsafe-inline`，页面里没有内联脚本，资产是外置文件；
5. **日志**：`--log-dir` 生效、API 能取到尾部；
6. **崩溃恢复**：SIGKILL 杀掉工作台（等价崩溃）后重启，`desired=running` 自动恢复；
7. **日志通道不会拖死代理**：连打 320 个请求（足以写满 64 KiB 管道）全部成功；
8. **实例崩溃可见且不留僵尸**：SIGKILL 掉实例后工作台不再报 running、子进程被回收、
   日志仍可读、能重新拉起；
9. **v2.1 安全增强**：dashboard `?token=` 与 header 等效（含启动日志打印可点链接）、
   `proxy_user`/`proxy_password` 下发为 mitmproxy `proxyauth`（407/200 对照）、
   对外服务开关（listen.host 0.0.0.0）真的按新地址重启；
10. **按域名放宽上游证书校验（`insecure_hosts`）**：自签上游在名单外必然 502，
    运行中把它加进名单即热生效（不重启、健康保持 running），
    同一实例里另一个被改写但未列出的域名仍旧 502。

做法上有一个关键点：规则只改**连到哪个 IP**、不改端口，所以"命中哪个上游"由客户端
请求里的端口决定。于是"同一个域名 + 两个环境各覆盖不同域名"就能构造出判别性对照。

没有 mitmdump 时整体跳过（退出码 0 并打印原因）：它属于 `live` 那一层。
"""

from __future__ import annotations

import http.client
import http.server
import json
import os
import pathlib
import re
import shutil
import signal
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
BINARY = ROOT / "target" / "debug" / "envboard"
RELOAD_INTERVAL = "1"

RESULTS: list[tuple[str, bool, str]] = []


def check(name: str, ok: bool, evidence: str) -> None:
    RESULTS.append((name, ok, evidence))
    print(f"{'PASS' if ok else 'FAIL'}  {name}\n      {evidence}")


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def wait_port(port: int, timeout: float = 30.0) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        with socket.socket() as probe:
            probe.settimeout(0.2)
            if probe.connect_ex(("127.0.0.1", port)) == 0:
                return True
        time.sleep(0.1)
    return False


class Upstream:
    """只回自己名字的极小 HTTP 服务 —— 用来判断"请求被改写到了谁那里"。

    给出 `tls_cert`（cert, key 两个路径）时在同一端口上做 HTTPS，并出示那张**自签**证书：
    这正是 `insecure_hosts` 要处理的可复现现场（上游证书不在信任库里）。
    """

    def __init__(self, name: str, tls_cert: tuple[str, str] | None = None) -> None:
        outer = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def do_GET(self) -> None:  # noqa: N802
                body = outer.name.encode()
                self.send_response(200)
                self.send_header("Content-Type", "text/plain")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *args) -> None:
                pass

        self.name = name
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        if tls_cert is not None:
            context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            context.load_cert_chain(tls_cert[0], tls_cert[1])
            self.server.socket = context.wrap_socket(self.server.socket, server_side=True)
        self.port = self.server.server_address[1]
        threading.Thread(target=self.server.serve_forever, daemon=True).start()


def self_signed_cert(work: pathlib.Path, host: str) -> tuple[str, str] | None:
    """现造一张自签证书（证书里带上 `host` 的 SAN）—— "信任库不认识它"的现场。

    只借 openssl 命令行生成文件，脚本本身仍是标准库；openssl 不可用时返回 None，
    调用方把那条用例如实记为跳过，而不是拿一个空分支冒充绿。
    """
    if shutil.which("openssl") is None:
        return None
    cert = work / f"{host}.crt"
    key = work / f"{host}.key"
    result = subprocess.run(
        ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
         "-keyout", str(key), "-out", str(cert), "-days", "1",
         "-subj", f"/CN={host}", "-addext", f"subjectAltName=DNS:{host}"],
        capture_output=True, text=True,
    )
    if result.returncode != 0 or not (cert.exists() and key.exists()):
        return None
    return str(cert), str(key)


def api(port: int, path: str, method: str = "GET", body: dict | None = None,
        host: str | None = None, csrf: bool = True, token: str | None = None,
        raw: bool = False) -> tuple[int, dict]:
    request = urllib.request.Request(f"http://127.0.0.1:{port}{path}", method=method)
    if host:
        request.add_header("Host", host)
    if method != "GET" and csrf:
        request.add_header("x-envboard-request", "1")
    if token:
        request.add_header("x-envboard-token", token)
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        request.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(request, data=data, timeout=25) as response:
            payload = response.read().decode()
            if raw:
                return response.status, {"raw": payload}
            return response.status, json.loads(payload or "{}")
    except urllib.error.HTTPError as error:
        raw_body = error.read().decode()
        try:
            return error.code, json.loads(raw_body)
        except ValueError:
            return error.code, {"raw": raw_body}


def not_covered(body: str) -> bool:
    """"这个域名没被本环境覆盖"的判据：请求失败（502 页面或空响应）。

    注意别用"响应体为空"当判据 —— 明文 HTTP 场景下 mitmproxy 会回一页 502 HTML，
    只有 HTTPS(CONNECT) 失败才是空响应。用"不是另一个上游的名字 + 是失败"更稳。
    """
    return body == "" or "502" in body or "Bad Gateway" in body


def curl_body(proxy_port: int, url: str) -> str:
    """经代理取回响应体（失败时可能是一页错误 HTML，调用方用 not_covered 判断）。"""
    result = subprocess.run(
        ["curl", "-sS", "--max-time", "15", "-x", f"http://127.0.0.1:{proxy_port}", url],
        capture_output=True, text=True,
    )
    return result.stdout.strip()


def curl_status(proxy_port: int, url: str, auth: str | None = None) -> str:
    """经代理请求，返回 HTTP 状态码（代理鉴权用例要看 407/200，不看响应体）。"""
    proxy = f"http://{auth}@127.0.0.1:{proxy_port}" if auth else f"http://127.0.0.1:{proxy_port}"
    result = subprocess.run(
        ["curl", "-sS", "-o", "/dev/null", "-w", "%{http_code}", "-x", proxy,
         "--noproxy", "", "--max-time", "15", url],
        capture_output=True, text=True,
    )
    return result.stdout.strip() or result.stderr.strip()[:40]


def curl_https(proxy_port: int, url: str) -> tuple[str, str]:
    """经代理请求 HTTPS，返回 (状态码, stderr)。

    客户端侧用 `-k`：这条用例要判的是**上游**握手（放宽前后），不是 mitmproxy
    出示给客户端的证书；让客户端侧也失败会把两种失败混在一起，读不出结论。
    """
    result = subprocess.run(
        ["curl", "-sS", "-k", "-o", "/dev/null", "-w", "%{http_code}", "-x",
         f"http://127.0.0.1:{proxy_port}", "--noproxy", "", "--max-time", "15", url],
        capture_output=True, text=True,
    )
    return result.stdout.strip(), result.stderr.strip()


def sse_status(port: int, path: str) -> int:
    """SSE 端点的状态码。**不能**用 urlopen 读 body —— SSE 流永不结束，会永远阻塞；
    这里只取响应头（状态码在 headers 里就有了）。"""
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
    try:
        connection.request("GET", path)
        return connection.getresponse().status
    finally:
        connection.close()


def main() -> int:
    if shutil.which("mitmdump") is None:
        print("SKIP live-v2: mitmdump is not on PATH (this layer needs a real host)")
        return 0
    if not BINARY.exists():
        print(f"FAIL live-v2: {BINARY} not built (run `cargo build first`)")
        return 1

    work = pathlib.Path(tempfile.mkdtemp(prefix="envboard-live2-"))
    state = work / "state"
    logs = work / "logs"
    web_port = free_port()
    servers: list[Upstream] = []
    workbench: subprocess.Popen | None = None

    def start_workbench() -> subprocess.Popen:
        # `--without-token`：token 鉴权现在是默认启用的（自动生成）。这组断言只管
        # 管理器/代理本体，显式关掉鉴权免得每个 api() 调用都要带凭据；
        # token 的默认启用与自动生成在第 11 组里单独验。
        process = subprocess.Popen(
            [str(BINARY), "--state-dir", str(state), "--core", "mitmproxy",
             "--log-dir", str(logs), "--reload-interval", RELOAD_INTERVAL,
             "web", "--listen", f"127.0.0.1:{web_port}", "--without-token"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        if not wait_port(web_port):
            raise RuntimeError("workbench did not start")
        return process

    def cli(*args: str) -> int:
        return subprocess.run([str(BINARY), "--state-dir", str(state), *args],
                              capture_output=True, text=True).returncode

    try:
        alpha = Upstream("alpha-upstream")
        beta = Upstream("beta-upstream")
        servers = [alpha, beta]

        # 规则只改 IP、不改端口：两个环境各覆盖**不同的域名**，于是
        # "同一个 URL 经不同端口出去"就有了判别性。
        (work / "alpha.rules").write_text("127.0.0.1 alpha.test\n", encoding="utf-8")
        (work / "beta.rules").write_text("127.0.0.1 beta.test\n", encoding="utf-8")
        cli("rules", "import", "alpha", "--file", str(work / "alpha.rules"))
        cli("rules", "import", "beta", "--file", str(work / "beta.rules"))
        cli("env", "add", "alpha", "--rules", "alpha")
        cli("env", "add", "beta", "--rules", "beta")

        workbench = start_workbench()
        for env in ("alpha", "beta"):
            status, body = api(web_port, f"/api/environments/{env}/start", "POST")
            assert status == 200, f"start {env} failed: {status} {body}"

        alpha_env = api(web_port, "/api/environments/alpha")[1]
        beta_env = api(web_port, "/api/environments/beta")[1]
        check(
            "1a 两个环境同时在跑、端口不同",
            alpha_env["health"] == "running" and beta_env["health"] == "running"
            and alpha_env["listen"]["port"] != beta_env["listen"]["port"],
            f"alpha={alpha_env['health']}:{alpha_env['listen']['port']} "
            f"beta={beta_env['health']}:{beta_env['listen']['port']}",
        )

        # 同一个客户端请求，换个端口就换个环境：
        alpha_url = f"http://alpha.test:{alpha.port}/"
        beta_url = f"http://beta.test:{beta.port}/"
        via_alpha_hit = curl_body(alpha_env["listen"]["port"], alpha_url)
        via_alpha_miss = curl_body(alpha_env["listen"]["port"], beta_url)
        via_beta_hit = curl_body(beta_env["listen"]["port"], beta_url)
        via_beta_miss = curl_body(beta_env["listen"]["port"], alpha_url)

        check(
            "1b alpha 端口：只覆盖 alpha.test（同一 URL 换端口结果不同）",
            via_alpha_hit == "alpha-upstream" and not_covered(via_alpha_miss),
            f"alpha.test→{via_alpha_hit!r} / beta.test→{'未覆盖(502)' if not_covered(via_alpha_miss) else via_alpha_miss!r}",
        )
        check(
            "1c beta 端口：只覆盖 beta.test（两个环境互不干扰）",
            via_beta_hit == "beta-upstream" and not_covered(via_beta_miss),
            f"beta.test→{via_beta_hit!r} / alpha.test→{'未覆盖(502)' if not_covered(via_beta_miss) else via_beta_miss!r}",
        )

        # ---- 2 规则热重载：加一条规则，不重启实例 ----
        api(web_port, "/api/rules", "POST",
            {"name": "alpha", "text": "127.0.0.1 alpha.test\n127.0.0.1 hot.test\n"})
        deadline = time.time() + 12
        hot_body = ""
        while time.time() < deadline and hot_body != "alpha-upstream":
            hot_body = curl_body(alpha_env["listen"]["port"], f"http://hot.test:{alpha.port}/")
            time.sleep(0.5)
        count = api(web_port, "/api/environments/alpha")[1]["rules_count"]
        check(
            "2 规则热重载：不重启实例即生效",
            hot_body == "alpha-upstream" and count == 2,
            f"hot.test→{hot_body!r} rules_count={count}",
        )

        # ---- 3 安全 ----
        status_code, _ = api(web_port, "/api/status", host="evil.test")
        check("3a Host 头必须是配置的监听地址（DNS rebinding）", status_code == 403, f"status={status_code}")

        status_code, _ = api(web_port, "/api/environments/beta/stop", "POST", csrf=False)
        check("3b 变更类路由必须带自定义头（CSRF）", status_code == 403, f"status={status_code}")

        status_code, _ = api(web_port, "/api/environments/beta/stop", "POST")
        check("3c 带上自定义头后变更成功", status_code == 200, f"status={status_code}")

        # ---- 4 CSP 与资产形态 ----
        with urllib.request.urlopen(f"http://127.0.0.1:{web_port}/", timeout=10) as response:
            csp = response.headers.get("content-security-policy", "")
            html = response.read().decode()
        check(
            "4a CSP 无 unsafe-inline，且脚本/样式外置（v1 的教训）",
            "unsafe-inline" not in csp and "<script>" not in html and 'src="/app.js"' in html,
            f"csp={csp[:60]}… inline_script={'<script>' in html}",
        )
        types = {}
        for asset in ("app.css", "app.js"):
            with urllib.request.urlopen(f"http://127.0.0.1:{web_port}/{asset}", timeout=10) as response:
                types[asset] = response.headers.get("content-type", "")
        check(
            "4b 资产以正确类型提供",
            "text/css" in types["app.css"] and "javascript" in types["app.js"],
            json.dumps(types),
        )

        # ---- 5 日志 ----
        log_file = logs / "alpha.log"
        _, log_body = api(web_port, "/api/environments/alpha/logs?lines=5")
        check(
            "5 --log-dir 生效，且 API 也能取到日志尾部",
            log_file.exists() and log_file.stat().st_size > 0 and bool(log_body.get("lines")),
            f"{log_file.name} size={log_file.stat().st_size if log_file.exists() else 'missing'} "
            f"api_lines={len(log_body.get('lines', []))}",
        )

        # ---- 6 跨环境静态对比 ----
        _, compare = api(web_port, "/api/compare?host=hot.test")
        rows = {row["env"]: row["covered"] for row in compare.get("environments", [])}
        check(
            "6 静态对比：hot.test 在 alpha 覆盖、在 beta 不覆盖（不发任何请求）",
            rows.get("alpha") is True and rows.get("beta") is False,
            f"coverage={rows}",
        )

        # ---- 7 崩溃恢复 ----
        desired_running = [env["name"] for env in api(web_port, "/api/environments")[1]
                           if env["desired"] == "running"]
        workbench.send_signal(signal.SIGKILL)
        workbench.wait(timeout=10)
        time.sleep(1.0)
        workbench = start_workbench()
        deadline = time.time() + 20
        recovered: list[str] = []
        while time.time() < deadline:
            recovered = [env["name"] for env in api(web_port, "/api/environments")[1]
                         if env["health"] == "running"]
            if set(desired_running) <= set(recovered):
                break
            time.sleep(1.0)
        check(
            "7 工作台被 SIGKILL 后重启：desired=running 的环境自动恢复",
            bool(desired_running) and set(desired_running) <= set(recovered),
            f"before={desired_running} recovered={recovered}",
        )

        # ---- 8 日志通道不会拖死代理 ----
        #
        # 这一条是"日志把代理卡死"的回归护栏：管道容量 64 KiB，默认详细度约 307 字节/请求，
        # 所以约 213 个请求就能写满；写满之后 mitmdump 会阻塞在自己的事件循环里，
        # 所有客户端一起挂住（实测过）。现在子进程的输出直接接文件，内核负责写盘，
        # 我们进程不在链路上 —— 打够 320 个请求必须一个不丢。
        #
        # 逐个 curl（而不是一条命令带 320 个 URL）：后者会复用同一条连接，在单线程上游前面
        # 排队，测出来的是 curl 自己的调度，不是"日志会不会卡住代理"。
        port = api(web_port, "/api/environments/alpha")[1]["listen"]["port"]
        target = f"http://alpha.test:{alpha.port}/"
        burst_total, sent, failures = 320, 0, []
        deadline = time.time() + 120
        while sent < burst_total and time.time() < deadline:
            result = subprocess.run(
                ["curl", "-sS", "-o", "/dev/null", "-w", "%{http_code}", "-x",
                 f"http://127.0.0.1:{port}", "--noproxy", "", "--max-time", "10", target],
                capture_output=True, text=True,
            )
            sent += 1
            if result.stdout.strip() != "200":
                failures.append(result.stdout.strip() or result.stderr.strip()[:60])
                break
        log_size = (logs / "alpha.log")
        log_size = log_size.stat().st_size if log_size.exists() else 0
        check(
            f"8 连打 {burst_total} 个请求（足以写满 64 KiB 管道）全部成功",
            sent == burst_total and not failures and log_size > 64 * 1024,
            f"sent={sent} failures={failures[:1]} log_bytes={log_size}（管道容量 65536）",
        )

        # ---- 9 实例崩溃能立刻看见，且不留僵尸 ----
        status_file = state / "runtime" / "alpha.status.json"
        pid = json.loads(status_file.read_text(encoding="utf-8"))["pid"]
        os.kill(pid, signal.SIGKILL)

        # 工作台必须改口：视图曾经只看"handles 表里有没有这个环境"，于是会一直报 running。
        # 采样分两段看：崩溃之后**任何一次**都不能再说 running（含糊其辞也不行）；
        # 而"为什么会死"要等回收任务写下遗言（它每 500ms 轮询一次），所以最终态必须有信号。
        deadline = time.time() + 10
        claims_running: list[str] = []
        reasons: list[str] = []
        while time.time() < deadline:
            body = api(web_port, "/api/environments/alpha")[1]
            if body["health"] == "running":
                claims_running.append(body["health"])
            else:
                reasons.append(f"{body['health']}:{body.get('health_reason')}")
                if "signal" in (body.get("health_reason") or ""):
                    break
            time.sleep(0.3)

        # 僵尸被收掉之后 /proc/<pid> 就消失了；若无人 try_wait()，条目会一直挂着（state=Z）
        reaped_deadline = time.time() + 10
        while time.time() < reaped_deadline and pathlib.Path(f"/proc/{pid}").exists():
            time.sleep(0.3)
        settled = reasons[-1] if reasons else "(never changed)"
        check(
            "9 实例被 SIGKILL 后：工作台不再报 running、说清是被信号杀死、子进程被回收",
            not claims_running
            and settled.startswith("failed:")
            and "signal" in settled
            and not pathlib.Path(f"/proc/{pid}").exists(),
            f"claims_running={len(claims_running)} settled={settled!r} "
            f"zombie={pathlib.Path(f'/proc/{pid}').exists()}",
        )

        # 崩溃现场要留着：日志仍然读得到，而且还能读到崩溃前的内容。
        _, crash_logs = api(web_port, "/api/environments/alpha/logs?lines=20")
        check(
            "9b 崩溃后日志仍可读（现场没丢）",
            bool(crash_logs.get("lines")),
            f"lines={len(crash_logs.get('lines', []))}",
        )

        # 期望状态仍是 running → 重新拉起来就该回到 running
        api(web_port, "/api/environments/alpha/start", "POST")
        check(
            "9c 崩溃的实例可以重新拉起",
            api(web_port, "/api/environments/alpha")[1]["health"] == "running",
            f"health={api(web_port, '/api/environments/alpha')[1]['health']}",
        )

        # ---- 10 编辑已建好的环境：补绑规则 + 改名换端口，然后真的按新配置干活 ----
        # 对应"建的时候没绑规则，事后改不出来"那个坑：PATCH 必须真的能用，
        # 而且改完实例要按**新配置**跑起来（不是只有列表好看）。
        api(web_port, "/api/rules", "POST",
            {"name": "edited", "text": "127.0.0.1 gamma.test\n"})
        status, created = api(web_port, "/api/environments", "POST", {"name": "edited"})
        assert status == 201, f"create edited failed: {status} {created}"
        api(web_port, "/api/environments/edited/start", "POST")
        gamma_before = curl_body(created["listen"]["port"], f"http://gamma.test:{alpha.port}/")

        # 运行中换绑定是**热**的：绑定由固定名软链承载（管理器原子换链 + 重写
        # config.json），运行中的注入器按轮询间隔跟上，实例不必重启。描述同样允许热改。
        status_bind_running, _ = api(web_port, "/api/environments/edited", "PATCH",
                                     {"rules": "edited"})
        status_desc_running, _ = api(web_port, "/api/environments/edited", "PATCH",
                                     {"description": "运行中改的描述"})
        # 不重启就等新绑定生效 —— 这一条才是"热"的可证伪形式
        deadline = time.time() + 15
        gamma_hot = ""
        while time.time() < deadline:
            gamma_hot = curl_body(created["listen"]["port"], f"http://gamma.test:{alpha.port}/")
            if gamma_hot == "alpha-upstream":
                break
            time.sleep(0.5)

        api(web_port, "/api/environments/edited/stop", "POST")
        status_edit, edited = api(web_port, "/api/environments/edited", "PATCH",
                                  {"rules": "edited", "name": "renamed", "description": "改过"})

        status_start, _ = api(web_port, "/api/environments/renamed/start", "POST")
        gamma_after = curl_body(edited["listen"]["port"], f"http://gamma.test:{alpha.port}/")
        old_status, _ = api(web_port, "/api/environments/edited")
        check(
            "10 编辑：运行中补绑规则热生效（不重启）、停止后可改名换端口、新配置真的生效",
            not_covered(gamma_before)
            and status_bind_running == 200
            and status_desc_running == 200
            and gamma_hot == "alpha-upstream"
            and status_edit == 200
            and edited["rules"] == "edited" and edited["name"] == "renamed"
            and status_start == 200
            and gamma_after == "alpha-upstream"
            and old_status == 404,
            f"before={gamma_before!r} bind_running={status_bind_running} "
            f"hot={gamma_hot!r} desc_running={status_desc_running} edit={status_edit} "
            f"after={gamma_after!r} old_name={old_status} port={edited['listen']['port']}",
        )

        # ---- 11 dashboard URL token 鉴权 ----
        # token 鉴权**默认启用**（自动生成随机值）：不给任何 token 旗标起一个工作台，
        # 必须无 token 401、带横幅里的 token 200，且横幅打印可点链接；
        # 显式 --token 与 header/?token= 等效另行验证。
        # token 工作台用 --core fake（不起实例）+ 独立 state-dir：主工作台持有状态锁。
        def banner_of(process: subprocess.Popen, needle: str) -> str:
            banner = ""
            deadline = time.time() + 5
            while time.time() < deadline and needle not in banner:
                line = process.stdout.readline()
                if not line:
                    break
                banner += line
            return banner

        auto_port = free_port()
        auto_work: subprocess.Popen | None = None
        try:
            auto_work = subprocess.Popen(
                [str(BINARY), "--state-dir", str(work / "state-auto"), "--core", "fake",
                 "web", "--listen", f"127.0.0.1:{auto_port}"],
                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
            )
            assert wait_port(auto_port), "auto-token workbench did not start"
            banner = banner_of(auto_work, "dashboard:")
            match = re.search(r"dashboard: \S*//\S*token=([0-9a-f]+)", banner)
            auto_token = match.group(1) if match else ""
            status_none, _ = api(auto_port, "/api/status", raw=True)
            status_auto, _ = api(auto_port, "/api/status", token=auto_token, raw=True)
            check(
                "11a token 默认启用：自动生成随机 token，启动日志打印可点链接",
                bool(auto_token) and len(auto_token) == 32
                and status_none == 401 and status_auto == 200,
                f"token_len={len(auto_token)} none={status_none} with_token={status_auto} "
                f"banner={'…' + banner.strip().splitlines()[-1] if banner.strip() else '(empty)'}",
            )
        finally:
            if auto_work is not None:
                auto_work.kill()
                auto_work.wait(timeout=10)

        # --without-token 在非回环监听上必须被拒绝：进程应立即带着错误退出
        deny_port = free_port()
        refused = subprocess.run(
            [str(BINARY), "--state-dir", str(work / "state-deny"), "--core", "fake",
             "web", "--listen", f"0.0.0.0:{deny_port}", "--without-token"],
            capture_output=True, text=True, timeout=30,
        )
        check(
            "11b 非回环监听拒绝 --without-token（无鉴权对外不允许）",
            refused.returncode != 0 and "web.token" in (refused.stderr + refused.stdout),
            f"exit={refused.returncode} "
            f"stderr={refused.stderr.strip().splitlines()[-1][:80] if refused.stderr.strip() else '(empty)'}",
        )

        token_port = free_port()
        token_work: subprocess.Popen | None = None
        try:
            token_work = subprocess.Popen(
                [str(BINARY), "--state-dir", str(work / "state-token"), "--core", "fake",
                 "web", "--listen", f"127.0.0.1:{token_port}",
                 "--token", "s3cret-token"],
                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
            )
            assert wait_port(token_port), "token workbench did not start"
            banner = banner_of(token_work, "dashboard:")
            status_none, _ = api(token_port, "/api/status", raw=True)
            status_wrong, _ = api(token_port, "/api/status", token="wrong", raw=True)
            status_header, _ = api(token_port, "/api/status", token="s3cret-token", raw=True)
            page_status, page_body = api(token_port, "/?token=s3cret-token", raw=True)
            sse_status_code = sse_status(token_port, "/api/events?token=s3cret-token")
            bad_page, _ = api(token_port, "/?token=nope", raw=True)
            # 子资源：浏览器解析 <link>/<script> 时带不了 header 也不会复制 ?token=，
            # 内嵌资产必须豁免（否则开 token 必白屏）；API 仍要 401。
            css_status, _ = api(token_port, "/app.css", raw=True)
            js_status, _ = api(token_port, "/app.js", raw=True)
            api_status_none, _ = api(token_port, "/api/status", raw=True)
            check(
                "11c 显式 --token：header 与 ?token= 等效，横幅打印可点链接；静态资产豁免",
                status_none == 401 and status_wrong == 401 and status_header == 200
                and page_status == 200 and sse_status_code == 200 and bad_page == 401
                and css_status == 200 and js_status == 200 and api_status_none == 401
                and "dashboard:" in banner and "token=s3cret-token" in banner,
                f"none={status_none} wrong={status_wrong} header={status_header} "
                f"page={page_status} sse={sse_status_code} bad_page={bad_page} "
                f"css={css_status} js={js_status} api_none={api_status_none} "
                f"banner={'…' + banner.strip().splitlines()[-1] if banner.strip() else '(empty)'}",
            )
            # ---- 11d 端面清点：**每一个** API 端点都必须要求 token ----
            # 这条是"新增端点忘了鉴权"的兜底。曾踩过：前端只有 mutate() 带凭据，
            # 所有 GET（日志/规则原文/对比）在开 token 后静默 401，而页面看着"只有日志坏了"。
            # 白名单只有两个内嵌静态资产（不含数据，浏览器子资源带不了凭据）。
            endpoints: list[tuple[str, str]] = [
                ("GET", "/api/status"),
                ("GET", "/api/environments"),
                ("POST", "/api/environments"),
                ("GET", "/api/environments/probe"),
                ("PATCH", "/api/environments/probe"),
                ("DELETE", "/api/environments/probe"),
                ("POST", "/api/environments/probe/start"),
                ("POST", "/api/environments/probe/stop"),
                ("POST", "/api/environments/probe/restart"),
                ("POST", "/api/environments/probe/reallocate"),
                ("GET", "/api/environments/probe/logs"),
                ("GET", "/api/rules"),
                ("POST", "/api/rules"),
                ("GET", "/api/rules/probe"),
                ("DELETE", "/api/rules/probe"),
                ("GET", "/api/compare?host=probe.test"),
                ("GET", "/api/events"),
            ]
            unauthenticated: dict[str, int] = {}
            for method, path in endpoints:
                status, _ = api(token_port, path, method, body={} if method in ("POST", "PATCH") else None,
                                csrf=False, raw=True)
                if status != 401:
                    unauthenticated[f"{method} {path}"] = status
            # 带上 token 后不能再是 401（404/409 之类的业务回答都算通过）
            authorized = sse_status(token_port, "/api/events?token=s3cret-token")
            check(
                "11d 全部 API 端点无 token 一律 401（静态资产是唯一白名单）",
                not unauthenticated and authorized == 200,
                f"leaks={unauthenticated or 'none'} sse_with_token={authorized} "
                f"endpoints={len(endpoints)}",
            )

            # 单独覆盖 `?token=` 的非 SSE 用法（GET 与变更类都要认）
            logs_via_query, _ = api(token_port, "/api/status?token=s3cret-token", raw=True)
            logs_no_query, _ = api(token_port, "/api/status", raw=True)
            check(
                "11e ?token= 对普通 GET 同样有效",
                logs_via_query == 200 and logs_no_query == 401,
                f"with_query={logs_via_query} without={logs_no_query}",
            )

        finally:
            if token_work is not None:
                token_work.kill()
                token_work.wait(timeout=10)


        # ---- 12 代理访问鉴权（proxy_user / proxy_password → mitmproxy proxyauth）----
        # 承接测试 10：edited 已改名 renamed 且在跑。两个凭据字段是启动时读取的：
        # 运行中改被拒（409），停止后改、重启才生效；视图只回一个布尔，永不回显取值。
        status_auth_running_early, _ = api(web_port, "/api/environments/renamed", "PATCH",
                                           {"proxy_user": "alice", "proxy_password": "live-pass"})
        api(web_port, "/api/environments/renamed/stop", "POST")
        status_auth_patch, authed = api(web_port, "/api/environments/renamed", "PATCH",
                                        {"proxy_user": "alice", "proxy_password": "live-pass"})
        status_auth_start, _ = api(web_port, "/api/environments/renamed/start", "POST")
        auth_port = authed["listen"]["port"]
        auth_url = f"http://gamma.test:{alpha.port}/"
        deadline = time.time() + 15
        denied = ""
        while time.time() < deadline:
            denied = curl_status(auth_port, auth_url)
            if denied == "407":
                break
            time.sleep(0.5)
        granted = curl_status(auth_port, auth_url, auth="alice:live-pass")
        status_auth_running, _ = api(web_port, "/api/environments/renamed", "PATCH",
                                     {"proxy_user": "bob", "proxy_password": "other"})
        authed_json = json.dumps(authed)
        check(
            "12 代理鉴权：无凭据 407、带凭据 200、运行中改 proxy_user/proxy_password 被拒、"
            "视图不回显凭据",
            status_auth_running_early == 409
            and status_auth_patch == 200 and authed["proxy_auth_enabled"] is True
            # 只断言**取值**不回显：`proxy_auth_enabled` 本身就含 `proxy_auth` 子串，
            # 拿键名当判据会误报。
            and "live-pass" not in authed_json and "alice" not in authed_json
            and denied == "407" and granted == "200" and status_auth_running == 409,
            f"early={status_auth_running_early} patch={status_auth_patch} denied={denied} "
            f"granted={granted} running_patch={status_auth_running} "
            f"enabled={authed['proxy_auth_enabled']}",
        )

        # ---- 13 对外服务开关：listen.host 0.0.0.0 ----
        # 通配绑定在本机测不出"外部可达"，但可以证伪两件事：
        # host 真的换成了 0.0.0.0（不是只有开关好看），且回环方向照常服务。
        # listen 是身份字段：运行中改会被拒，先停止。
        api(web_port, "/api/environments/renamed/stop", "POST")
        status_host_patch, exposed = api(web_port, "/api/environments/renamed", "PATCH",
                                         {"listen": {"host": "0.0.0.0",
                                                     "port": authed["listen"]["port"]}})
        status_start_wild, _ = api(web_port, "/api/environments/renamed/start", "POST")
        deadline = time.time() + 15
        loopback_ok = ""
        wild_port = exposed["listen"]["port"]
        while time.time() < deadline:
            loopback_ok = curl_status(wild_port, f"http://gamma.test:{alpha.port}/",
                                      auth="alice:live-pass")
            if loopback_ok == "200":
                break
            time.sleep(0.5)
        _, wild_view = api(web_port, "/api/environments/renamed")
        check(
            "13 对外服务开关：listen.host 换成 0.0.0.0 并按新地址重启，回环方向照常服务",
            status_host_patch == 200 and exposed["listen"]["host"] == "0.0.0.0"
            and status_start_wild == 200 and wild_view["listen"]["host"] == "0.0.0.0"
            and wild_view["health"] == "running" and loopback_ok == "200",
            f"patch={status_host_patch} start={status_start_wild} "
            f"host={wild_view['listen']['host']} health={wild_view['health']} "
            f"loopback={loopback_ok}",
        )

        # ---- 14 insecure_hosts：按域名放宽上游证书校验（运行中热生效）----
        # 端到端判别性对照：规则把两个域名都改写到同一个**自签** HTTPS 上游，
        # 只有列进 insecure_hosts 的那个能通，另一个必须仍然 502。
        tls_cert = self_signed_cert(work, "relaxed.test")
        if tls_cert is None:
            print("SKIP 14 insecure_hosts: openssl is not available "
                  "(cannot stand up a self-signed upstream)")
        else:
            tls_upstream = Upstream("tls-upstream", tls_cert=tls_cert)
            servers.append(tls_upstream)
            api(web_port, "/api/rules", "POST",
                {"name": "tlsdemo",
                 "text": "127.0.0.1 relaxed.test\n127.0.0.1 strict.test\n"})
            status_tls_create, created_tls = api(web_port, "/api/environments", "POST",
                                                 {"name": "tlsdemo", "rules": "tlsdemo"})
            status_tls_start, _ = api(web_port, "/api/environments/tlsdemo/start", "POST")
            tls_proxy_port = created_tls["listen"]["port"]
            relaxed_url = f"https://relaxed.test:{tls_upstream.port}/"
            strict_url = f"https://strict.test:{tls_upstream.port}/"

            # 1) 名单为空：改写生效，但上游证书不在信任库里 → 严格校验 → 502
            before, _ = curl_https(tls_proxy_port, relaxed_url)
            health_before = api(web_port, "/api/environments/tlsdemo")[1]["health"]

            # 2) **运行中**把域名加进名单：不重启、不 stop/start，等注入器轮询到新配置
            status_hot, listed = api(web_port, "/api/environments/tlsdemo", "PATCH",
                                     {"insecure_hosts": ["relaxed.test"]})
            deadline = time.time() + 20
            after = ""
            while time.time() < deadline:
                after, _ = curl_https(tls_proxy_port, relaxed_url)
                if after == "200":
                    break
                time.sleep(0.5)
            health_after = api(web_port, "/api/environments/tlsdemo")[1]["health"]

            # 3) 同一个实例：同样被改写、但**没**列进名单的域名仍然严格 → 502
            control_tls, _ = curl_https(tls_proxy_port, strict_url)
            check(
                "14 insecure_hosts 端到端：名单外严格校验 502 → 运行中加名单热生效 200"
                "（健康始终 running）→ 同实例未列出的域名仍 502",
                status_tls_create == 201 and status_tls_start == 200
                and before == "502"
                and status_hot == 200 and listed["insecure_hosts"] == ["relaxed.test"]
                and health_before == "running" and health_after == "running"
                and after == "200" and control_tls == "502",
                f"create={status_tls_create} start={status_tls_start} before={before!r} "
                f"hot_patch={status_hot} listed={listed['insecure_hosts']} "
                f"health={health_before}→{health_after} after={after!r} "
                f"control={control_tls!r} port={tls_proxy_port} "
                f"upstream={tls_upstream.name}:{tls_upstream.port}",
            )
    finally:
        if workbench is not None:
            workbench.kill()
            workbench.wait(timeout=10)
        for server in servers:
            server.server.shutdown()
        shutil.rmtree(work, ignore_errors=True)

    print("\n=== 汇总 ===")
    failed = [name for name, ok, _ in RESULTS if not ok]
    for name, ok, _ in RESULTS:
        print(f"  {'PASS' if ok else 'FAIL'}  {name}")
    print(f"\n{len(RESULTS) - len(failed)}/{len(RESULTS)} passed")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())

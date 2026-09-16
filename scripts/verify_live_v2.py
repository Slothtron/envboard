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
   日志仍可读、能重新拉起。

做法上有一个关键点：规则只改**连到哪个 IP**、不改端口，所以"命中哪个上游"由客户端
请求里的端口决定。于是"同一个域名 + 两个环境各覆盖不同域名"就能构造出判别性对照。

没有 mitmdump 时整体跳过（退出码 0 并打印原因）：它属于 `live` 那一层。
"""

from __future__ import annotations

import http.server
import json
import os
import pathlib
import shutil
import signal
import socket
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
    """只回自己名字的极小 HTTP 服务 —— 用来判断"请求被改写到了谁那里"。"""

    def __init__(self, name: str) -> None:
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
        self.port = self.server.server_address[1]
        threading.Thread(target=self.server.serve_forever, daemon=True).start()


def api(port: int, path: str, method: str = "GET", body: dict | None = None,
        host: str | None = None, csrf: bool = True) -> tuple[int, dict]:
    request = urllib.request.Request(f"http://127.0.0.1:{port}{path}", method=method)
    if host:
        request.add_header("Host", host)
    if method != "GET" and csrf:
        request.add_header("x-envboard-request", "1")
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        request.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(request, data=data, timeout=25) as response:
            return response.status, json.loads(response.read().decode() or "{}")
    except urllib.error.HTTPError as error:
        raw = error.read().decode()
        try:
            return error.code, json.loads(raw)
        except ValueError:
            return error.code, {"raw": raw}


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
        process = subprocess.Popen(
            [str(BINARY), "--state-dir", str(state), "--core", "mitmproxy",
             "--log-dir", str(logs), "--reload-interval", RELOAD_INTERVAL,
             "web", "--listen", f"127.0.0.1:{web_port}"],
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

        # 运行中换绑定：服务端必须拒绝（实例在启动时才固定规则路径，热改绑定只会造成
        # "配置说绑了、实例没按它干"的不一致）。描述则允许热改。
        status_bind_running, _ = api(web_port, "/api/environments/edited", "PATCH",
                                     {"rules": "edited"})
        status_desc_running, _ = api(web_port, "/api/environments/edited", "PATCH",
                                     {"description": "运行中改的描述"})

        api(web_port, "/api/environments/edited/stop", "POST")
        status_edit, edited = api(web_port, "/api/environments/edited", "PATCH",
                                  {"rules": "edited", "name": "renamed", "description": "改过"})

        status_start, _ = api(web_port, "/api/environments/renamed/start", "POST")
        gamma_after = curl_body(edited["listen"]["port"], f"http://gamma.test:{alpha.port}/")
        old_status, _ = api(web_port, "/api/environments/edited")
        check(
            "10 编辑：运行中换绑定被拒、停止后可补绑规则并改名换端口、新配置真的生效",
            not_covered(gamma_before)
            and status_bind_running == 409
            and status_desc_running == 200
            and status_edit == 200
            and edited["rules"] == "edited" and edited["name"] == "renamed"
            and status_start == 200
            and gamma_after == "alpha-upstream"
            and old_status == 404,
            f"before={gamma_before!r} bind_running={status_bind_running} "
            f"desc_running={status_desc_running} edit={status_edit} after={gamma_after!r} "
            f"old_name={old_status} port={edited['listen']['port']}",
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

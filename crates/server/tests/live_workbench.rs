//! 实机端到端（v3）：真进程、真 TLS 引擎、真管理器。
//!
//! 这一层回答的是"脚本化不了的实机行为"，14 组断言：
//!
//! 1. **两个环境同时可用、且结果不同** —— 同一个 URL 经不同端口出去结果必须不同，
//!    这正是"换端口即换环境"的可证伪形式；
//! 2. **规则热重载**：导入新规则后不重启实例即生效；
//! 3. **安全**：`Host` 头校验（DNS rebinding）、变更类路由的自定义头（CSRF）；
//! 4. **CSP 与资产形态**：无 `unsafe-inline`、页面无内联脚本、资产外置；
//! 5. **日志**：`--log-dir` 生效、API 能取到尾部；
//! 6. **跨环境静态对比**：某域名在哪个环境被覆盖（不发请求）；
//! 7. **崩溃恢复**：SIGKILL 工作台后重启，`desired=running` 的环境自动恢复；
//! 8. **日志通道不会拖死代理**：连打 320 个请求全部成功；
//! 9. **实例崩溃可见且不占资源**：注入 failed（等价"引擎线程死了"的可观察形态）
//!    后不再报 running、原因可见、监听端口真的释放；
//! 10. **编辑已建环境**：运行中换绑定**热生效**（v3 = 一次同步装配，不重启实例）、
//!     停止后可改名换端口、新配置真的生效；
//! 11. **dashboard token 鉴权档位**：回环默认免鉴权（组 1-10 全程即此形态）、裸
//!     `--token` 自动生成、显式 `--token` 下 header 与 `?token=` 等效、静态资产豁免、
//!     **启用后全部 API 端点无 token 一律 401**、非回环不给 token 拒绝启动；
//! 12. **代理鉴权**：一等字段下发（无凭据 407、带凭据 200）；12b **凭据 argv 审计**
//!     （/proc 全量 cmdline 不得出现明文密码 —— v2 靠脱敏契约兜底，v3 结构性成立）；
//! 13. **对外服务开关**：`listen.host` 换 `0.0.0.0` 并按新地址重启，回环方向照常服务；
//! 14. **按域名放宽上游证书校验（`insecure_hosts`）**：自签上游在名单外必然 502，
//!     运行中把它加进名单即热生效（不重启、健康保持 running），同一实例里另一个
//!     被改写但未列出的域名仍旧 502；14b **热生效时延**：PATCH 返回后的**第一次**
//!     请求即已生效（v2 要等 reload 轮询，v3 装配是同步的 —— 计时留证据）；
//! 15. **既有 CA 零感知兼容**：confdir 预放 mitmproxy 形状的 CA（PKCS#1 私钥+证书
//!     拼接），引擎必须**原样加载**（文件逐字节不变、不重新生成），客户端以该 CA
//!     校验 MITM 证书链成功 —— 已装证书用户升级零感证的实机形式。
//!
//! 做法上有一个关键点：规则只改**连到哪个 IP**、不改端口，所以"命中哪个上游"由客户端
//! 请求里的端口决定 —— 于是"同一域名 + 两个环境各覆盖不同域名"就构造出了判别性对照。
//!
//! **为什么标 `#[ignore]`**：它需要真宿主工具与真网络（真进程起停）。默认的
//! `cargo test --workspace` 因此不需要宿主；`ci/verify.sh live` 用 `--ignored` 显式触发。
//! 显式要跑这一层时宿主缺失**响亮失败**（不静默跳过 —— 跳过等于这些断言消失）。
//!
//! 跑法：`bash ci/verify.sh live`（或本测试加 `--ignored` 直跑）

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// 管道容量 64 KiB、默认详细度约 307 字节/请求 —— 320 个足以写满。
const BURST_TOTAL: usize = 320;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_envboard")
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

// --------------------------------------------------------------------------- //
// 断言收集（沿用旧脚本的 PASS/FAIL 逐条清单与末尾汇总）
// --------------------------------------------------------------------------- //

struct Checks {
    results: Vec<(String, bool, String)>,
}

impl Checks {
    fn new() -> Self {
        Checks {
            results: Vec::new(),
        }
    }

    fn record(&mut self, name: &str, ok: bool, evidence: impl Into<String>) {
        let evidence = evidence.into();
        println!(
            "{}  {name}\n      {evidence}",
            if ok { "PASS" } else { "FAIL" }
        );
        self.results.push((name.to_string(), ok, evidence));
    }

    /// 打印汇总；有任何一条失败就 panic（红），并把断言数报清楚。
    fn finish(self) {
        println!("\n=== 汇总 ===");
        let mut failed = Vec::new();
        for (name, ok, _) in &self.results {
            println!("  {}  {name}", if *ok { "PASS" } else { "FAIL" });
            if !ok {
                failed.push(name.clone());
            }
        }
        let total = self.results.len();
        let passed = total - failed.len();
        println!("\n{passed}/{total} passed");
        if !failed.is_empty() {
            panic!("live-workbench FAILED: {passed}/{total} passed; 失败：{failed:?}");
        }
    }
}

// --------------------------------------------------------------------------- //
// 极小的 HTTP 客户端（不引 HTTP 库：门禁不该为这点用途多背一个依赖）
// --------------------------------------------------------------------------- //

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Response {
    fn header(&self, name: &str) -> String {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    }
}

fn parse_response(raw: &[u8]) -> Response {
    let text = String::from_utf8_lossy(raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .collect();
    Response {
        status,
        headers,
        body: body.to_string(),
    }
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

fn wait_port(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().expect("addr"),
            Duration::from_millis(200),
        )
        .is_ok()
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// 管理面 API 调用。`host` 用来构造 DNS rebinding 场景。
fn api(
    port: u16,
    method: &str,
    path: &str,
    host: Option<&str>,
    csrf: bool,
    token: Option<&str>,
    body: Option<&str>,
) -> Response {
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
        host.unwrap_or(&format!("127.0.0.1:{port}"))
    );
    if method != "GET" && csrf {
        request.push_str("x-envboard-request: 1\r\n");
    }
    if let Some(token) = token {
        request.push_str(&format!("x-envboard-token: {token}\r\n"));
    }
    if let Some(body) = body {
        request.push_str("Content-Type: application/json\r\n");
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
        request.push_str("\r\n");
        request.push_str(body);
    } else {
        request.push_str("\r\n");
    }

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to the workbench");
    stream
        .set_read_timeout(Some(Duration::from_secs(25)))
        .expect("read timeout");
    stream.write_all(request.as_bytes()).expect("write request");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read response");
    parse_response(&raw)
}

/// 只要状态码（SSE 的响应体永不结束，**不能**读到底）。
/// token 放在 **URL query** 里的 GET —— 浏览器直接打开工作台时唯一可用的携带
/// 方式，与 api() 的 header 通道互补（两条都要有断言：query 端到端曾是盲区）。
/// 铺设（CLI 退役后唯一正途）：导入规则走 POST /api/rules。
fn seed_rule(port: u16, name: &str, text: &str) {
    let body = serde_json::json!({"name": name, "text": text}).to_string();
    let response = api(port, "POST", "/api/rules", None, true, None, Some(&body));
    assert!(
        response.status == 201 || response.status == 200,
        "规则 {name} 导入失败：{} {}",
        response.status,
        response.body
    );
}

/// 铺设：建环境（端口自动分配）走 POST /api/environments。
fn seed_env(port: u16, name: &str, rules: &str) {
    let body = serde_json::json!({"name": name, "rules": rules}).to_string();
    let response = api(
        port,
        "POST",
        "/api/environments",
        None,
        true,
        None,
        Some(&body),
    );
    assert!(
        response.status == 201 || response.status == 200,
        "环境 {name} 创建失败：{} {}",
        response.status,
        response.body
    );
}

fn api_query(port: u16, path: &str, token: &str) -> u16 {
    let sep = if path.contains('?') { '&' } else { '?' };
    let request = format!(
        "GET {path}{sep}token={token} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n",
    );
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to the workbench");
    stream
        .set_read_timeout(Some(Duration::from_secs(25)))
        .expect("set read timeout");
    stream.write_all(request.as_bytes()).expect("send request");
    stream.flush().ok();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).ok();
    let text = String::from_utf8_lossy(&raw).into_owned();
    text.split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0)
}

fn status_only(port: u16, path: &str) -> u16 {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .expect("write");
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let _ = reader.read_line(&mut line);
    parse_response(line.as_bytes()).status
}

fn json(response: &Response) -> serde_json::Value {
    serde_json::from_str(&response.body).unwrap_or(serde_json::Value::Null)
}

// --------------------------------------------------------------------------- //
// 经代理的请求：裸 TCP 直连（不 spawn curl）
// --------------------------------------------------------------------------- //

fn base64(input: &str) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = input.as_bytes();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// 经代理取响应（绝对 URI 形式）。
fn proxy_get(proxy_port: u16, url: &str, auth: Option<&str>) -> Response {
    let host = url
        .strip_prefix("http://")
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
        .to_string();
    let mut request =
        format!("GET {url} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nAccept: */*\r\n");
    if let Some(auth) = auth {
        request.push_str(&format!("Proxy-Authorization: Basic {}\r\n", base64(auth)));
    }
    request.push_str("\r\n");

    let mut stream = TcpStream::connect(("127.0.0.1", proxy_port)).expect("connect to the proxy");
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .expect("read timeout");
    stream.write_all(request.as_bytes()).expect("write");
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw);
    parse_response(&raw)
}

/// "这个域名没被本环境覆盖"的判据：请求失败（502 页面或空响应）。
///
/// 注意别用"响应体为空"当判据 —— 明文 HTTP 场景下代理会回一页 502 文本，
/// 只有 HTTPS（CONNECT）失败才是空响应。用"不是另一个上游的名字 + 是失败"更稳。
fn not_covered(body: &str) -> bool {
    body.is_empty() || body.contains("502") || body.contains("Bad Gateway")
}

// --------------------------------------------------------------------------- //
// 上游替身：只回自己的名字，用来判断"请求被改写到了谁那里"
// --------------------------------------------------------------------------- //

fn start_upstream(name: &'static str) -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind upstream");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buffer = [0u8; 4096];
            let _ = stream.read(&mut buffer);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                name.len(),
                name
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    port
}

/// 自签证书的 HTTPS 上游 —— `insecure_hosts` 要处理的可复现现场（上游证书不在信任库里）。
///
/// 现造一张带 SAN 的自签证书，再用 `openssl s_server -www` 在同一端口上做 HTTPS。
/// 用一个进程同时干"发证书"和"当上游"两件事：既省一个宿主工具，也不必把私钥入库
/// （`.gitignore` 本来就不收 `*.key` / `*.pem`）。
fn start_tls_upstream(work: &Path, host: &str) -> (u16, Child) {
    let cert = work.join(format!("{host}.crt"));
    let key = work.join(format!("{host}.key"));
    let generated = Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            key.to_str().expect("key path"),
            "-out",
            cert.to_str().expect("cert path"),
            "-days",
            "1",
            "-subj",
            &format!("/CN={host}"),
            "-addext",
            &format!("subjectAltName=DNS:{host}"),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run openssl req");
    assert!(generated.success(), "自签证书没造出来（openssl req 失败）");

    let port = free_port();
    let child = Command::new("openssl")
        .args([
            "s_server",
            "-accept",
            &port.to_string(),
            "-cert",
            cert.to_str().expect("cert path"),
            "-key",
            key.to_str().expect("key path"),
            "-www",
            "-quiet",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn openssl s_server");
    assert!(
        wait_port(port, Duration::from_secs(10)),
        "自签 HTTPS 上游没起来"
    );
    (port, child)
}

/// 经代理请求 HTTPS，客户端用指定 CA 校验（不带 -k）：链验不过就是失败。
fn curl_with_ca(proxy_port: u16, url: &str, cacert: &std::path::Path) -> u16 {
    let output = Command::new("curl")
        .args([
            "-sS",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "-x",
            &format!("http://127.0.0.1:{proxy_port}"),
            "--cacert",
            cacert.to_str().expect("cacert path"),
            "--noproxy",
            "",
            "--max-time",
            "20",
            url,
        ])
        .output()
        .expect("run curl --cacert");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

/// 经代理请求 **HTTPS** 并返回状态码。
///
/// 只有这一条路径用 curl：经代理访问 HTTPS 要先 `CONNECT` 再在隧道里做 TLS 握手，
/// 而 TLS 客户端不在标准库里 —— 自造一个等于重写 curl。明文请求仍然全部走裸 TCP
/// （见 [`proxy_get`]），320 次连打那条断言因此还是零进程开销。
///
/// 客户端侧带 `-k`：这条用例判的是**上游**握手（放宽前后），不是 mitmproxy 出示给
/// 客户端的证书；让客户端侧也失败会把两种失败混在一起，读不出结论。
fn curl_https_status(proxy_port: u16, url: &str) -> u16 {
    let output = Command::new("curl")
        .args([
            "-sS",
            "-k",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "-x",
            &format!("http://127.0.0.1:{proxy_port}"),
            "--noproxy",
            "",
            "--max-time",
            "20",
            url,
        ])
        .output()
        .expect("run curl");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

// --------------------------------------------------------------------------- //
// 工作台进程
// --------------------------------------------------------------------------- //

struct Workbench {
    child: Child,
}

impl Workbench {
    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 读子进程 stdout 直到出现 `needle`（或超时）。
fn banner_until(mut child: Child, needle: &str, timeout: Duration) -> (Child, String) {
    let stdout = child.stdout.take().expect("piped stdout");
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                break;
            }
        }
    });

    let mut banner = String::new();
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline && !banner.contains(needle) {
        match receiver.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                banner.push_str(&line);
                banner.push('\n');
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    (child, banner)
}

fn spawn_workbench(state: &Path, extra: &[&str]) -> Child {
    let mut command = Command::new(binary());
    command
        .arg("--state-dir")
        .arg(state)
        .args(extra)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.spawn().expect("spawn the workbench")
}

#[test]
#[ignore = "需要真宿主工具（openssl / curl）与真网络；由 ci/verify.sh 的 live 层用 --ignored 触发"]
fn the_workbench_behaves_on_a_real_host() {
    let missing: Vec<&str> = ["openssl", "curl"]
        .into_iter()
        .filter(|tool| which(tool).is_none())
        .collect();
    if !missing.is_empty() {
        panic!(
            "live 层需要真宿主工具 {missing:?}，但 PATH 上没有全部。\\n\
             openssl 现造自签 CA/上游证书并充当自签 HTTPS 上游（第 14、15 组的
             可复现现场），curl 只用于经代理的 HTTPS 请求 —— TLS 客户端不在
             标准库里，明文请求全部走裸 TCP。两条出路：装上缺的工具，或不跑
             这一层（cargo test --workspace 不含本测试）。"
        );
    }

    let work = std::env::temp_dir().join(format!("envboard-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let state = work.join("state");
    let logs = work.join("logs");
    std::fs::create_dir_all(&state).expect("state dir");
    std::fs::create_dir_all(&logs).expect("log dir");

    let web_port = free_port();
    let alpha_upstream = start_upstream("alpha-upstream");
    let beta_upstream = start_upstream("beta-upstream");

    let mut checks = Checks::new();

    // 规则只改 IP、不改端口：两个环境各覆盖**不同域名**，于是"同一 URL 换端口"有判别性。

    // 回环默认免鉴权：这组断言只管管理器/代理本体，所有 api() 不带凭据直用；
    // 鉴权档位（显式/裸给/非回环拒启）在第 11 组单独验。
    let mut workbench = Workbench {
        child: spawn_workbench(
            &state,
            &[
                "--log-dir",
                logs.to_str().unwrap(),
                "--listen",
                &format!("127.0.0.1:{web_port}"),
            ],
        ),
    };
    assert!(
        wait_port(web_port, Duration::from_secs(30)),
        "工作台没起来（端口 {web_port}）"
    );

    // 铺设改走 HTTP API（CLI 退役后脚本与工作台同一入口）：先规则、再环境。
    seed_rule(
        web_port,
        "alpha",
        "127.0.0.1 alpha.test
",
    );
    seed_rule(
        web_port,
        "beta",
        "127.0.0.1 beta.test
",
    );
    seed_env(web_port, "alpha", "alpha");
    seed_env(web_port, "beta", "beta");

    for env in ["alpha", "beta"] {
        let response = api(
            web_port,
            "POST",
            &format!("/api/environments/{env}/start"),
            None,
            true,
            None,
            None,
        );
        assert_eq!(response.status, 200, "start {env} 失败：{}", response.body);
    }

    // ---- 1 两个环境同时在跑、且结果不同 ----
    let alpha_env = json(&api(
        web_port,
        "GET",
        "/api/environments/alpha",
        None,
        true,
        None,
        None,
    ));
    let beta_env = json(&api(
        web_port,
        "GET",
        "/api/environments/beta",
        None,
        true,
        None,
        None,
    ));
    let alpha_proxy = alpha_env["listen"]["port"].as_u64().unwrap_or(0) as u16;
    let beta_proxy = beta_env["listen"]["port"].as_u64().unwrap_or(0) as u16;
    checks.record(
        "1a 两个环境同时在跑、端口不同",
        alpha_env["health"] == "running"
            && beta_env["health"] == "running"
            && alpha_proxy != beta_proxy,
        format!(
            "alpha={}:{alpha_proxy} beta={}:{beta_proxy}",
            alpha_env["health"], beta_env["health"]
        ),
    );

    let alpha_url = format!("http://alpha.test:{alpha_upstream}/");
    let beta_url = format!("http://beta.test:{beta_upstream}/");
    let via_alpha_hit = proxy_get(alpha_proxy, &alpha_url, None).body;
    let via_alpha_miss = proxy_get(alpha_proxy, &beta_url, None).body;
    let via_beta_hit = proxy_get(beta_proxy, &beta_url, None).body;
    let via_beta_miss = proxy_get(beta_proxy, &alpha_url, None).body;

    checks.record(
        "1b alpha 端口：只覆盖 alpha.test（同一 URL 换端口结果不同）",
        via_alpha_hit == "alpha-upstream" && not_covered(&via_alpha_miss),
        format!(
            "alpha.test→{via_alpha_hit:?} / beta.test→{}",
            if not_covered(&via_alpha_miss) {
                "未覆盖(502)".to_string()
            } else {
                via_alpha_miss.clone()
            }
        ),
    );
    checks.record(
        "1c beta 端口：只覆盖 beta.test（两个环境互不干扰）",
        via_beta_hit == "beta-upstream" && not_covered(&via_beta_miss),
        format!(
            "beta.test→{via_beta_hit:?} / alpha.test→{}",
            if not_covered(&via_beta_miss) {
                "未覆盖(502)".to_string()
            } else {
                via_beta_miss.clone()
            }
        ),
    );

    // ---- 2 规则热更新：加一条规则，不重启实例 ----
    // v3 的热是一次同步 apply：POST 返回即生效，下面的宽限循环通常首轮命中。
    api(
        web_port,
        "POST",
        "/api/rules",
        None,
        true,
        None,
        Some(r#"{"name":"alpha","text":"127.0.0.1 alpha.test\n127.0.0.1 hot.test\n"}"#),
    );
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut hot_body = String::new();
    while Instant::now() < deadline && hot_body != "alpha-upstream" {
        hot_body = proxy_get(
            alpha_proxy,
            &format!("http://hot.test:{alpha_upstream}/"),
            None,
        )
        .body;
        std::thread::sleep(Duration::from_millis(500));
    }
    let count = json(&api(
        web_port,
        "GET",
        "/api/environments/alpha",
        None,
        true,
        None,
        None,
    ))["rules_count"]
        .as_u64()
        .unwrap_or(0);
    checks.record(
        "2 规则热重载：不重启实例即生效",
        hot_body == "alpha-upstream" && count == 2,
        format!("hot.test→{hot_body:?} rules_count={count}"),
    );

    // ---- 3 安全 ----
    let host_checked = api(
        web_port,
        "GET",
        "/api/status",
        Some("evil.test"),
        true,
        None,
        None,
    );
    checks.record(
        "3a Host 头必须是配置的监听地址（DNS rebinding）",
        host_checked.status == 403,
        format!("status={}", host_checked.status),
    );

    let no_csrf = api(
        web_port,
        "POST",
        "/api/environments/beta/stop",
        None,
        false,
        None,
        None,
    );
    checks.record(
        "3b 变更类路由必须带自定义头（CSRF）",
        no_csrf.status == 403,
        format!("status={}", no_csrf.status),
    );

    let with_csrf = api(
        web_port,
        "POST",
        "/api/environments/beta/stop",
        None,
        true,
        None,
        None,
    );
    checks.record(
        "3c 带上自定义头后变更成功",
        with_csrf.status == 200,
        format!("status={}", with_csrf.status),
    );

    // ---- 4 CSP 与资产形态 ----
    let index = api(web_port, "GET", "/", None, true, None, None);
    let csp = index.header("content-security-policy");
    checks.record(
        "4a CSP 无 unsafe-inline，且脚本/样式外置（v1 的教训）",
        !csp.contains("unsafe-inline")
            && !index.body.contains("<script>")
            && index.body.contains("src=\"/app.js\""),
        format!(
            "csp={}… inline_script={}",
            &csp[..csp.len().min(60)],
            index.body.contains("<script>")
        ),
    );
    let css = api(web_port, "GET", "/app.css", None, true, None, None);
    let js = api(web_port, "GET", "/app.js", None, true, None, None);
    checks.record(
        "4b 资产以正确类型提供",
        css.header("content-type").contains("text/css")
            && js.header("content-type").contains("javascript"),
        format!(
            "css={} js={}",
            css.header("content-type"),
            js.header("content-type")
        ),
    );

    // ---- 5 日志 ----
    let log_file = logs.join("alpha.log");
    let log_body = json(&api(
        web_port,
        "GET",
        "/api/environments/alpha/logs?lines=400",
        None,
        true,
        None,
        None,
    ));
    let size = std::fs::metadata(&log_file)
        .map(|meta| meta.len())
        .unwrap_or(0);
    let lines = log_body["lines"].as_array().map(Vec::len).unwrap_or(0);
    checks.record(
        "5 --log-dir 生效，且 API 也能取到日志尾部",
        log_file.exists() && size > 0 && lines > 0,
        format!("alpha.log size={size} api_lines={lines}"),
    );

    // ---- 6 跨环境静态对比 ----
    let compare = json(&api(
        web_port,
        "GET",
        "/api/compare?host=hot.test",
        None,
        true,
        None,
        None,
    ));
    let mut coverage = Vec::new();
    for row in compare["environments"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        coverage.push(format!(
            "{}={}",
            row["env"].as_str().unwrap_or("?"),
            row["covered"]
        ));
    }
    checks.record(
        "6 静态对比：hot.test 在 alpha 覆盖、在 beta 不覆盖（不发任何请求）",
        compare["environments"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .any(|row| row["env"] == "alpha" && row["covered"] == true)
                    && rows
                        .iter()
                        .any(|row| row["env"] == "beta" && row["covered"] == false)
            })
            .unwrap_or(false),
        format!("coverage=[{}]", coverage.join(" ")),
    );

    // ---- 7 崩溃恢复 ----
    let desired_running: Vec<String> = json(&api(
        web_port,
        "GET",
        "/api/environments",
        None,
        true,
        None,
        None,
    ))
    .as_array()
    .cloned()
    .unwrap_or_default()
    .iter()
    .filter(|env| env["desired"] == "running")
    .map(|env| env["name"].as_str().unwrap_or_default().to_string())
    .collect();

    workbench.child.kill().expect("SIGKILL the workbench");
    let _ = workbench.child.wait();
    std::thread::sleep(Duration::from_millis(1000));

    workbench = Workbench {
        child: spawn_workbench(
            &state,
            &[
                "--log-dir",
                logs.to_str().unwrap(),
                "--listen",
                &format!("127.0.0.1:{web_port}"),
            ],
        ),
    };
    assert!(
        wait_port(web_port, Duration::from_secs(30)),
        "工作台没能重启"
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut recovered: Vec<String> = Vec::new();
    while Instant::now() < deadline {
        recovered = json(&api(
            web_port,
            "GET",
            "/api/environments",
            None,
            true,
            None,
            None,
        ))
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|env| env["health"] == "running")
        .map(|env| env["name"].as_str().unwrap_or_default().to_string())
        .collect();
        if desired_running.iter().all(|name| recovered.contains(name)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
    checks.record(
        "7 工作台被 SIGKILL 后重启：desired=running 的环境自动恢复",
        !desired_running.is_empty() && desired_running.iter().all(|name| recovered.contains(name)),
        format!("before={desired_running:?} recovered={recovered:?}"),
    );

    // ---- 8 日志通道不会拖死代理 ----
    //
    // 这一条是"日志把代理卡死"的回归护栏：管道容量 64 KiB、默认详细度约 307 字节/请求，
    // 所以约 213 个请求就能写满；写满之后代理会阻塞在写日志上，所有客户端
    // 一起挂住（实测过）。v3 的形态：投递进有界总线立即返回，满了丢弃并计数 ——
    // 数据面与日志面彻底解耦（丢弃量在 EngineReport.log_drops 可见）。
    //
    // 逐个请求（而不是一条连接复用到底）：后者测出来的是客户端的调度，不是"日志会不会
    // 卡住代理"。裸 TCP 直连同时省掉了 320 次 curl spawn，也不受 *_proxy 环境变量干扰。
    let alpha_proxy = json(&api(
        web_port,
        "GET",
        "/api/environments/alpha",
        None,
        true,
        None,
        None,
    ))["listen"]["port"]
        .as_u64()
        .unwrap_or(0) as u16;
    let target = format!("http://alpha.test:{alpha_upstream}/");
    let mut sent = 0usize;
    let mut failures: Vec<String> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(120);
    while sent < BURST_TOTAL && Instant::now() < deadline {
        let response = proxy_get(alpha_proxy, &target, None);
        sent += 1;
        if response.status != 200 {
            failures.push(format!(
                "{} {}",
                response.status,
                response.body.chars().take(60).collect::<String>()
            ));
            break;
        }
    }
    let log_size = std::fs::metadata(&log_file)
        .map(|meta| meta.len())
        .unwrap_or(0);
    // v3 判据：全部成功 + 日志确实落盘（行数与请求数对得上）。总线的有界性由
    // log_drops 报告面负责；"写满 64 KiB 管道"这一 v2 介质判据随管道一起退场。
    let logged_lines = std::fs::read_to_string(&log_file)
        .map(|text| text.lines().count())
        .unwrap_or(0);
    checks.record(
        &format!("8 连打 {BURST_TOTAL} 个请求全部成功，且日志逐条落盘"),
        sent == BURST_TOTAL && failures.is_empty() && log_size > 0 && logged_lines + 20 >= sent,
        format!("sent={sent} failures={failures:?} log_bytes={log_size} log_lines={logged_lines}"),
    );

    // ---- 9 故障注入：实例崩溃的可见性与资源释放 ----
    // v2 这条读状态文件拿 pid、SIGKILL 真子进程、盯 /proc 等僵尸回收；v3 实例
    // 在进程内，"线程死了"的可观察形态由注入旋钮给出（真引擎、真进程、真端口）。
    let fault = api(
        web_port,
        "POST",
        "/api/_fault",
        None,
        true,
        None,
        Some(r#"{"env":"alpha","reason":"live fault injection"}"#),
    );
    assert_eq!(
        fault.status, 200,
        "fault injection must be accepted: {}",
        fault.body
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut claims_running = 0usize;
    let mut settled = String::from("(never changed)");
    while Instant::now() < deadline {
        let body = json(&api(
            web_port,
            "GET",
            "/api/environments/alpha",
            None,
            true,
            None,
            None,
        ));
        if body["health"] == "running" {
            claims_running += 1;
        } else {
            settled = format!(
                "{}:{}",
                body["health"].as_str().unwrap_or("?"),
                body["health_reason"].as_str().unwrap_or("")
            );
            if settled.contains("injected") {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }

    // 僵尸回收的 v3 等价判据：端口**真的**空出来了 —— 还能在原端口 bind 起新
    // 监听，才说明没有隐性的占着不放。
    let rebind = TcpListener::bind(("127.0.0.1", alpha_proxy));
    checks.record(
        "9 注入 failed 后：不再报 running、原因可见、监听端口真的释放",
        claims_running == 0
            && settled.starts_with("failed:")
            && settled.contains("injected")
            && rebind.is_ok(),
        format!(
            "claims_running={claims_running} settled={settled:?} rebind_ok={}",
            rebind.is_ok()
        ),
    );

    let crash_logs = json(&api(
        web_port,
        "GET",
        "/api/environments/alpha/logs?lines=20",
        None,
        true,
        None,
        None,
    ));
    checks.record(
        "9b 崩溃后日志仍可读（现场没丢）",
        crash_logs["lines"].as_array().map(Vec::len).unwrap_or(0) > 0,
        format!(
            "lines={}",
            crash_logs["lines"].as_array().map(Vec::len).unwrap_or(0)
        ),
    );

    api(
        web_port,
        "POST",
        "/api/environments/alpha/start",
        None,
        true,
        None,
        None,
    );
    let health = json(&api(
        web_port,
        "GET",
        "/api/environments/alpha",
        None,
        true,
        None,
        None,
    ))["health"]
        .as_str()
        .unwrap_or("?")
        .to_string();
    checks.record(
        "9c 崩溃的实例可以重新拉起",
        health == "running",
        format!("health={health}"),
    );

    // ---- 10 编辑已建好的环境：补绑规则 + 改名换端口，然后真的按新配置干活 ----
    api(
        web_port,
        "POST",
        "/api/rules",
        None,
        true,
        None,
        Some(r#"{"name":"edited","text":"127.0.0.1 gamma.test\n"}"#),
    );
    let created = api(
        web_port,
        "POST",
        "/api/environments",
        None,
        true,
        None,
        Some(r#"{"name":"edited"}"#),
    );
    assert_eq!(created.status, 201, "create edited 失败：{}", created.body);
    let created = json(&created);
    let created_port = created["listen"]["port"].as_u64().unwrap_or(0) as u16;
    api(
        web_port,
        "POST",
        "/api/environments/edited/start",
        None,
        true,
        None,
        None,
    );
    let gamma_url = format!("http://gamma.test:{alpha_upstream}/");
    let gamma_before = proxy_get(created_port, &gamma_url, None).body;

    // 运行中换绑定是**热**的：v3 是一次同步装配（apply），POST 返回即生效，实例不必重启。
    // 描述同样允许热改。
    let bind_running = api(
        web_port,
        "PATCH",
        "/api/environments/edited",
        None,
        true,
        None,
        Some(r#"{"rules":"edited"}"#),
    );
    let desc_running = api(
        web_port,
        "PATCH",
        "/api/environments/edited",
        None,
        true,
        None,
        Some(r#"{"description":"运行中改的描述"}"#),
    );

    // 不重启就等新绑定生效 —— 这一条才是"热"的可证伪形式
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut gamma_hot = String::new();
    while Instant::now() < deadline {
        gamma_hot = proxy_get(created_port, &gamma_url, None).body;
        if gamma_hot == "alpha-upstream" {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    api(
        web_port,
        "POST",
        "/api/environments/edited/stop",
        None,
        true,
        None,
        None,
    );
    let edited = api(
        web_port,
        "PATCH",
        "/api/environments/edited",
        None,
        true,
        None,
        Some(r#"{"rules":"edited","name":"renamed","description":"改过"}"#),
    );
    let edited = json(&edited);
    let edited_port = edited["listen"]["port"].as_u64().unwrap_or(0) as u16;
    let start_renamed = api(
        web_port,
        "POST",
        "/api/environments/renamed/start",
        None,
        true,
        None,
        None,
    );
    let gamma_after = proxy_get(edited_port, &gamma_url, None).body;
    let old_status = api(
        web_port,
        "GET",
        "/api/environments/edited",
        None,
        true,
        None,
        None,
    )
    .status;
    checks.record(
        "10 编辑：运行中补绑规则热生效（不重启）、停止后可改名换端口、新配置真的生效",
        not_covered(&gamma_before)
            && bind_running.status == 200
            && desc_running.status == 200
            && gamma_hot == "alpha-upstream"
            && edited["rules"] == "edited"
            && edited["name"] == "renamed"
            && start_renamed.status == 200
            && gamma_after == "alpha-upstream"
            && old_status == 404,
        format!(
            "before={gamma_before:?} bind_running={} hot={gamma_hot:?} desc_running={} edit={} after={gamma_after:?} old_name={old_status} port={edited_port}",
            bind_running.status, desc_running.status, edited["name"].as_str().unwrap_or("?")
        ),
    );

    // ---- 11 dashboard token 鉴权档位 ----
    // 11a：回环默认免鉴权（主工作台全程没带过凭据，这里如实钉死）。
    let open_api = api(web_port, "GET", "/api/status", None, true, None, None).status;
    let open_page = api(web_port, "GET", "/", None, true, None, None).status;
    checks.record(
        "11a 回环默认免鉴权：页面与 API 不带凭据一律 200",
        open_api == 200 && open_page == 200,
        format!("api={open_api} page={open_page}"),
    );

    // 11a2：裸 `--token`（不给值）自动生成：无 token 401、横幅可点链接、双通道 200。
    let auto_port = free_port();
    let auto_child = Command::new(binary())
        .arg("--state-dir")
        .arg(work.join("state-auto"))
        .args(["--listen", &format!("127.0.0.1:{auto_port}"), "--token"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the auto-token workbench");
    assert!(
        wait_port(auto_port, Duration::from_secs(30)),
        "auto-token 工作台没起来"
    );
    let (mut auto_child, banner) = banner_until(auto_child, "dashboard:", Duration::from_secs(5));
    let auto_token = banner
        .split("token=")
        .nth(1)
        .map(|rest| {
            rest.chars()
                .take_while(|c| c.is_ascii_hexdigit())
                .collect::<String>()
        })
        .unwrap_or_default();
    let none_status = api(auto_port, "GET", "/api/status", None, true, None, None).status;
    let query_status = api_query(auto_port, "/api/status", &auto_token);
    let auto_status = api(
        auto_port,
        "GET",
        "/api/status",
        None,
        true,
        Some(&auto_token),
        None,
    )
    .status;
    checks.record(
        "11a2 裸 --token：自动生成随机 token，启动日志打印可点链接",
        !auto_token.is_empty()
            && auto_token.len() == 32
            && none_status == 401
            && auto_status == 200
            && query_status == 200,
        format!(
            "token_len={} none={none_status} header={auto_status} query={query_status}",
            auto_token.len()
        ),
    );
    auto_child.kill().expect("kill auto-token workbench");
    let _ = auto_child.wait();

    // 非回环监听没给 token 必须被拒：进程应立即带着 invalid_config 退出。
    let deny = Command::new(binary())
        .arg("--state-dir")
        .arg(work.join("state-deny"))
        .args(["--listen", &format!("0.0.0.0:{}", free_port())])
        .output()
        .expect("run a public bind without a token");
    let deny_output = format!(
        "{}{}",
        String::from_utf8_lossy(&deny.stdout),
        String::from_utf8_lossy(&deny.stderr)
    );
    checks.record(
        "11b 非回环无 token 拒绝启动（对外暴露必须显式 --token）",
        !deny.status.success() && deny_output.contains("web.token"),
        format!("exit={:?}", deny.status.code()),
    );

    let token_port = free_port();
    let token_child = Command::new(binary())
        .arg("--state-dir")
        .arg(work.join("state-token"))
        .args([
            "--listen",
            &format!("127.0.0.1:{token_port}"),
            "--token",
            "s3cret-token",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the token workbench");
    assert!(
        wait_port(token_port, Duration::from_secs(30)),
        "token 工作台没起来"
    );
    let (mut token_child, token_banner) =
        banner_until(token_child, "dashboard:", Duration::from_secs(5));

    let s_none = api(token_port, "GET", "/api/status", None, true, None, None).status;
    let s_wrong = api(
        token_port,
        "GET",
        "/api/status",
        None,
        true,
        Some("wrong"),
        None,
    )
    .status;
    let s_header = api(
        token_port,
        "GET",
        "/api/status",
        None,
        true,
        Some("s3cret-token"),
        None,
    )
    .status;
    let s_page = api(
        token_port,
        "GET",
        "/?token=s3cret-token",
        None,
        true,
        None,
        None,
    )
    .status;
    let s_sse = status_only(token_port, "/api/events?token=s3cret-token");
    let s_bad_page = api(token_port, "GET", "/?token=nope", None, true, None, None).status;
    // 子资源：浏览器解析 <link>/<script> 时带不了 header 也不会复制 ?token=，
    // 内嵌资产必须豁免（否则开 token 必白屏）；API 仍要 401。
    let s_css = api(token_port, "GET", "/app.css", None, true, None, None).status;
    let s_js = api(token_port, "GET", "/app.js", None, true, None, None).status;
    let s_api_none = api(token_port, "GET", "/api/status", None, true, None, None).status;
    checks.record(
        "11c 显式 --token：header 与 ?token= 等效，横幅打印可点链接；静态资产豁免",
        s_none == 401
            && s_wrong == 401
            && s_header == 200
            && s_page == 200
            && s_sse == 200
            && s_bad_page == 401
            && s_css == 200
            && s_js == 200
            && s_api_none == 401
            && token_banner.contains("dashboard:")
            && token_banner.contains("token=s3cret-token"),
        format!(
            "none={s_none} wrong={s_wrong} header={s_header} page={s_page} sse={s_sse} \
             bad_page={s_bad_page} css={s_css} js={s_js} api_none={s_api_none}"
        ),
    );

    // ---- 11d 端面清点：**每一个** API 端点都必须要求 token ----
    // 这条是"新增端点忘了鉴权"的兜底。曾踩过：前端只有 mutate() 带凭据，所有 GET
    // （日志/规则原文/对比）在开 token 后静默 401，而页面看着"只有日志坏了"。
    // 白名单只有两个内嵌静态资产（不含数据，浏览器子资源带不了凭据）。
    let endpoints: [(&str, &str); 21] = [
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
        ("POST", "/api/_fault"),
        // 设置页「证书」三件套：摘要 / 下载 / 二维码，同样必须过鉴权。
        ("GET", "/api/ca"),
        ("GET", "/api/ca.pem"),
        ("GET", "/api/ca/qrcode.svg?data=http://127.0.0.1/x"),
    ];
    let mut leaks: Vec<String> = Vec::new();
    for (method, path) in endpoints {
        let body = if method == "GET" { None } else { Some("{}") };
        let response = api(token_port, method, path, None, false, None, body);
        if response.status != 401 {
            leaks.push(format!("{method} {path} → {}", response.status));
        }
    }
    let authorized = status_only(token_port, "/api/events?token=s3cret-token");
    checks.record(
        "11d 全部 API 端点无 token 一律 401（静态资产是唯一白名单）",
        leaks.is_empty() && authorized == 200,
        format!(
            "leaks={} sse_with_token={authorized} endpoints={}",
            if leaks.is_empty() {
                "none".to_string()
            } else {
                leaks.join(", ")
            },
            endpoints.len()
        ),
    );

    // 单独覆盖 `?token=` 的非 SSE 用法（GET 与变更类都要认）。
    let with_query = api(
        token_port,
        "GET",
        "/api/status?token=s3cret-token",
        None,
        true,
        None,
        None,
    )
    .status;
    let without_query = api(token_port, "GET", "/api/status", None, true, None, None).status;
    checks.record(
        "11e ?token= 对普通 GET 同样有效",
        with_query == 200 && without_query == 401,
        format!("with_query={with_query} without={without_query}"),
    );
    token_child.kill().expect("kill token workbench");
    let _ = token_child.wait();

    // ---- 12 代理访问鉴权（proxy_user / proxy_password → mitmproxy proxyauth）----
    // 承接测试 10：edited 已改名 renamed 且在跑。两个凭据字段都是启动时读取的：
    // 运行中改被拒（409），停止后改、重启才生效；视图只回一个布尔，永不回显取值。
    let auth_running_early = api(
        web_port,
        "PATCH",
        "/api/environments/renamed",
        None,
        true,
        None,
        Some(r#"{"proxy_user":"alice","proxy_password":"live-pass"}"#),
    )
    .status;
    api(
        web_port,
        "POST",
        "/api/environments/renamed/stop",
        None,
        true,
        None,
        None,
    );
    let auth_patched = api(
        web_port,
        "PATCH",
        "/api/environments/renamed",
        None,
        true,
        None,
        Some(r#"{"proxy_user":"alice","proxy_password":"live-pass"}"#),
    );
    let authed = json(&auth_patched);
    api(
        web_port,
        "POST",
        "/api/environments/renamed/start",
        None,
        true,
        None,
        None,
    );
    let auth_port = authed["listen"]["port"].as_u64().unwrap_or(0) as u16;
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut denied = 0u16;
    while Instant::now() < deadline {
        denied = proxy_get(auth_port, &gamma_url, None).status;
        if denied == 407 {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let granted = proxy_get(auth_port, &gamma_url, Some("alice:live-pass")).status;
    let auth_running = api(
        web_port,
        "PATCH",
        "/api/environments/renamed",
        None,
        true,
        None,
        Some(r#"{"proxy_user":"bob","proxy_password":"other"}"#),
    )
    .status;
    checks.record(
        "12 代理鉴权：无凭据 407、带凭据 200、运行中改凭据被拒、视图不回显凭据取值",
        auth_running_early == 409
            && auth_patched.status == 200
            && authed["proxy_auth_enabled"] == true
            // 只断言**取值**不回显：`proxy_auth_enabled` 这个键名本身就含 `proxy_auth`
            // 子串，拿键名当判据会误报。
            && !auth_patched.body.contains("live-pass")
            && !auth_patched.body.contains("alice")
            && denied == 407
            && granted == 200
            && auth_running == 409,
        format!(
            "early={auth_running_early} patch={} denied={denied} granted={granted} running_patch={auth_running} enabled={}",
            auth_patched.status, authed["proxy_auth_enabled"].as_bool().unwrap_or(false)
        ),
    );

    // ---- 12b 凭据 argv 审计：明文密码不得出现在任何进程的 cmdline ----
    // v2 的凭据经启动参数下发，靠记录前脱敏的契约兜底；v3 凭据只在内存比对 ——
    // 结构性成立，这里用全系统扫描把它钉死。
    let mut argv_leaks: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let Ok(raw) = std::fs::read(entry.path().join("cmdline")) else {
                continue;
            };
            let text = String::from_utf8_lossy(&raw).replace("\0", " ");
            if text.contains("live-pass") {
                argv_leaks.push(name);
            }
        }
    }
    checks.record(
        "12b 凭据不进 argv：全系统 /proc cmdline 扫描不得出现明文密码",
        argv_leaks.is_empty(),
        format!("leaks={argv_leaks:?}"),
    );

    // ---- 13 对外服务开关：listen.host 0.0.0.0 ----
    // 通配绑定在本机测不出"外部可达"，但可以证伪两件事：host 真的换成了 0.0.0.0
    // （不是只有开关好看），且回环方向照常服务。listen 是身份字段：运行中改会被拒，先停止。
    api(
        web_port,
        "POST",
        "/api/environments/renamed/stop",
        None,
        true,
        None,
        None,
    );
    let host_patched = api(
        web_port,
        "PATCH",
        "/api/environments/renamed",
        None,
        true,
        None,
        Some(&format!(
            r#"{{"listen":{{"host":"0.0.0.0","port":{auth_port}}}}}"#
        )),
    );
    let exposed = json(&host_patched);
    let start_wild = api(
        web_port,
        "POST",
        "/api/environments/renamed/start",
        None,
        true,
        None,
        None,
    );
    let wild_port = exposed["listen"]["port"].as_u64().unwrap_or(0) as u16;
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut loopback = 0u16;
    while Instant::now() < deadline {
        loopback = proxy_get(wild_port, &gamma_url, Some("alice:live-pass")).status;
        if loopback == 200 {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let wild_view = json(&api(
        web_port,
        "GET",
        "/api/environments/renamed",
        None,
        true,
        None,
        None,
    ));
    checks.record(
        "13 对外服务开关：listen.host 换成 0.0.0.0 并按新地址重启，回环方向照常服务",
        host_patched.status == 200
            && exposed["listen"]["host"] == "0.0.0.0"
            && start_wild.status == 200
            && wild_view["listen"]["host"] == "0.0.0.0"
            && wild_view["health"] == "running"
            && loopback == 200,
        format!(
            "patch={} start={} host={} health={} loopback={loopback}",
            host_patched.status,
            start_wild.status,
            wild_view["listen"]["host"],
            wild_view["health"]
        ),
    );

    // ---- 14 insecure_hosts：按域名放宽上游证书校验（运行中热生效）----
    // 端到端判别性对照：规则把两个域名都改写到同一个**自签** HTTPS 上游，
    // 只有列进 insecure_hosts 的那个能通，另一个必须仍然 502。
    let (tls_port, mut tls_upstream) = start_tls_upstream(&work, "relaxed.test");
    // body 用 serde_json 构造：这段规则文本里需要**真正的换行**，手写转义容易出错
    // （踩过：把 `\n` 写成实字符会让 JSON 非法，导入静默失败，直到"绑定失败"才暴露）。
    let rules_body = serde_json::json!({
        "name": "tlsdemo",
        "text": "127.0.0.1 relaxed.test\n127.0.0.1 strict.test\n",
    })
    .to_string();
    let rules_post = api(
        web_port,
        "POST",
        "/api/rules",
        None,
        true,
        None,
        Some(&rules_body),
    );
    let created_tls = api(
        web_port,
        "POST",
        "/api/environments",
        None,
        true,
        None,
        Some(r#"{"name":"tlsdemo","rules":"tlsdemo"}"#),
    );
    let tls_create = created_tls.status;
    let tls_proxy = json(&created_tls)["listen"]["port"].as_u64().unwrap_or(0) as u16;
    let started_tls = api(
        web_port,
        "POST",
        "/api/environments/tlsdemo/start",
        None,
        true,
        None,
        None,
    )
    .status;
    let relaxed_url = format!("https://relaxed.test:{tls_port}/");
    let strict_url = format!("https://strict.test:{tls_port}/");

    // 1) 名单为空：改写生效，但上游证书不在信任库里 → 严格校验 → 502
    let before = curl_https_status(tls_proxy, &relaxed_url);
    let health_before = json(&api(
        web_port,
        "GET",
        "/api/environments/tlsdemo",
        None,
        true,
        None,
        None,
    ))["health"]
        .as_str()
        .unwrap_or("?")
        .to_string();

    // 2) **运行中**把域名加进名单：不重启、不 stop/start，等注入器轮询到新配置
    let hot_patch = api(
        web_port,
        "PATCH",
        "/api/environments/tlsdemo",
        None,
        true,
        None,
        Some(r#"{"insecure_hosts":["relaxed.test"]}"#),
    );
    let listed = json(&hot_patch);
    // v3 装配同步：PATCH 已返回，第一次请求就应命中新名单（轮询只是宽限保险）。
    let first_started = Instant::now();
    let first_after_patch = curl_https_status(tls_proxy, &relaxed_url);
    let first_latency_ms = first_started.elapsed().as_millis();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut after = first_after_patch;
    while after != 200 && Instant::now() < deadline {
        after = curl_https_status(tls_proxy, &relaxed_url);
        if after == 200 {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let health_after = json(&api(
        web_port,
        "GET",
        "/api/environments/tlsdemo",
        None,
        true,
        None,
        None,
    ))["health"]
        .as_str()
        .unwrap_or("?")
        .to_string();

    // 3) 同一个实例：同样被改写、但**没**列进名单的域名仍然严格 → 502
    let control = curl_https_status(tls_proxy, &strict_url);
    checks.record(
        "14 insecure_hosts 端到端：名单外严格校验 502 → 运行中加名单热生效 200（健康始终 running）→ 同一实例未列出的域名仍 502",
        tls_create == 201
            && started_tls == 200
            && before == 502
            && hot_patch.status == 200
            && listed["insecure_hosts"] == serde_json::json!(["relaxed.test"])
            && health_before == "running"
            && health_after == "running"
            && after == 200
            && control == 502,
        format!(
            "rules_import={} create={tls_create} start={started_tls} before={before} hot_patch={} listed={:?} \
             health={health_before}→{health_after} after={after} control={control} port={tls_proxy} upstream={tls_port}",
            rules_post.status, hot_patch.status, listed["insecure_hosts"]
        ),
    );
    checks.record(
        "14b 热生效时延：PATCH 返回后的第一次请求即已生效（装配同步，不等轮询）",
        first_after_patch == 200 && first_latency_ms < 3000,
        format!("first_try={first_after_patch} latency_ms={first_latency_ms}"),
    );

    // ---- 15 预放既有 CA：引擎原样加载，已装证书客户端零感知 ----
    // confdir 预先放一张 mitmproxy 形状的 CA（-ca.pem = 私钥+证书拼接、
    // -ca-cert.pem = 客户端装的那张）。引擎必须**加载复用**而不是重新生成：
    // 文件逐字节不变 + 客户端仅凭这张 CA 校验 MITM 链成功，两面同时成立才算数。
    let state_ca = work.join("state-ca");
    let confdir = state_ca.join("shared").join("confdir");
    std::fs::create_dir_all(&confdir).expect("confdir");
    let ca_key = work.join("preca.key");
    let ca_crt = work.join("preca.crt");
    let genrsa = Command::new("openssl")
        .args(["genrsa", "-out", ca_key.to_str().unwrap(), "2048"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("openssl genrsa");
    assert!(genrsa.success(), "CA 私钥生成失败");
    let req = Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-new",
            "-key",
            ca_key.to_str().unwrap(),
            "-out",
            ca_crt.to_str().unwrap(),
            "-days",
            "825",
            "-subj",
            "/O=mitmproxy/CN=envboard-live-ca",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-addext",
            "keyUsage=critical,keyCertSign,cRLSign",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("openssl req");
    assert!(req.success(), "CA 证书生成失败");
    let key_pem = std::fs::read_to_string(&ca_key).unwrap();
    let crt_pem = std::fs::read_to_string(&ca_crt).unwrap();
    let bundle = format!("{key_pem}{crt_pem}");
    std::fs::write(confdir.join("mitmproxy-ca.pem"), &bundle).unwrap();
    std::fs::write(confdir.join("mitmproxy-ca-cert.pem"), &crt_pem).unwrap();
    let web_ca = free_port();
    let mut wb_ca = Workbench {
        child: spawn_workbench(&state_ca, &["--listen", &format!("127.0.0.1:{web_ca}")]),
    };
    assert!(
        wait_port(web_ca, Duration::from_secs(30)),
        "CA 兼容工作台没起来"
    );
    seed_rule(
        web_ca,
        "ca",
        "127.0.0.1 ca.test
",
    );
    seed_env(web_ca, "caenv", "ca");
    api(
        web_ca,
        "POST",
        "/api/environments/caenv/start",
        None,
        true,
        None,
        None,
    );
    let (tls_ca, mut up_ca) = start_tls_upstream(&work, "ca.test");
    api(
        web_ca,
        "PATCH",
        "/api/environments/caenv",
        None,
        true,
        None,
        Some(r#"{"insecure_hosts":["ca.test"]}"#),
    );
    let ca_view = json(&api(
        web_ca,
        "GET",
        "/api/environments/caenv",
        None,
        true,
        None,
        None,
    ));
    let ca_proxy = ca_view["listen"]["port"].as_u64().unwrap_or(0) as u16;
    let code_ca = curl_with_ca(
        ca_proxy,
        &format!("https://ca.test:{tls_ca}/"),
        &confdir.join("mitmproxy-ca-cert.pem"),
    );
    let unchanged =
        std::fs::read(confdir.join("mitmproxy-ca.pem")).unwrap_or_default() == bundle.as_bytes();
    checks.record(
        "15 预放既有 CA：引擎原样加载（confdir 逐字节不变），客户端仅凭该 CA 校验 MITM 链成功",
        code_ca == 200 && unchanged && ca_view["health"] == "running",
        format!(
            "cacert_code={code_ca} confdir_unchanged={unchanged} health={}",
            ca_view["health"].as_str().unwrap_or("?")
        ),
    );
    up_ca.kill().ok();
    let _ = up_ca.wait();
    wb_ca.kill();
    let _ = wb_ca.child.wait();
    let _ = std::fs::remove_dir_all(&state_ca);

    api(
        web_port,
        "POST",
        "/api/environments/tlsdemo/stop",
        None,
        true,
        None,
        None,
    );
    tls_upstream.kill().expect("kill the self-signed upstream");
    let _ = tls_upstream.wait();

    workbench.kill();
    let _ = std::fs::remove_dir_all(&work);

    checks.finish();
}

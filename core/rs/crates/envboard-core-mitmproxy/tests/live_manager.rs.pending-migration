//! M2 的实机验收：**真管理器 + 真 mitmdump + 真改写**。
//!
//! 它回答的问题与 M0.5 spike 不同：spike 证明"机制成立"，这里证明"经管理器的编排
//! 之后仍然成立" —— 参数是管理器拼的、CA 是管理器预物化的、注入器是管理器物化的、
//! 健康判定是管理器做的。任何一环错位（比如选项名拼错被静默忽略），这里就该红。
//!
//! 没有 mitmdump 时**跳过**（不是失败）：它属于 `ci/verify.sh live` 那一层，
//! 不该让"没装 mitmproxy 的机器"跑不了默认流水线。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use envboard_core_api::{ClockPort, LoggerPort, NullLogger, ProxyCore};
use envboard_core_mitmproxy::{MitmproxyCore, MitmproxyCoreConfig};
use envboard_manager::infra::{ProcfsProcessTable, RealFiles, SocketPortProbe, SystemClock};
use envboard_manager::{JsonFileStateRepo, Manager, ManagerConfig, StateRepo};

const RULES_TEXT: &str = "127.0.0.1 live.test\n";

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn curl_code(proxy_port: u16, url: &str) -> String {
    curl_code_with_ca(proxy_port, url, None)
}

/// 经代理发一个请求，返回 HTTP 状态码。
///
/// `cacert` 给的是**客户端**要信任的 CA（mitmproxy 自己的那张），与上游校验无关 ——
/// 上游那条腿是否校验由注入器的 `insecure_hosts` 决定。
fn curl_code_with_ca(proxy_port: u16, url: &str, cacert: Option<&Path>) -> String {
    let mut command = Command::new("curl");
    command.args([
        "-sS",
        "-o",
        "/dev/null",
        "-w",
        "%{http_code}",
        "--max-time",
        "20",
        "-x",
    ]);
    command.arg(format!("http://127.0.0.1:{proxy_port}"));
    if let Some(cacert) = cacert {
        command.arg(format!("--cacert={}", cacert.display()));
    }
    let output = command
        .arg(url)
        .output()
        .expect("curl must be available for the live test");
    let code = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() {
        // 实机测试最容易卡在"curl 什么都没拿到"，把 stderr 打出来比只看 000 有用得多。
        eprintln!(
            "[live] curl {url} exited {} (code={code}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    code
}

/// 轮询直到拿到期望的状态码（热重载是异步的：注入器按自己的节奏轮询）。
async fn wait_for_code(proxy_port: u16, url: &str, cacert: &Path, expected: &str) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut last = String::new();
    while std::time::Instant::now() < deadline {
        last = curl_code_with_ca(proxy_port, url, Some(cacert));
        if last == expected {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    last
}

/// 生成一张**自签**证书（issuer == subject、私钥不在任何信任库里）。
///
/// 这就是"上游校验必然失败"的最小复现：mitmproxy 的信任库是自带的 certifi，
/// 不认识这张 CA，于是严格校验下握手会以 `unable to get local issuer certificate` 失败。
fn make_self_signed_cert(dir: &Path, names: &[&str]) -> Option<(PathBuf, PathBuf)> {
    let key = dir.join("upstream-key.pem");
    let cert = dir.join("upstream-cert.pem");
    let san = names
        .iter()
        .map(|name| format!("DNS:{name}"))
        .collect::<Vec<_>>()
        .join(",");
    let output = Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "1",
            "-subj",
            &format!("/CN={}", names.first().copied().unwrap_or("upstream.test")),
            "-addext",
            &format!("subjectAltName={san}"),
            "-keyout",
        ])
        .arg(&key)
        .arg("-out")
        .arg(&cert)
        .output()
        .ok()?;
    (output.status.success() && cert.exists() && key.exists()).then_some((cert, key))
}

/// 一个**只回 `ok` 的极小 HTTPS 上游**（自签证书）。
///
/// 用解释器跑一个 `http.server`：注入器/管理器需要的 TLS 语义都在标准库里，
/// 不必为此引一个 Rust TLS 依赖（那会破坏"离线可构建 + 依赖克制"的纪律）。
fn start_tls_upstream(port: u16, cert: &Path, key: &Path) -> Option<std::process::Child> {
    // 把夹具自己的输出留着：它起不来时，"为什么"只能从这里看。
    let log = std::fs::File::create(dir_of(cert).join("tls-upstream.log")).ok()?;
    let log_err = log.try_clone().ok()?;
    Command::new("python3")
        .arg("-c")
        .arg(TLS_UPSTREAM_SCRIPT)
        .arg(port.to_string())
        .arg(cert)
        .arg(key)
        .stdout(std::process::Stdio::from(log))
        .stderr(std::process::Stdio::from(log_err))
        .spawn()
        .ok()
}

/// 上游 HTTPS 服务：只回 `ok`，证书由调用方给（测试里是一张自签证书）。
///
/// 用**解释器跑标准库**而不是引一个 Rust TLS 依赖：这只是一段测试夹具，
/// 而仓库的依赖纪律是"离线可构建 + 依赖克制"。内容刻意写在列 0
/// （raw string 不做缩进处理），否则 Python 会报 `IndentationError`。
const TLS_UPSTREAM_SCRIPT: &str = r#"
import http.server, ssl, sys


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = b"ok"
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


port = int(sys.argv[1])
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain(sys.argv[2], sys.argv[3])
server = http.server.HTTPServer(("127.0.0.1", port), Handler)
server.socket = ctx.wrap_socket(server.socket, server_side=True)
server.serve_forever()
"#;

fn dir_of(path: &Path) -> PathBuf {
    path.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// 一个只会回 `ok` 的极小 HTTP 服务 —— 用来当"上游"。
///
/// 用非阻塞 accept + 停止标志：**不能**用"收满 N 个连接就退出"的循环，
/// 那样 `join()` 会一直等到 N 个请求发生，测试就挂住了（第一版就是这么挂的）。
fn start_upstream() -> (u16, Arc<AtomicBool>, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        while !stop_clone.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buffer = [0_u8; 2048];
                    let _ = stream.read(&mut buffer);
                    let body = b"ok";
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(body);
                }
                Err(ref error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => break,
            }
        }
    });
    (port, stop, handle)
}

fn port_is_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
        && TcpStream::connect(("127.0.0.1", port)).is_err()
}

#[tokio::test(flavor = "multi_thread")]
async fn manager_drives_real_mitmdump_and_rules_really_take_effect() {
    let Some(mitmdump) = which("mitmdump") else {
        eprintln!("SKIP live test: mitmdump is not on PATH (run `ci/verify.sh live` where it is)");
        return;
    };

    let directory = std::env::temp_dir().join(format!("envboard-live-{}", std::process::id()));
    std::fs::remove_dir_all(&directory).ok();
    std::fs::create_dir_all(&directory).unwrap();

    let mut config = ManagerConfig::new(&directory);
    config.port_range = (27_400, 27_439);
    config.status_ttl_secs = 30;
    // 热重载轮询间隔与注解开关现在写进每环境的 `config.json`，不再经 `--set`。
    config.reload_interval_secs = 1;
    config.annotate = true;

    let clock: Arc<dyn ClockPort> = Arc::new(SystemClock);
    let core: Arc<dyn ProxyCore> = Arc::new(
        MitmproxyCore::new(
            MitmproxyCoreConfig {
                core_bin: mitmdump.clone(),
                python: None, // 走发现逻辑：shebang → uv-tool 布局 → 自检
                agent_dir: config.agent_dir.clone(),
                startup_timeout_secs: 30,
            },
            Arc::clone(&clock),
        )
        .unwrap(),
    );

    let manager = Manager::new(
        config,
        core,
        Arc::new(SocketPortProbe),
        Arc::new(ProcfsProcessTable),
        Arc::new(RealFiles),
        Arc::clone(&clock),
        Arc::new(NullLogger) as Arc<dyn LoggerPort>,
        Arc::new(JsonFileStateRepo::new(directory.join("state.json"))) as Arc<dyn StateRepo>,
        false,
    )
    .unwrap();

    // 规则导入 → 建环境 → 启动
    manager.import_rules("beta", RULES_TEXT).unwrap();
    let view = manager
        .create(&serde_json::json!({"name": "beta", "rules": "beta", "description": "M2 live"}))
        .unwrap();
    let proxy_port = view.listen.port;

    eprintln!("[live] starting beta (interpreter discovery + CA prewarm + spawn)…");
    let started = manager.start("beta").await.unwrap();
    eprintln!(
        "[live] started: health={} rules={}",
        started.health.as_str(),
        started.rules_count
    );
    assert_eq!(
        started.health.as_str(),
        "running",
        "status file must be fresh and consistent"
    );
    assert_eq!(
        started.rules_count, 1,
        "the manager counts the bound rules file"
    );

    // 真客户端经真代理访问：命中规则 → 200；不在规则里 → 502（判别性对照）
    let (upstream_port, upstream_stop, upstream) = start_upstream();
    eprintln!("[live] curling through the proxy…");
    let hit = curl_code(proxy_port, &format!("http://live.test:{upstream_port}/hit"));
    let miss = curl_code(
        proxy_port,
        &format!("http://unmapped.test:{upstream_port}/miss"),
    );
    assert_eq!(
        hit, "200",
        "the rewrite must actually take effect (rules → 127.0.0.1)"
    );
    assert_eq!(
        miss, "502",
        "a host without a rule must fail (DNS), proving 200 came from rewriting"
    );

    // 日志：默认**直接落盘** —— 子进程的 stdout/stderr 接的是文件，没有管道。
    //
    // 这条是"日志不会拖死代理"的结构性保证：管道写满（实测 64 KiB，默认详细度下约
    // 213 个请求）时，子进程会阻塞在写日志上，整个代理连同所有客户端一起挂住；
    // 文件由内核写，我们进程不在链路上，就不存在这个窗口。
    let log_file = manager
        .config()
        .log_file("beta")
        .expect("the default config must point logs at <state_dir>/logs");
    assert!(
        log_file.exists(),
        "the default sink must be a file: {}",
        log_file.display()
    );
    let raw_log = std::fs::read_to_string(&log_file).unwrap();
    assert!(
        raw_log.contains("--- envboard: env=beta"),
        "every run must leave a marker so restarts are distinguishable: {raw_log:?}"
    );

    let tail = manager.logs_tail("beta", 200).unwrap();
    assert!(
        !tail.is_empty(),
        "the manager must read the child's log file"
    );
    assert!(
        tail.iter()
            .any(|line| line.contains("envboard") || line.contains("mitmproxy")),
        "unexpected log content: {:?}",
        &tail[..tail.len().min(3)]
    );

    // CA 是被共享/预物化的：实例启动后必须落在同一个 confdir 里
    let confdir = manager.config().confdir.clone();
    assert!(
        confdir.join("mitmproxy-ca.pem").exists(),
        "CA must be pre-materialised"
    );
    assert!(confdir.join("mitmproxy-ca-cert.pem").exists());

    // 每个环境一个 agent 目录：注入器、配置、规则软链都在里面，且不由仓库路径提供。
    let agent_dir = manager.config().env_agent_dir("beta");
    let injector = agent_dir.join("envboard_mitmproxy.py");
    assert!(
        injector.exists(),
        "the injector must be materialised at runtime: {}",
        injector.display()
    );
    assert!(
        agent_dir.join("config.json").exists(),
        "the manager must write the per-environment config channel"
    );
    let link = agent_dir.join("envboard.rules");
    assert!(
        std::fs::read_link(&link)
            .map(|target| target.ends_with("beta.rules"))
            .unwrap_or(false),
        "the fixed-name rules link must point at the bound rules file: {}",
        link.display()
    );

    manager.stop("beta").await.unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline && !port_is_free(proxy_port) {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        port_is_free(proxy_port),
        "the port must be released after stop"
    );
    assert!(
        !manager.config().status_file("beta").exists(),
        "a stale status file would make a dead instance look alive"
    );
    // 日志跨重启留存：停掉之后仍然读得到（"崩溃/停止前的现场"正是最需要日志的时刻）。
    let after_stop = manager.logs_tail("beta", 50).unwrap();
    assert!(
        !after_stop.is_empty(),
        "logs must survive a stop (the file is not tied to the running entry)"
    );

    // 启动契约回显端到端：代理凭据经**两个字段**下发，注入器逐项核对宿主有没有真的
    // 接受那个 `--set`（mitmproxy 对未知/拼错的 `--set` 是静默忽略的，而 proxyauth
    // 是安全控制 —— 被忽略就等于裸奔）。凭据是停机字段，所以此时 beta 必须已停止。
    manager
        .update(
            "beta",
            &serde_json::json!({"proxy_user": "alice", "proxy_password": "s3cret"}),
        )
        .unwrap();
    let started = manager.start("beta").await.unwrap();
    assert_eq!(started.health.as_str(), "running");
    assert!(started.proxy_auth_enabled);
    let status: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(manager.config().status_file("beta")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        status["options_echo"]["proxyauth"], "ok",
        "the injector must confirm the host accepted proxyauth: {status}"
    );
    // 凭据不得出现在视图里
    assert!(
        !serde_json::to_string(&started.to_json())
            .unwrap()
            .contains("s3cret")
    );
    let _ = manager.stop("beta").await;

    upstream_stop.store(true, Ordering::SeqCst);
    upstream.join().ok();
    std::fs::remove_dir_all(&directory).ok();
}

/// 实例**自己崩掉**时，编排层必须说实话，并且不留僵尸。
///
/// 这条覆盖的是两个已实测的缺陷：
///
/// 1. 视图层（列表/详情，工作台就在看它）曾经只要 `handles` 表里有这个环境就报
///    `running`，从不探活 —— `kill -9` 掉 mitmdump 后 60 秒仍然报 running；
/// 2. 子进程退出后无人 `try_wait()`，会以僵尸形态一直挂在进程表里（`Z [mitmdump]`），
///    而"PID + starttime 都还在"又刚好骗过身份判活。
#[tokio::test(flavor = "multi_thread")]
async fn a_crashed_instance_is_reported_as_failed_and_the_child_is_reaped() {
    let Some(mitmdump) = which("mitmdump") else {
        eprintln!("SKIP live test: mitmdump is not on PATH (run `ci/verify.sh live` where it is)");
        return;
    };

    let directory = std::env::temp_dir().join(format!("envboard-crash-{}", std::process::id()));
    std::fs::remove_dir_all(&directory).ok();
    std::fs::create_dir_all(&directory).unwrap();

    let mut config = ManagerConfig::new(&directory);
    config.port_range = (27_440, 27_479);
    config.status_ttl_secs = 30;
    config.reload_interval_secs = 1;
    config.annotate = false;

    let clock: Arc<dyn ClockPort> = Arc::new(SystemClock);
    let core: Arc<dyn ProxyCore> = Arc::new(
        MitmproxyCore::new(
            MitmproxyCoreConfig {
                core_bin: mitmdump.clone(),
                python: None,
                agent_dir: config.agent_dir.clone(),
                startup_timeout_secs: 30,
            },
            Arc::clone(&clock),
        )
        .unwrap(),
    );
    let manager = Manager::new(
        config,
        core,
        Arc::new(SocketPortProbe),
        Arc::new(ProcfsProcessTable),
        Arc::new(RealFiles),
        Arc::clone(&clock),
        Arc::new(NullLogger) as Arc<dyn LoggerPort>,
        Arc::new(JsonFileStateRepo::new(directory.join("state.json"))) as Arc<dyn StateRepo>,
        false,
    )
    .unwrap();

    manager
        .create(&serde_json::json!({"name": "gamma"}))
        .unwrap();
    manager.start("gamma").await.unwrap();
    assert_eq!(manager.get("gamma").unwrap().health.as_str(), "running");

    // 状态文件里的 pid 就是子进程的 pid（注入器写的是 os.getpid()）。
    let status: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(manager.config().status_file("gamma")).unwrap(),
    )
    .unwrap();
    let pid = status["pid"].as_i64().unwrap() as i32;
    assert!(
        std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "the instance must be a real process"
    );

    // 崩给它看
    // SAFETY: `kill` 只读入参；pid 来自刚读到状态文件，且我们就是要它死。
    unsafe { libc::kill(pid, libc::SIGKILL) };

    // 视图层必须在几秒内改口，并且**一次都不能**再说 running。
    //
    // 采样要看两件事：崩溃之后立刻不能再报"在跑"；而"为什么会死"要等回收任务写下遗言
    // （它每 500ms 轮询一次），所以最终态必须带上信号 —— 中间那一小段只能说
    // "进程不在了，没人叫它停"，那是诚实的过渡态。
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut claims_running = 0;
    let mut settled = String::from("(never changed)");
    while std::time::Instant::now() < deadline {
        let view = manager.get("gamma").unwrap();
        if view.health.is_running() {
            claims_running += 1;
        } else {
            settled = view.health.reason().unwrap_or("(no reason)").to_string();
            if settled.contains("signal") {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(
        claims_running, 0,
        "the view must stop claiming a dead instance is running"
    );
    assert!(
        settled.contains("signal"),
        "the settled reason should say how it died, got: {settled}"
    );

    // 僵尸必须被收掉：`/proc/<pid>` 应当消失（reap 之后 PID 条目就没了）。
    // 若没人 `try_wait()`，这里会读到 state=Z 且条目一直存在。
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline
        && std::path::Path::new(&format!("/proc/{pid}")).exists()
    {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        !std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "the dead child must be reaped, not left as a zombie"
    );

    // 崩溃现场要留着：日志文件仍然可读，而且能读到崩溃前的内容。
    let after_crash = manager.logs_tail("gamma", 100).unwrap();
    assert!(
        !after_crash.is_empty(),
        "the log of a crashed instance must still be readable"
    );

    // 它仍然是"期望 running"，所以 reconcile 会把它重新拉起来（编排层负责收敛）。
    let report = manager.reconcile().await.unwrap();
    let restored = report
        .actions
        .iter()
        .any(|(env, action)| env == "gamma" && action == "start");
    assert!(
        restored,
        "reconcile must restart an instance that died: {:?}",
        report.actions
    );
    assert_eq!(manager.get("gamma").unwrap().health.as_str(), "running");

    manager.stop("gamma").await.unwrap();
    std::fs::remove_dir_all(&directory).ok();
}

/// **按域名放宽上游证书校验**的实机验收 —— 本次特性的核心断言。
///
/// 场景就是最初那个报障：规则把域名改写到一台内网测试机，那台机器的证书由私有 CA 签发
/// （客户端/信任库都不认），于是上游握手 502。`insecure_hosts` 只对该域名放宽。
///
/// 三条判别性对照，缺一条都不能证明"放宽是**按域名**的"：
/// 1. 名单内 → 200（放宽生效，且请求真的到了那台 HTTPS 上游）；
/// 2. 同一份规则改写、**不在**名单里 → 502（其余域名仍然严格校验）；
/// 3. 运行中把名单改成空 → 退化为 502；再加回来 → 恢复 200。
///    第 3 条同时证明这是**热**的：整个过程 `health` 一直是 `running`，没有重启过实例。
#[tokio::test(flavor = "multi_thread")]
async fn insecure_hosts_relax_upstream_tls_per_domain_and_reload_hot() {
    let Some(mitmdump) = which("mitmdump") else {
        eprintln!("SKIP live test: mitmdump is not on PATH (run `ci/verify.sh live` where it is)");
        return;
    };
    if which("openssl").is_none() || which("python3").is_none() {
        eprintln!("SKIP live test: openssl + python3 are needed for the self-signed TLS upstream");
        return;
    }

    let directory = std::env::temp_dir().join(format!("envboard-insecure-{}", std::process::id()));
    std::fs::remove_dir_all(&directory).ok();
    std::fs::create_dir_all(&directory).unwrap();

    let Some((cert, key)) = make_self_signed_cert(&directory, &["insecure.test", "other.test"])
    else {
        eprintln!("SKIP live test: cannot generate a self-signed certificate");
        return;
    };

    // 上游端口：先绑 0 拿一个空闲端口再放掉（与生产里的端口试绑同款取舍）。
    let tls_port = {
        let probe = TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap().port()
    };
    let Some(mut upstream) = start_tls_upstream(tls_port, &cert, &key) else {
        eprintln!("SKIP live test: cannot start the TLS upstream");
        return;
    };
    // 等它起来（失败就没什么可测的了）。
    let mut upstream_up = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", tls_port)).is_ok() {
            upstream_up = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if !upstream_up {
        let log = std::fs::read_to_string(directory.join("tls-upstream.log")).unwrap_or_default();
        eprintln!("[live] tls upstream on 127.0.0.1:{tls_port} DID NOT START: {log}");
    }

    let mut config = ManagerConfig::new(&directory);
    config.port_range = (27_480, 27_519);
    config.status_ttl_secs = 30;
    // 1 秒轮询 → 热重载在几秒内可观测。
    config.reload_interval_secs = 1;

    let clock: Arc<dyn ClockPort> = Arc::new(SystemClock);
    let core: Arc<dyn ProxyCore> = Arc::new(
        MitmproxyCore::new(
            MitmproxyCoreConfig {
                core_bin: mitmdump.clone(),
                python: None,
                agent_dir: config.agent_dir.clone(),
                startup_timeout_secs: 30,
            },
            Arc::clone(&clock),
        )
        .unwrap(),
    );
    let manager = Manager::new(
        config,
        core,
        Arc::new(SocketPortProbe),
        Arc::new(ProcfsProcessTable),
        Arc::new(RealFiles),
        Arc::clone(&clock),
        Arc::new(NullLogger) as Arc<dyn LoggerPort>,
        Arc::new(JsonFileStateRepo::new(directory.join("state.json"))) as Arc<dyn StateRepo>,
        false,
    )
    .unwrap();

    // 两个域名被同一份规则改写到同一台上游 —— 差别**只有**名单里有没有它。
    manager
        .import_rules("tls", "127.0.0.1 insecure.test other.test\n")
        .unwrap();
    let view = manager
        .create(&serde_json::json!({
            "name": "tls",
            "rules": "tls",
            "insecure_hosts": ["insecure.test"],
            "description": "per-domain insecure live",
        }))
        .unwrap();
    let proxy_port = view.listen.port;
    let started = manager.start("tls").await.unwrap();
    assert_eq!(started.health.as_str(), "running", "{:?}", started.health);

    let ca = manager.config().confdir.join("mitmproxy-ca-cert.pem");
    let listed = format!("https://insecure.test:{tls_port}/relaxed");
    let unlisted = format!("https://other.test:{tls_port}/strict");

    eprintln!("[live] probe 1/3: listed domain must succeed (relaxed upstream verification)");
    let relaxed = wait_for_code(proxy_port, &listed, &ca, "200").await;
    if relaxed != "200" {
        eprintln!(
            "[live] status: {}",
            manager.get("tls").unwrap().health.as_str()
        );
        for line in manager.logs_tail("tls", 60).unwrap_or_default() {
            eprintln!("[live][mitm] {line}");
        }
    }
    assert_eq!(
        relaxed, "200",
        "a domain listed in insecure_hosts must pass despite the unknown private CA"
    );

    eprintln!("[live] probe 2/3: unlisted domain must stay strict");
    assert_eq!(
        wait_for_code(proxy_port, &unlisted, &ca, "502").await,
        "502",
        "an unlisted domain rewritten to the same upstream must still fail verification"
    );

    // 热改：运行中清空名单 → 立刻退回严格；再加回来 → 恢复。
    eprintln!("[live] probe 3/3: clearing/restoring the list while running (hot)");
    let cleared = manager
        .update("tls", &serde_json::json!({"insecure_hosts": []}))
        .unwrap();
    assert_eq!(
        cleared.health.as_str(),
        "running",
        "insecure_hosts is a hot field: the instance must not be restarted"
    );
    assert_eq!(
        wait_for_code(proxy_port, &listed, &ca, "502").await,
        "502",
        "after clearing the list the domain must be strictly verified again"
    );

    let restored = manager
        .update(
            "tls",
            &serde_json::json!({"insecure_hosts": ["insecure.test"]}),
        )
        .unwrap();
    assert_eq!(restored.health.as_str(), "running");
    assert_eq!(
        wait_for_code(proxy_port, &listed, &ca, "200").await,
        "200",
        "restoring the list must take effect without a restart"
    );
    assert_eq!(
        manager.health("tls").await.unwrap(),
        envboard_core_api::InstanceHealth::Running,
        "the instance stayed healthy across both hot edits"
    );

    let _ = manager.stop("tls").await;
    let _ = upstream.kill();
    let _ = upstream.wait();
    std::fs::remove_dir_all(&directory).ok();
}

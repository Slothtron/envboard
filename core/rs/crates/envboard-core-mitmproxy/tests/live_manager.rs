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
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use envboard_core_api::{ClockPort, InstanceHealth, LoggerPort, NullLogger, ProxyCore};
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
    let output = Command::new("curl")
        .args([
            "-sS",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "--max-time",
            "20",
            "-x",
        ])
        .arg(format!("http://127.0.0.1:{proxy_port}"))
        .arg(url)
        .output()
        .expect("curl must be available for the live test");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
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

    let clock: Arc<dyn ClockPort> = Arc::new(SystemClock);
    let core: Arc<dyn ProxyCore> = Arc::new(
        MitmproxyCore::new(
            MitmproxyCoreConfig {
                core_bin: mitmdump.clone(),
                python: None, // 走发现逻辑：shebang → uv-tool 布局 → 自检
                agent_dir: config.agent_dir.clone(),
                reload_interval_secs: 1,
                annotate_flow: true,
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

    // 注入器被物化到 state_dir/agent，且不由仓库路径提供
    let injector = manager.config().agent_dir.join("envboard_mitmproxy.py");
    assert!(
        injector.exists(),
        "the injector must be materialised at runtime"
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

    // 契约回显端到端：下发一个**不存在**的选项 —— 宿主会静默忽略，只有注入器能发现。
    // 选项是环境的持久化配置，所以先写进环境（此时已停止）再启动。
    manager
        .update(
            "beta",
            &serde_json::json!({"options": {"no_such_option": "1"}}),
        )
        .unwrap();
    let _ = manager.start("beta").await;
    match manager.health("beta").await.unwrap() {
        InstanceHealth::ConfigMismatch { reason } => assert!(
            reason.contains("no_such_option"),
            "the self-check must name the offending option, got: {reason}"
        ),
        other => panic!("expected config_mismatch for a bogus option, got {other:?}"),
    }
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

    let clock: Arc<dyn ClockPort> = Arc::new(SystemClock);
    let core: Arc<dyn ProxyCore> = Arc::new(
        MitmproxyCore::new(
            MitmproxyCoreConfig {
                core_bin: mitmdump.clone(),
                python: None,
                agent_dir: config.agent_dir.clone(),
                reload_interval_secs: 1,
                annotate_flow: false,
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

//! 实机组（v3）：引擎直驱 —— 真 confdir、真 openssl 现场、真 curl 客户端。
//!
//! v2 的三条（manager 驱动 mitmdump / SIGKILL 回收 / 热重载放宽）在此换载体，
//! 语义一条条对得上：
//!
//! * ① 引擎直驱：confdir 预放 mitmproxy 形状的 CA → 引擎加载复用（不重新生成），
//!   启动回执（config_hash/epoch）如实；真 curl --cacert 校验 MITM 链成功。
//!   （options_echo 断言删除：同步 apply 后没有"静默忽略选项"的介质。）
//! * ② 崩溃路径：SIGKILL 子进程 → 注入 failed（等价"线程死了"的可观察形态），
//!   断言可见、监听释放、重拉回 running。
//! * ③ 按域名放宽：名单外 502 → 热 apply 后首个请求 200、同实例未列域名仍 502
//!   （v2 的 reload 轮询等待在 v3 塌缩成"第一次就中"）。
//!
//! 标 #[ignore]：需要 openssl 与 curl；由 ci/verify.sh 的 live 层触发。

use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use envboard_engine::EngineBackend;
use envboard_engine::ca::{CaOutcome, SharedCa};
use envboard_engine::{
    EngineHandle, EngineReport, EngineSpec, InstanceState, Listen, ProxyEngine, sha256,
};
use std::net::IpAddr;

fn work_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("envboard-livemgr-{}-{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn which(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false)
}

fn require_tools() {
    let missing: Vec<&str> = ["openssl", "curl"]
        .into_iter()
        .filter(|tool| !which(tool))
        .collect();
    assert!(missing.is_empty(), "live 层缺少宿主工具 {missing:?}");
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// openssl 造一张 mitmproxy 形状的 CA：-ca.pem = 私钥+证书拼接。返回拼接字节。
fn preseed_mitmproxy_ca(confdir: &Path) -> Vec<u8> {
    std::fs::create_dir_all(confdir).unwrap();
    let key = confdir.join("preca.key");
    let crt = confdir.join("preca.crt");
    let ok = Command::new("openssl")
        .args(["genrsa", "-out", key.to_str().unwrap(), "2048"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(ok, "openssl genrsa 失败");
    let ok = Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-new",
            "-key",
            key.to_str().unwrap(),
            "-out",
            crt.to_str().unwrap(),
            "-days",
            "825",
            "-subj",
            "/O=mitmproxy/CN=envboard-livemgr-ca",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-addext",
            "keyUsage=critical,keyCertSign,cRLSign",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(ok, "openssl req 失败");
    let bundle = format!(
        "{}{}",
        std::fs::read_to_string(&key).unwrap(),
        std::fs::read_to_string(&crt).unwrap()
    );
    std::fs::write(confdir.join("mitmproxy-ca.pem"), &bundle).unwrap();
    std::fs::copy(&crt, confdir.join("mitmproxy-ca-cert.pem")).unwrap();
    std::fs::remove_file(&key).ok();
    std::fs::remove_file(&crt).ok();
    bundle.into_bytes()
}

/// 自签 HTTPS 上游（openssl s_server -www），返回（端口, 子进程）。
fn spawn_tls_upstream(dir: &Path, host: &str) -> (u16, std::process::Child) {
    let key = dir.join(format!("{host}.key"));
    let crt = dir.join(format!("{host}.crt"));
    let ok = Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            key.to_str().unwrap(),
            "-out",
            crt.to_str().unwrap(),
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
        .unwrap()
        .success();
    assert!(ok, "自签上游证书生成失败");
    let port = free_port();
    let mut child = Command::new("openssl")
        .args([
            "s_server",
            "-accept",
            &port.to_string(),
            "-cert",
            crt.to_str().unwrap(),
            "-key",
            key.to_str().unwrap(),
            "-www",
            "-quiet",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            return (port, child);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // 超时也要收干净（clippy：所有路径都必须 wait 过子进程）。
    let _ = child.kill();
    let _ = child.wait();
    panic!("自签 HTTPS 上游没起来");
}

fn curl_code(proxy: u16, url: &str, cacert: Option<&Path>) -> u16 {
    let mut command = Command::new("curl");
    command.args([
        "-sS",
        "-o",
        "/dev/null",
        "-w",
        "%{http_code}",
        "-x",
        &format!("http://127.0.0.1:{proxy}"),
        "--noproxy",
        "",
        "--max-time",
        "20",
    ]);
    if let Some(ca) = cacert {
        command.arg("--cacert").arg(ca);
    } else {
        command.arg("-k");
    }
    command.arg(url);
    let output = command.output().expect("run curl");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

fn spec(port: u16, rules: Option<&str>, insecure: &[&str]) -> EngineSpec {
    EngineSpec {
        listen: Listen::new("127.0.0.1".parse::<IpAddr>().unwrap(), port),
        insecure_hosts: insecure.iter().map(|host| host.to_string()).collect(),
        proxy_user: None,
        proxy_password: None,
        rules_text: rules.map(str::to_string),
        rules_source: None,
        log: None,
        trajectory: None,
        capture: false,
        capture_budget: 0,
    }
}

async fn settle(backend: &EngineBackend, handle: &EngineHandle) -> EngineReport {
    for _ in 0..250 {
        let report = backend.report(handle);
        if !matches!(report.state, InstanceState::Starting) {
            return report;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    backend.report(handle)
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "需要 openssl 与 curl；由 ci/verify.sh 的 live 层触发"]
async fn engine_drives_preseeded_ca_with_honest_receipts() {
    require_tools();
    let dir = work_dir("ca");
    let confdir = dir.join("shared").join("confdir");
    let bundle = preseed_mitmproxy_ca(&confdir);

    let (ca, outcome) = SharedCa::load_or_create(&confdir).unwrap();
    match &outcome {
        CaOutcome::Loaded { fingerprint } => {
            assert_eq!(fingerprint, ca.fingerprint());
            assert_eq!(fingerprint.len(), 64, "指纹是 SPKI 的 sha256 hex");
        }
        CaOutcome::Regenerated { reason, .. } => {
            panic!("预放的 CA 必须被加载复用，不得重新生成：{reason}");
        }
    }
    assert_eq!(
        std::fs::read(confdir.join("mitmproxy-ca.pem")).unwrap(),
        bundle,
        "confdir 的既有条目必须逐字节不变"
    );

    let backend = EngineBackend::new(Arc::clone(&ca));
    let (tls_port, mut upstream) = spawn_tls_upstream(&dir, "svc.test");
    let proxy = free_port();
    let rules = "127.0.0.1 svc.test\n";
    let handle = backend
        .start("beta".into(), spec(proxy, Some(rules), &["svc.test"]))
        .await
        .unwrap();
    let settled = settle(&backend, &handle).await;
    assert_eq!(settled.state, InstanceState::Running, "{settled:?}");
    assert_eq!(
        settled.config_hash,
        sha256::hex(
            spec(proxy, Some(rules), &["svc.test"])
                .hashable_json()
                .as_bytes()
        )
    );

    let code = curl_code(
        proxy,
        &format!("https://svc.test:{tls_port}/"),
        Some(&confdir.join("mitmproxy-ca-cert.pem")),
    );
    assert_eq!(code, 200, "预放 CA 的链必须被 curl 仅凭该 CA 验过");

    backend.stop(&handle).await.unwrap();
    upstream.kill().ok();
    let _ = upstream.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "需要真端口 bind；由 ci/verify.sh 的 live 层触发"]
async fn injected_failure_is_visible_ports_release_and_restart_works() {
    require_tools();
    let dir = work_dir("fault");
    let (ca, _) = SharedCa::load_or_create(&dir).unwrap();
    let backend = EngineBackend::new(Arc::clone(&ca));
    let proxy = free_port();
    let handle = backend
        .start("beta".into(), spec(proxy, None, &[]))
        .await
        .unwrap();
    assert_eq!(
        settle(&backend, &handle).await.state,
        InstanceState::Running
    );

    assert!(backend.inject_failed("beta", "live test fault"));
    let report = backend.report(&handle);
    match report.state {
        InstanceState::Failed { reason } => {
            assert!(reason.contains("live test fault"), "{reason}")
        }
        other => panic!("expected failed, got {other:?}"),
    }
    let addr: SocketAddr = format!("127.0.0.1:{proxy}").parse().unwrap();
    let rebind = TcpListener::bind(addr);
    assert!(rebind.is_ok(), "failed 后监听端口必须已释放");
    drop(rebind);

    backend.stop(&handle).await.unwrap();
    let again = backend
        .start("beta".into(), spec(proxy, None, &[]))
        .await
        .unwrap();
    assert_eq!(settle(&backend, &again).await.state, InstanceState::Running);
    backend.stop(&again).await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "需要 openssl 与 curl；由 ci/verify.sh 的 live 层触发"]
async fn insecure_hosts_hot_flip_takes_effect_on_the_first_request() {
    require_tools();
    let dir = work_dir("insecure");
    let (ca, _) = SharedCa::load_or_create(&dir).unwrap();
    let backend = EngineBackend::new(Arc::clone(&ca));
    let (tls_port, mut upstream) = spawn_tls_upstream(&dir, "relaxed.test");
    let proxy = free_port();
    let rules = "127.0.0.1 relaxed.test\n127.0.0.1 strict.test\n";
    let handle = backend
        .start("beta".into(), spec(proxy, Some(rules), &[]))
        .await
        .unwrap();
    assert_eq!(
        settle(&backend, &handle).await.state,
        InstanceState::Running
    );

    let relaxed = format!("https://relaxed.test:{tls_port}/");
    let strict = format!("https://strict.test:{tls_port}/");
    assert_eq!(curl_code(proxy, &relaxed, None), 502, "名单外必须失败关闭");
    assert_eq!(curl_code(proxy, &strict, None), 502, "对照域名同样必须失败");

    let started = Instant::now();
    backend
        .apply(&handle, spec(proxy, Some(rules), &["relaxed.test"]))
        .unwrap();
    let first = curl_code(proxy, &relaxed, None);
    let latency = started.elapsed();
    assert_eq!(
        first, 200,
        "热改后第一次请求就必须命中（latency {latency:?}）"
    );
    assert!(
        latency < Duration::from_secs(3),
        "热生效时延异常：{latency:?}"
    );
    assert_eq!(
        curl_code(proxy, &strict, None),
        502,
        "放宽不得传染到没点名的域名"
    );

    backend.stop(&handle).await.unwrap();
    upstream.kill().ok();
    let _ = upstream.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

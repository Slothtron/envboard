//! 插件契约的实机验证（注册表装配的真实路径 + debug-inject 注入旋钮）。
//!
//! 钉住的契约（core/spec/capabilities.md 的「v3 插件与能力注册表」）：
//!
//! * connect/改写钩子 Err → fail-closed 502，正文带插件名与原因，上游零接触；
//! * 声明 bypass 的插件 → 跳过 + WARN 行 + 逐插件计数，流量不中断；
//! * 钩子超时按 Err（这里用 3s 睡眠撞 1s 上限）；
//! * Early 短路 = mock 语义：响应由插件给出，上游不连线；
//! * request-log：默认启用，每个终局请求一行（成功与失败各一行）。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use envboard_core::EngineConfig;
use envboard_core::ca::SharedCa;
use envboard_core::engine::{EngineInstance, EngineState};
use envboard_core::plugin::{
    Flow, LogWriter, OnError, PlannedResponse, Plugin, PluginError, PluginPlan, RequestView,
};
use envboard_core::target::ConnectTarget;

struct CounterPlugin {
    mode: Mode,
    hits: AtomicUsize,
}

enum Mode {
    FailHead,
    FailConnect,
    SlowHead,
    Mock,
}

#[async_trait]
impl Plugin for CounterPlugin {
    fn id(&self) -> &'static str {
        "debug-inject"
    }

    async fn on_connect(&self, _target: &mut ConnectTarget) -> Result<(), PluginError> {
        self.hits.fetch_add(1, Ordering::SeqCst);
        if matches!(self.mode, Mode::FailConnect) {
            return Err(PluginError::new("connect exploded"));
        }
        Ok(())
    }

    async fn on_request_head(&self, _cx: &RequestView<'_>) -> Result<Flow, PluginError> {
        self.hits.fetch_add(1, Ordering::SeqCst);
        match self.mode {
            Mode::FailHead | Mode::SlowHead | Mode::FailConnect => {
                if matches!(self.mode, Mode::SlowHead) {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
                Err(PluginError::new("head exploded"))
            }
            Mode::Mock => Ok(Flow::Early(PlannedResponse {
                status: 200,
                headers: vec![(r#"content-type"#.to_string(), r#"text/plain"#.to_string())],
                body: b"mocked-by-plugin\n".to_vec(),
            })),
        }
    }
}

#[derive(Debug)]
struct Sink(Mutex<Vec<String>>);

impl LogWriter for Sink {
    fn write_line(&self, line: &str) {
        self.0.lock().unwrap().push(line.to_string());
    }
}

async fn origin() -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut tcp, _)) = listener.accept().await else {
                return;
            };
            let counter = counter.clone();
            tokio::spawn(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                let mut buf = Vec::new();
                let mut byte = [0u8; 1];
                loop {
                    match tcp.read(&mut byte).await {
                        Ok(0) => break,
                        Ok(_) => {
                            buf.push(byte[0]);
                            if buf.ends_with(b"\r\n\r\n") {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let body = b"real-origin\n";
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
                let _ = tcp.write_all(head.as_bytes()).await;
                let _ = tcp.write_all(body).await;
            });
        }
    });
    (port, hits)
}

async fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

async fn boot(cfg: EngineConfig, ca: Arc<SharedCa>) -> EngineInstance {
    let engine = EngineInstance::start(cfg, ca).unwrap();
    let status = engine.wait_until_settled(Duration::from_secs(5));
    assert_eq!(status.state, EngineState::Running, "{status:?}");
    engine
}

async fn get(proxy: u16, url: &str) -> (u16, String) {
    let mut socket = TcpStream::connect(("127.0.0.1", proxy)).await.unwrap();
    socket
        .write_all(
            format!("GET {url} HTTP/1.1\r\nHost: svc.test\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
    let mut raw = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), socket.read_to_end(&mut raw)).await;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    (status, text)
}

fn config(
    proxy: u16,
    origin: SocketAddr,
    plugin: Arc<dyn Plugin>,
    on_error: OnError,
    sink: Arc<Sink>,
) -> EngineConfig {
    EngineConfig {
        listen: envboard_core_api::proxy::Listen::new(origin.ip(), proxy),
        log_writer: Some(sink),
        extra_plugins: vec![PluginPlan { plugin, on_error }],
        rules_text: Some(format!("{} svc.test", origin.ip())),
        ..Default::default()
    }
}

#[tokio::test]
async fn fail_closed_502_names_the_plugin_and_skips_upstream() {
    let dir = std::env::temp_dir().join(format!("envboard-m3-fc-{}", std::process::id()));
    let (ca, _) = SharedCa::load_or_create(&dir).unwrap();
    let (o_port, hits) = origin().await;
    let origin_addr: SocketAddr = format!("127.0.0.1:{o_port}").parse().unwrap();
    let proxy = free_port().await;
    let sink = Arc::new(Sink(Mutex::new(Vec::new())));
    let plugin = Arc::new(CounterPlugin {
        mode: Mode::FailHead,
        hits: AtomicUsize::new(0),
    });
    let engine = boot(
        config(
            proxy,
            origin_addr,
            plugin.clone(),
            OnError::FailClosed,
            sink.clone(),
        ),
        ca.clone(),
    )
    .await;

    let (status, text) = get(proxy, &format!("http://svc.test:{o_port}/x")).await;
    assert_eq!(status, 502, "fail-closed body: {text}");
    assert!(
        text.contains("debug-inject"),
        "502 must name the plugin: {text}"
    );
    assert!(
        text.contains("head exploded"),
        "502 must carry the reason: {text}"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "upstream must never be contacted"
    );

    let lines = sink.0.lock().unwrap();
    let logged = lines
        .iter()
        .any(|l| l.contains("-> 502") && l.contains("head exploded"));
    assert!(logged, "终局记录要带 error：{lines:?}");
    drop(engine);
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn bypass_declares_skip_with_warn_and_counts() {
    let dir = std::env::temp_dir().join(format!("envboard-m3-bp-{}", std::process::id()));
    let (ca, _) = SharedCa::load_or_create(&dir).unwrap();
    let (o_port, hits) = origin().await;
    let origin_addr: SocketAddr = format!("127.0.0.1:{o_port}").parse().unwrap();
    let proxy = free_port().await;
    let sink = Arc::new(Sink(Mutex::new(Vec::new())));
    let plugin = Arc::new(CounterPlugin {
        mode: Mode::FailHead,
        hits: AtomicUsize::new(0),
    });
    let engine = boot(
        config(proxy, origin_addr, plugin, OnError::Bypass, sink.clone()),
        ca.clone(),
    )
    .await;

    let (status, text) = get(proxy, &format!("http://svc.test:{o_port}/x")).await;
    assert_eq!(status, 200, "bypass keeps traffic alive: {text}");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    let lines = sink.0.lock().unwrap();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("WARN plugin debug-inject bypassed")),
        "bypass 必须留 WARN：{lines:?}"
    );
    drop(engine);
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn hook_timeout_is_an_error() {
    let dir = std::env::temp_dir().join(format!("envboard-m3-to-{}", std::process::id()));
    let (ca, _) = SharedCa::load_or_create(&dir).unwrap();
    let (o_port, _) = origin().await;
    let origin_addr: SocketAddr = format!("127.0.0.1:{o_port}").parse().unwrap();
    let proxy = free_port().await;
    let sink = Arc::new(Sink(Mutex::new(Vec::new())));
    let plugin = Arc::new(CounterPlugin {
        mode: Mode::SlowHead,
        hits: AtomicUsize::new(0),
    });
    let engine = boot(
        config(proxy, origin_addr, plugin, OnError::FailClosed, sink),
        ca.clone(),
    )
    .await;
    let (status, text) = get(proxy, &format!("http://svc.test:{o_port}/x")).await;
    assert_eq!(status, 502);
    assert!(text.contains("hook timed out"), "超时按 Err：{text}");
    drop(engine);
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn early_response_short_circuits_the_upstream() {
    let dir = std::env::temp_dir().join(format!("envboard-m3-ka-{}", std::process::id()));
    let (ca, _) = SharedCa::load_or_create(&dir).unwrap();
    let (o_port, hits) = origin().await;
    let origin_addr: SocketAddr = format!("127.0.0.1:{o_port}").parse().unwrap();
    let proxy = free_port().await;
    let sink = Arc::new(Sink(Mutex::new(Vec::new())));
    let plugin = Arc::new(CounterPlugin {
        mode: Mode::Mock,
        hits: AtomicUsize::new(0),
    });
    let engine = boot(
        config(
            proxy,
            origin_addr,
            plugin,
            OnError::FailClosed,
            sink.clone(),
        ),
        ca.clone(),
    )
    .await;
    let (status, text) = get(proxy, &format!("http://svc.test:{o_port}/x")).await;
    assert_eq!(status, 200);
    assert!(text.contains("mocked-by-plugin"), "响应由插件给出：{text}");
    assert_eq!(hits.load(Ordering::SeqCst), 0, "Early 必须不碰上游");
    let lines = sink.0.lock().unwrap();
    assert!(lines.iter().any(|l| l.contains("-> 200")), "{lines:?}");
    drop(engine);
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn connect_phase_failure_also_fails_closed() {
    let dir = std::env::temp_dir().join(format!("envboard-m3-cn-{}", std::process::id()));
    let (ca, _) = SharedCa::load_or_create(&dir).unwrap();
    let (o_port, hits) = origin().await;
    let origin_addr: SocketAddr = format!("127.0.0.1:{o_port}").parse().unwrap();
    let proxy = free_port().await;
    let sink = Arc::new(Sink(Mutex::new(Vec::new())));
    let plugin = Arc::new(CounterPlugin {
        mode: Mode::FailConnect,
        hits: AtomicUsize::new(0),
    });
    let engine = boot(
        config(proxy, origin_addr, plugin, OnError::FailClosed, sink),
        ca.clone(),
    )
    .await;
    let (status, text) = get(proxy, &format!("http://svc.test:{o_port}/x")).await;
    assert_eq!(status, 502);
    assert!(text.contains("connect exploded"), "{text}");
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    drop(engine);
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn unregistered_plugin_id_is_refused_at_startup() {
    struct Ghost;
    #[async_trait]
    impl Plugin for Ghost {
        fn id(&self) -> &'static str {
            "not-registered"
        }
    }
    let dir = std::env::temp_dir().join(format!("envboard-m3-gh-{}", std::process::id()));
    let (ca, _) = SharedCa::load_or_create(&dir).unwrap();
    let sink = Arc::new(Sink(Mutex::new(Vec::new())));
    let cfg = EngineConfig {
        listen: envboard_core_api::proxy::Listen::new(
            "127.0.0.1".parse::<std::net::IpAddr>().unwrap(),
            1,
        ),
        log_writer: Some(sink),
        extra_plugins: vec![PluginPlan {
            plugin: Arc::new(Ghost),
            on_error: OnError::FailClosed,
        }],
        ..Default::default()
    };
    // 装配期就拒绝：引擎根本不会带着无效链启动。
    let error = EngineInstance::start(cfg, ca).unwrap_err();
    assert!(
        error.message.contains("not-registered"),
        "{}",
        error.message
    );
    std::fs::remove_dir_all(&dir).ok();
}

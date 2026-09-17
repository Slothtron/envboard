//! EngineBackend（ProxyEngine 真实现）的实机验收：簿记、内存报告、
//! 热更回执、有界日志总线 —— 全部经 trait 面走，与未来 manager 的用法同形。

use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use envboard_core::EngineBackend;
use envboard_core::ca::SharedCa;
use envboard_core_api::{EngineSpec, InstanceState, LineWriter, Listen, ProxyEngine, sha256};

#[derive(Debug, Default)]
struct Capture(Mutex<Vec<String>>);

impl LineWriter for Capture {
    fn write_line(&self, line: &str) {
        self.0.lock().unwrap().push(line.to_string());
    }
}

fn spec(port: u16, rules: Option<&str>) -> EngineSpec {
    EngineSpec {
        listen: Listen::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        insecure_hosts: Vec::new(),
        proxy_user: None,
        proxy_password: None,
        rules_text: rules.map(str::to_string),
        rules_source: None,
        log: None,
    }
}

async fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn origin() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut tcp, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut byte = [0u8; 1];
                loop {
                    match tcp.read(&mut byte).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            buf.push(byte[0]);
                            if buf.ends_with(b"\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }
                let body = b"backend-origin\n";
                let _ = tcp
                    .write_all(
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len())
                            .as_bytes(),
                    )
                    .await;
                let _ = tcp.write_all(body).await;
            });
        }
    });
    port
}

async fn settle(engine: &EngineBackend, handle: &envboard_core_api::EngineHandle) -> InstanceState {
    for _ in 0..200 {
        let report = engine.report(handle);
        if !matches!(report.state, InstanceState::Starting) {
            return report.state;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    engine.report(handle).state
}

#[tokio::test]
async fn backend_lifecycle_through_the_trait_face() {
    let dir = std::env::temp_dir().join(format!("envboard-m4-backend-{}", std::process::id()));
    let (ca, _) = SharedCa::load_or_create(&dir).unwrap();
    let engine = Arc::new(EngineBackend::new(ca));
    let capture = Arc::new(Capture::default());
    let origin_port = origin().await;
    let proxy_port = free_port().await;

    let mut starting = spec(proxy_port, Some("127.0.0.1 svc.test"));
    starting.log = Some(capture.clone());
    let handle = engine.start("beta".into(), starting.clone()).await.unwrap();
    assert_eq!(settle(&engine, &handle).await, InstanceState::Running);

    // 真实转发：absolute-URI GET 走完整管线。
    let mut socket = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
    socket
        .write_all(
            format!("GET http://svc.test:{origin_port}/x HTTP/1.1\r\nHost: svc.test\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut raw = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), socket.read_to_end(&mut raw)).await;
    assert!(
        String::from_utf8_lossy(&raw).contains("backend-origin"),
        "traffic through backend: {:?}",
        String::from_utf8_lossy(&raw)
    );

    // 日志经总线落地（终局行由 request-log 内置插件写）。
    for _ in 0..50 {
        if !capture.0.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let lines = capture.0.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("-> 200") && l.contains("svc.test")),
        "bus lines: {lines:?}"
    );
    assert_eq!(engine.report(&handle).log_drops, 0);

    // 热更：hash 由 spec 的哈希（core-api 定义），回执一致、epoch+1。
    let report_before = engine.report(&handle);
    let next = spec(proxy_port, Some("10.9.8.7 other.test"));
    let hash = engine.apply(&handle, next.clone()).unwrap();
    assert_eq!(hash, sha256::hex(next.hashable_json().as_bytes()));
    let report_after = engine.report(&handle);
    assert_eq!(report_after.config_hash, hash);
    assert_eq!(report_after.epoch, report_before.epoch + 1);

    // stop 幂等面：移除后报告 stopped；重复 stop 报 NotFound。
    engine.stop(&handle).await.unwrap();
    assert_eq!(engine.report(&handle).state, InstanceState::Stopped);
    let error = engine.stop(&handle).await.unwrap_err();
    assert_eq!(error.code, envboard_core_api::ErrorCode::NotFound);
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn double_register_conflicts_without_touching_the_first() {
    let dir = std::env::temp_dir().join(format!("envboard-m4-conf-{}", std::process::id()));
    let (ca, _) = SharedCa::load_or_create(&dir).unwrap();
    let engine = EngineBackend::new(ca);
    let port = free_port().await;
    let handle = engine.start("beta".into(), spec(port, None)).await.unwrap();
    let error = engine
        .start("beta".into(), spec(port, None))
        .await
        .expect_err("same env twice is a registration conflict");
    assert_eq!(error.code, envboard_core_api::ErrorCode::Conflict);
    assert_eq!(settle(&engine, &handle).await, InstanceState::Running);
    engine.stop(&handle).await.unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

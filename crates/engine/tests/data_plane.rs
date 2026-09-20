//! 数据面验收（hermetic、离线、进程内）：CONNECT 隧道、absolute-URI、鉴权门、
//! insecure 对照、绑定直检、隧道保活。与设计验收逐项对应：规则命中 200 /
//! 未覆盖 502 / 407 / insecure 对照。
//!
//! 上游是进程内的迷你 echo（明文与自签 TLS 两种）；客户端全部走 envboard-core
//! 自己的 http 原语 —— 不引 reqwest，测试与实现共享分帧代码，分帧坏了这里先红。

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

use envboard_engine::EngineConfig;
use envboard_engine::ca::SharedCa;
use envboard_engine::engine::{EngineInstance, EngineState};
use envboard_engine::http;
use envboard_engine::http::Message;

fn localhost_config(port: u16) -> EngineConfig {
    EngineConfig {
        listen: envboard_engine::proxy::Listen::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        ..Default::default()
    }
}

async fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

/// 迷你 echo 上游（明文或自签 TLS），一问一答后关闭。
async fn spawn_echo(tls_mode: bool) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let acceptor = if tls_mode {
        let cert = rcgen::generate_simple_self_signed(vec!["upstream.test".to_string()]).unwrap();
        let base = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth();
        let mut config = base
            .with_single_cert(
                vec![CertificateDer::from(cert.cert.der().to_vec())],
                rustls::pki_types::PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                    cert.signing_key.serialize_der(),
                )),
            )
            .expect("self-signed echo cert");
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Some(TlsAcceptor::from(Arc::new(config)))
    } else {
        None
    };
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Some(acceptor) = &acceptor {
                    if let Ok(tls) = acceptor.accept(tcp).await {
                        serve_echo(tls).await;
                    }
                } else {
                    serve_echo(tcp).await;
                }
            });
        }
    });
    port
}

async fn serve_echo<S: AsyncRead + AsyncWrite + Unpin>(mut io: S) {
    let mut reader = BufReader::new(&mut io);
    let Ok(Some(head)) = http::read_head(&mut reader).await else {
        return;
    };
    let framing = http::request_framing(head.method().unwrap_or("GET"), &head)
        .unwrap_or(http::Framing::Empty);
    let Ok(body) = http::read_body(&mut reader, framing, http::MAX_BODY_BYTES).await else {
        return;
    };
    drop(reader);
    let path = head.uri().unwrap_or("/").to_string();
    let host = head.header("host").unwrap_or("-").to_string();
    let method = head.method().unwrap_or("GET").to_string();
    let payload = format!(
        "host={host} method={method} path={path} body={}\n",
        String::from_utf8_lossy(&body)
    );
    let headers = vec![("content-type".to_string(), "text/plain".to_string())];
    let _ = http::write_message(
        &mut io,
        "HTTP/1.1 200 OK",
        &headers,
        payload.as_bytes(),
        true,
    )
    .await;
    let _ = io.shutdown().await;
}

async fn start_engine(cfg: EngineConfig, ca: Arc<SharedCa>) -> EngineInstance {
    let recorder = std::sync::Arc::new(envboard_engine::trajectory::TrajectoryRecorder::start(
        std::sync::Arc::new(envboard_engine::NullLineWriter),
    ));
    let engine = EngineInstance::start(cfg, ca, recorder, 1).unwrap();
    let status = engine.wait_until_settled(Duration::from_secs(5));
    assert_eq!(
        status.state,
        EngineState::Running,
        "engine must reach running: {status:?}"
    );
    engine
}

fn client_config(ca: &SharedCa) -> Arc<ClientConfig> {
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(ca.ca_cert_der().to_vec()))
        .unwrap();
    let mut config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Arc::new(config)
}

async fn read_head_raw(socket: &mut TcpStream) -> Option<Message> {
    http::read_head_exactly(socket).await.ok().flatten()
}

/// absolute-URI GET；返回（状态码、体、响应头）。
async fn absolute_get(
    proxy: u16,
    url: &str,
    host: &str,
    credentials: Option<&str>,
) -> (u16, String, Message) {
    let mut socket = TcpStream::connect(("127.0.0.1", proxy)).await.unwrap();
    let auth = match credentials {
        Some(value) => format!("Proxy-Authorization: Basic {value}\r\n"),
        None => String::new(),
    };
    let request = format!("GET {url} HTTP/1.1\r\nHost: {host}\r\n{auth}Connection: close\r\n\r\n");
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut reader = BufReader::new(&mut socket);
    let response = http::read_head(&mut reader)
        .await
        .unwrap()
        .expect("a response");
    let status = response.status().unwrap_or(0);
    let framing = http::response_framing("GET", status, &response);
    let body = String::from_utf8_lossy(
        &http::read_body(&mut reader, framing, http::MAX_BODY_BYTES)
            .await
            .unwrap(),
    )
    .into_owned();
    (status, body, response)
}

/// CONNECT + MITM 内的一次 GET；返回（CONNECT 状态、隧道响应状态、体）。
async fn connect_get(
    proxy: u16,
    authority: &str,
    credentials: Option<&str>,
    ca: &SharedCa,
) -> (u16, u16, String) {
    let mut socket = TcpStream::connect(("127.0.0.1", proxy)).await.unwrap();
    let auth = match credentials {
        Some(value) => format!("Proxy-Authorization: Basic {value}\r\n"),
        None => String::new(),
    };
    socket
        .write_all(
            format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n{auth}\r\n").as_bytes(),
        )
        .await
        .unwrap();
    let established = read_head_raw(&mut socket).await.expect("CONNECT response");
    let established_status = established.status().unwrap_or(0);
    if established_status != 200 {
        return (established_status, 0, String::new());
    }
    let connector = TlsConnector::from(client_config(ca));
    let (host, _port) = authority.split_once(':').unwrap_or((authority, "443"));
    let name = match host.parse::<IpAddr>() {
        Ok(ip) => ServerName::IpAddress(ip.into()),
        Err(_) => ServerName::try_from(host.to_string()).unwrap(),
    };
    let tls = connector
        .connect(name, socket)
        .await
        .expect("MITM TLS handshake");
    let mut io = BufReader::new(tls);
    io.get_mut()
        .write_all(format!("GET /hello HTTP/1.1\r\nHost: {authority}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let response = http::read_head(&mut io)
        .await
        .unwrap()
        .expect("inner response");
    let status = response.status().unwrap_or(0);
    let framing = http::response_framing("GET", status, &response);
    let body = String::from_utf8_lossy(
        &http::read_body(&mut io, framing, http::MAX_BODY_BYTES)
            .await
            .unwrap(),
    )
    .into_owned();
    (200, status, body)
}

fn basic(user: &str, pass: &str) -> String {
    // 测试侧手写编码器，与 auth 模块的解码器互验（两条独立实现同一标准）。
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let text = format!("{user}:{pass}");
    let mut out = String::new();
    let (mut chunk, mut bits) = (0u32, 0u32);
    for byte in text.bytes() {
        chunk = (chunk << 8) | u32::from(byte);
        bits += 8;
        while bits >= 6 {
            bits -= 6;
            out.push(TABLE[((chunk >> bits) & 0x3f) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(TABLE[((chunk << (6 - bits)) & 0x3f) as usize] as char);
    }
    while !out.len().is_multiple_of(4) {
        out.push('=');
    }
    out
}

fn temp(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("envboard-m2-{tag}-{}", std::process::id()))
}

#[tokio::test]
async fn rule_hit_200_and_miss_502() {
    let origin = spawn_echo(false).await;
    let dir = temp("rule");
    let (ca, _) = SharedCa::load_or_create(&dir).unwrap();
    let proxy = free_port().await;
    let mut cfg = localhost_config(proxy);
    cfg.rules_text = Some("127.0.0.1 svc.test".to_string());
    let engine = start_engine(cfg, ca.clone()).await;

    let (status, body, _) = absolute_get(
        proxy,
        &format!("http://svc.test:{origin}/x?q=1"),
        &format!("svc.test:{origin}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "rule-covered host must be served: {body}");
    assert!(body.contains("path=/x?q=1"), "echo body: {body}");
    assert!(body.contains("host=svc.test"), "Host 头不被改写: {body}");

    let (status, _, _) = absolute_get(
        proxy,
        "http://nowhere.invalid:65001/y",
        "nowhere.invalid:65001",
        None,
    )
    .await;
    assert_eq!(status, 502, "uncovered + unresolvable must be 502");
    engine.stop();
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn auth_gate_returns_407_and_admits_with_credentials() {
    let origin = spawn_echo(false).await;
    let dir = temp("auth");
    let (ca, _) = SharedCa::load_or_create(&dir).unwrap();
    let proxy = free_port().await;
    let mut cfg = localhost_config(proxy);
    cfg.rules_text = Some("127.0.0.1 svc.test".to_string());
    cfg.proxy_user = Some("alice".into());
    cfg.proxy_password = Some("s3cret".into());
    let engine = start_engine(cfg, ca.clone()).await;

    let (status, _, response) = absolute_get(
        proxy,
        &format!("http://svc.test:{origin}/"),
        "svc.test",
        None,
    )
    .await;
    assert_eq!(status, 407);
    assert_eq!(
        response.header("proxy-authenticate"),
        Some("Basic realm=\"envboard\"")
    );

    let creds = basic("alice", "s3cret");
    let (status, body, _) = absolute_get(
        proxy,
        &format!("http://svc.test:{origin}/ok"),
        "svc.test",
        Some(&creds),
    )
    .await;
    assert_eq!(status, 200, "with credentials: {body}");

    let wrong = basic("alice", "nope");
    let (status, _, _) = absolute_get(
        proxy,
        &format!("http://svc.test:{origin}/"),
        "svc.test",
        Some(&wrong),
    )
    .await;
    assert_eq!(status, 407, "wrong password still rejected");

    // CONNECT 走同一道门。
    let (established, _, _) = connect_get(proxy, &format!("svc.test:{origin}"), None, &ca).await;
    assert_eq!(established, 407, "CONNECT without credentials must 407");
    engine.stop();
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn insecure_hosts_control_flips_without_restart() {
    let origin = spawn_echo(true).await;
    let dir = temp("insecure");
    let (ca, _) = SharedCa::load_or_create(&dir).unwrap();
    let proxy = free_port().await;
    let mut cfg = localhost_config(proxy);
    cfg.rules_text = Some("127.0.0.1 upstream.test".to_string());
    let engine = start_engine(cfg.clone(), ca.clone()).await;

    // 名单外：自签上游 → 严格校验失败 → 502（带可行动建议）。
    let (_, status, body) = connect_get(proxy, &format!("upstream.test:{origin}"), None, &ca).await;
    assert_eq!(
        status, 502,
        "unverifiable upstream must fail closed: {body}"
    );
    assert!(
        body.contains("insecure_hosts"),
        "error must point at the fix: {body}"
    );

    // 热改名单：同一实例、epoch+1、首个后续请求即生效。
    cfg.insecure_hosts = vec!["upstream.test".to_string()];
    let before = engine.status();
    let hash = engine.apply_config(cfg).unwrap();
    let after = engine.status();
    assert_eq!(after.config_hash, hash, "status must echo the applied hash");
    assert_eq!(after.epoch, before.epoch + 1);
    assert!(
        matches!(after.state, EngineState::Running),
        "instance untouched: {after:?}"
    );

    let (_, status, body) = connect_get(proxy, &format!("upstream.test:{origin}"), None, &ca).await;
    assert_eq!(status, 200, "after hot add: {body}");
    engine.stop();
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn port_conflict_is_detected_without_try_bind() {
    let dir = temp("conflict");
    let (ca, _) = SharedCa::load_or_create(&dir).unwrap();
    let proxy = free_port().await;
    let first = start_engine(localhost_config(proxy), ca.clone()).await;
    let recorder = std::sync::Arc::new(envboard_engine::trajectory::TrajectoryRecorder::start(
        std::sync::Arc::new(envboard_engine::NullLineWriter),
    ));
    let second = EngineInstance::start(localhost_config(proxy), ca.clone(), recorder, 1).unwrap();
    let status = second.wait_until_settled(Duration::from_secs(5));
    assert_eq!(
        status.state,
        EngineState::PortConflict { port: proxy },
        "{status:?}"
    );
    first.stop();
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn tunnel_keepalive_serves_sequential_requests() {
    // CONNECT 隧道内的上游必然是 TLS；echo 用自签证书，靠名单放行。
    let origin = spawn_echo(true).await;
    let dir = temp("ka");
    let (ca, _) = SharedCa::load_or_create(&dir).unwrap();
    let proxy = free_port().await;
    let mut cfg = localhost_config(proxy);
    cfg.rules_text = Some("127.0.0.1 svc.test".to_string());
    cfg.insecure_hosts = vec!["svc.test".to_string()];
    let engine = start_engine(cfg, ca.clone()).await;

    let mut socket = TcpStream::connect(("127.0.0.1", proxy)).await.unwrap();
    socket
        .write_all(
            format!("CONNECT svc.test:{origin} HTTP/1.1\r\nHost: svc.test\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
    let established = read_head_raw(&mut socket).await.unwrap();
    assert_eq!(established.status(), Some(200));
    let connector = TlsConnector::from(client_config(&ca));
    let tls = connector
        .connect(ServerName::try_from("svc.test").unwrap(), socket)
        .await
        .unwrap();
    let mut io = BufReader::new(tls);
    for round in 0..2u8 {
        io.get_mut()
            .write_all(
                format!("GET /ka HTTP/1.1\r\nHost: svc.test:{origin}\r\nX-Round: {round}\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let response = http::read_head(&mut io).await.unwrap().unwrap();
        assert_eq!(response.status(), Some(200), "round {round}");
        let body = http::read_body(
            &mut io,
            http::response_framing("GET", 200, &response),
            http::MAX_BODY_BYTES,
        )
        .await
        .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("path=/ka"));
    }
    engine.stop();
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn listen_is_a_stop_time_field_for_hot_update() {
    let dir = temp("listen");
    let (ca, _) = SharedCa::load_or_create(&dir).unwrap();
    let proxy = free_port().await;
    let engine = start_engine(localhost_config(proxy), ca.clone()).await;
    let moved = localhost_config(proxy + 1);
    let error = engine.apply_config(moved).unwrap_err();
    assert!(error.message.contains("stop-time"), "{}", error.message);
    engine.stop();
    std::fs::remove_dir_all(&dir).ok();
}

// --------------------------------------------------------------------------- //
// 二级代理（chained proxy）
// --------------------------------------------------------------------------- //

use envboard_engine::UpstreamSpec;
use envboard_engine::auth;

/// 进程内迷你二级代理的行为模式。
#[derive(Clone)]
enum ChainedBehavior {
    /// 直通（不要求鉴权）。
    Open,
    /// 要求 Basic 鉴权（缺失或错误一律 407）。
    RequireAuth(String, String),
    /// 一律以指定状态码拒绝。
    Reject(u16),
}

async fn spawn_chained_proxy(behavior: ChainedBehavior) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            let behavior = behavior.clone();
            tokio::spawn(async move {
                serve_chained(socket, behavior).await;
            });
        }
    });
    port
}

fn deny_head(status: u16) -> String {
    if status == 407 {
        "HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"chained\"\r\nContent-Length: 0\r\n\r\n".to_string()
    } else {
        format!("HTTP/1.1 {status} Forbidden\r\nContent-Length: 0\r\n\r\n")
    }
}

/// 迷你二级代理：CONNECT 隧道裸字节互抄；absolute-URI 转发到真实目标。
async fn serve_chained(mut socket: TcpStream, behavior: ChainedBehavior) {
    let mut reader = BufReader::new(&mut socket);
    let Ok(Some(head)) = http::read_head(&mut reader).await else {
        return;
    };
    let auth_ok = match &behavior {
        ChainedBehavior::RequireAuth(user, password) => head
            .header("proxy-authorization")
            .and_then(auth::parse_basic)
            .is_some_and(|(name, secret)| name == *user && secret == password.as_bytes()),
        _ => true,
    };
    if !auth_ok {
        drop(reader);
        let _ = socket.write_all(deny_head(407).as_bytes()).await;
        return;
    }
    let is_connect = head.method().unwrap_or("").eq_ignore_ascii_case("CONNECT");
    if let ChainedBehavior::Reject(status) = behavior {
        drop(reader);
        let _ = socket.write_all(deny_head(status).as_bytes()).await;
        return;
    }
    if is_connect {
        // CONNECT：解析 authority（测试域名一律映射 127.0.0.1），建隧道后裸互抄。
        let uri = head.uri().unwrap_or_default().to_string();
        drop(reader);
        let (host, port) = split_authority(&uri);
        let Ok(mut origin) = TcpStream::connect((host.as_str(), port)).await else {
            return;
        };
        let _ = socket
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await;
        let _ = tokio::io::copy_bidirectional(&mut socket, &mut origin).await;
        return;
    }
    // absolute-URI：解析目标 → 转发（去掉代理鉴权头）→ 回传响应。
    let uri = head.uri().unwrap_or_default().to_string();
    let Some((host, port, path)) = http::parse_absolute_http_uri(&uri) else {
        return;
    };
    let framing = http::request_framing(head.method().unwrap_or("GET"), &head)
        .unwrap_or(http::Framing::Empty);
    let Ok(body) = http::read_body(&mut reader, framing, http::MAX_BODY_BYTES).await else {
        return;
    };
    drop(reader);
    let host_ip = if host.parse::<IpAddr>().is_ok() {
        host.clone()
    } else {
        "127.0.0.1".to_string()
    };
    let Ok(mut origin) = TcpStream::connect((host_ip.as_str(), port)).await else {
        let _ = http::write_message(
            &mut socket,
            "HTTP/1.1 502 Bad Gateway",
            &[],
            b"chained: origin unreachable",
            true,
        )
        .await;
        return;
    };
    let mut headers = http::forward_request_headers(&head);
    headers.retain(|(name, _)| name != "proxy-authorization");
    headers.push(("connection".to_string(), "close".to_string()));
    let first = format!("{} {path} HTTP/1.1", head.method().unwrap_or("GET"));
    if http::write_message(&mut origin, &first, &headers, &body, true)
        .await
        .is_err()
    {
        return;
    }
    let mut origin_reader = BufReader::new(&mut origin);
    let Ok(Some(response)) = http::read_head(&mut origin_reader).await else {
        return;
    };
    let status = response.status().unwrap_or(502);
    let response_framing =
        http::response_framing(head.method().unwrap_or("GET"), status, &response);
    let Ok(response_body) =
        http::read_body(&mut origin_reader, response_framing, http::MAX_BODY_BYTES).await
    else {
        return;
    };
    drop(origin_reader);
    let response_headers = http::forward_response_headers(&response);
    let _ = http::write_message(
        &mut socket,
        &response.first,
        &response_headers,
        &response_body,
        true,
    )
    .await;
}

fn split_authority(uri: &str) -> (String, u16) {
    let (host, port) = uri.rsplit_once(':').unwrap_or((uri, "443"));
    let port = port.parse().unwrap_or(443);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    (host.to_string(), port)
}

fn chained_config(proxy_port: u16, chained_port: u16, behavior_auth: bool) -> EngineConfig {
    EngineConfig {
        listen: envboard_engine::proxy::Listen::new(IpAddr::V4(Ipv4Addr::LOCALHOST), proxy_port),
        upstream: Some(UpstreamSpec {
            host: "127.0.0.1".to_string(),
            port: chained_port,
            user: behavior_auth.then(|| "alice".to_string()),
            password: behavior_auth.then(|| "s3cret".to_string()),
        }),
        ..Default::default()
    }
}

#[tokio::test]
async fn tls_traffic_tunnels_through_the_chained_proxy() {
    let origin = spawn_echo(true).await;
    let chained = spawn_chained_proxy(ChainedBehavior::RequireAuth(
        "alice".to_string(),
        "s3cret".to_string(),
    ))
    .await;
    let dir = temp("chained-tls");
    let (ca, _) = SharedCa::load_or_create(&dir).unwrap();
    let proxy = free_port().await;
    let mut cfg = chained_config(proxy, chained, true);
    cfg.rules_text = Some("127.0.0.1 upstream.test".to_string());
    cfg.insecure_hosts = vec!["upstream.test".to_string()];
    let engine = start_engine(cfg, ca.clone()).await;

    // 规则改写 resolved_addr → CONNECT 走改写后的 127.0.0.1:origin；隧道内
    // rustls 握手 + 请求与直连完全一致（insecure 名单语义不变）。
    let (established, status, body) =
        connect_get(proxy, &format!("upstream.test:{origin}"), None, &ca).await;
    assert_eq!(established, 200, "chained proxy must admit the tunnel");
    assert_eq!(status, 200, "tunneled request must be served: {body}");
    assert!(body.contains("path=/hello"), "{body}");
    engine.stop();
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn chained_proxy_refusal_becomes_502_with_the_status() {
    let origin = spawn_echo(true).await;
    let chained = spawn_chained_proxy(ChainedBehavior::Reject(403)).await;
    let dir = temp("chained-refuse");
    let (ca, _) = SharedCa::load_or_create(&dir).unwrap();
    let proxy = free_port().await;
    let mut cfg = chained_config(proxy, chained, false);
    cfg.rules_text = Some("127.0.0.1 upstream.test".to_string());
    cfg.insecure_hosts = vec!["upstream.test".to_string()];
    let engine = start_engine(cfg, ca.clone()).await;

    let (_, status, body) = connect_get(proxy, &format!("upstream.test:{origin}"), None, &ca).await;
    assert_eq!(status, 502, "{body}");
    assert!(
        body.contains("refused CONNECT with 403"),
        "error must name the proxy's answer: {body}"
    );
    engine.stop();
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn missing_upstream_credentials_surface_the_407() {
    let origin = spawn_echo(true).await;
    let chained = spawn_chained_proxy(ChainedBehavior::RequireAuth(
        "alice".to_string(),
        "s3cret".to_string(),
    ))
    .await;
    let dir = temp("chained-407");
    let (ca, _) = SharedCa::load_or_create(&dir).unwrap();
    let proxy = free_port().await;
    let mut cfg = chained_config(proxy, chained, false);
    cfg.rules_text = Some("127.0.0.1 upstream.test".to_string());
    cfg.insecure_hosts = vec!["upstream.test".to_string()];
    let engine = start_engine(cfg, ca.clone()).await;

    let (_, status, body) = connect_get(proxy, &format!("upstream.test:{origin}"), None, &ca).await;
    assert_eq!(status, 502, "{body}");
    assert!(
        body.contains("refused CONNECT with 407"),
        "missing credentials must be actionable: {body}"
    );
    engine.stop();
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn plain_http_forwards_as_absolute_uri_through_the_chained_proxy() {
    let origin = spawn_echo(false).await;
    let chained = spawn_chained_proxy(ChainedBehavior::Open).await;
    let dir = temp("chained-plain");
    let (ca, _) = SharedCa::load_or_create(&dir).unwrap();
    let proxy = free_port().await;
    let cfg = chained_config(proxy, chained, false);
    let engine = start_engine(cfg, ca.clone()).await;

    // 目标域名不可解析（svc.test）：直连必然 502，经代理由代理侧解析转发。
    let (status, body, _) = absolute_get(
        proxy,
        &format!("http://svc.test:{origin}/x"),
        &format!("svc.test:{origin}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "chained plain http must be served: {body}");
    assert!(body.contains("path=/x"), "{body}");
    assert!(body.contains("host=svc.test"), "Host 头不被改写: {body}");
    engine.stop();
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn plain_http_through_authed_chained_proxy_carries_the_credentials() {
    let origin = spawn_echo(false).await;
    let chained = spawn_chained_proxy(ChainedBehavior::RequireAuth(
        "alice".to_string(),
        "s3cret".to_string(),
    ))
    .await;
    let dir = temp("chained-plain-auth");
    let (ca, _) = SharedCa::load_or_create(&dir).unwrap();
    let proxy = free_port().await;
    let cfg = chained_config(proxy, chained, true);
    let engine = start_engine(cfg, ca.clone()).await;

    // 明文 absolute-URI 转发与 CONNECT 是同一条链路的两半：凭据只注入
    // CONNECT 那一半的话，二级代理的 407 会原样穿给客户端（实测即此）。
    let (status, body, _) = absolute_get(
        proxy,
        &format!("http://svc.test:{origin}/x"),
        &format!("svc.test:{origin}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "authed chained plain http must be served, not 407: {body}"
    );
    assert!(body.contains("path=/x"), "{body}");
    engine.stop();
    std::fs::remove_dir_all(&dir).ok();
}

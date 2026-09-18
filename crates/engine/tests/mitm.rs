//! 共享 CA 与 MITM 握手的验收测试（全部离线、进程内，满足门禁默认层"不依赖
//! 真宿主"的分层要求）。
//!
//! 三件核心事实在这里钉死：
//!
//! 1. **mitmproxy 格式兼容**：tests/fixtures/mitmproxy-compat/ 是一张用 openssl
//!    按 mitmproxy 的写盘形状生成的 RSA CA（-ca.pem = PKCS#1 私钥 + 证书拼接，
//!    CN=mitmproxy），fixture 里的密钥是纯测试假值。加载它、并用这把 RSA 私钥
//!    现场签发叶子证书完成一次 rustls 握手 —— 这就是已装客户端零感证的判据。
//! 2. **配对与坏 CA 处置**：私钥与证书公钥不一致 → 拒绝加载；load_or_create
//!    对坏 CA 删除重新物化，并把原因作为可见结论交回上层。
//! 3. **生成分支**：confdir 为空时物化新 CA（ECDSA），文件权限 0600，重载指纹
//!    稳定。
//!
//! fixture 是两张独立的 CA（compat 与 compat-alt），交叉喂入即构造"不配对"。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::{TlsAcceptor, TlsConnector};

use envboard_engine::SharedCa;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn read_pair(dir: &Path) -> (Vec<u8>, Vec<u8>) {
    (
        std::fs::read(dir.join("mitmproxy-ca.pem")).unwrap(),
        std::fs::read(dir.join("mitmproxy-ca-cert.pem")).unwrap(),
    )
}

#[test]
fn mitmproxy_format_rsa_ca_loads_and_pairs() {
    let (key_pem, cert_pem) = read_pair(&fixture("mitmproxy-compat"));
    assert!(
        String::from_utf8_lossy(&key_pem).contains("BEGIN RSA PRIVATE KEY"),
        "fixture must stay in the mitmproxy PKCS#1 shape"
    );
    let ca = SharedCa::load(&key_pem, &cert_pem).expect("the mitmproxy-shaped CA must load");
    assert_eq!(ca.fingerprint().len(), 64, "fingerprint is a sha256 hex");
    let cn = envboard_engine::der::cert_subject_cn(ca.ca_cert_der()).unwrap();
    assert_eq!(cn, "mitmproxy", "loaded CA keeps its Subject");
}

#[test]
fn mismatched_key_and_certificate_are_rejected() {
    let (key_pem, _) = read_pair(&fixture("mitmproxy-compat"));
    let (_, cert_pem) = read_pair(&fixture("mitmproxy-compat-alt"));
    let error = SharedCa::load(&key_pem, &cert_pem).expect_err("cross-fed pair must fail");
    assert!(
        error.contains("embeds a different certificate") || error.contains("pair"),
        "unexpected rejection reason: {error}"
    );
}

#[tokio::test]
async fn mitm_handshake_chains_to_the_loaded_rsa_ca() {
    let (key_pem, cert_pem) = read_pair(&fixture("mitmproxy-compat"));
    let ca = Arc::new(SharedCa::load(&key_pem, &cert_pem).unwrap());
    let chain = negotiate(&ca, "demo.example.test").await;
    assert_eq!(
        chain.len(),
        1,
        "the leaf is sent alone: clients hold the CA"
    );
    let leaf = &chain[0];
    assert!(
        leaf.windows(b"demo.example.test".len())
            .any(|w| w == b"demo.example.test"),
        "the leaf must carry the SNI name"
    );
    // 第二次握手走缓存：同一 SNI 必须拿到逐字节相同的叶子证书。
    let again = negotiate(&ca, "demo.example.test").await;
    assert_eq!(again, chain, "SNI cache must reuse the leaf");
    // 不同 SNI 是不同证书。
    let other = negotiate(&ca, "other.example.test").await;
    assert_ne!(other, chain);
}

#[tokio::test]
async fn generated_ca_materializes_files_and_serves_handshakes() {
    let dir = temp_dir("envboard-m1-generate");
    let (ca, outcome) = SharedCa::load_or_create(&dir).unwrap();
    assert_eq!(
        outcome.fingerprint(),
        ca.fingerprint(),
        "the outcome must report the same CA that was returned"
    );
    // 文件形态与 mitmproxy 一致（同名、拼接顺序一致由加载侧反证：能重新加载）。
    let reloaded_bytes = read_pair(&dir);
    let reloaded =
        SharedCa::load(&reloaded_bytes.0, &reloaded_bytes.1).expect("generated files must reload");
    assert_eq!(reloaded.fingerprint(), ca.fingerprint());
    // 私钥文件必须 0600（凭据面纪律）。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("mitmproxy-ca.pem"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "the key file must be owner-only");
    }
    negotiate(&ca, "fresh.example.test").await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn corrupt_ca_is_rematerialised_with_a_visible_reason() {
    let dir = temp_dir("envboard-m1-corrupt");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("mitmproxy-ca.pem"),
        b"-----BEGIN RSA PRIVATE KEY-----\nbroken\n",
    )
    .unwrap();
    std::fs::write(dir.join("mitmproxy-ca-cert.pem"), b"garbage\n").unwrap();
    let (ca, outcome) = SharedCa::load_or_create(&dir).unwrap();
    let reason = match outcome {
        envboard_engine::CaOutcome::Regenerated { reason, .. } => reason,
        other => panic!("a broken CA must be reported as Regenerated, got {other:?}"),
    };
    assert!(!reason.is_empty());
    // 重新物化后立即可用。
    negotiate(&ca, "after-repair.example.test").await;
    std::fs::remove_dir_all(&dir).ok();
}

/// 一次进程内 TLS 握手：服务端用 SNI 现签，客户端只信任这张 CA。
/// 返回值是客户端看到的证书链（DER 列表）。ALPN 必须是 http/1.1。
async fn negotiate(ca: &Arc<SharedCa>, sni: &str) -> Vec<Vec<u8>> {
    let server_config = envboard_engine::mitm_server_config(ca.clone()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let acceptor = TlsAcceptor::from(server_config);
        let mut tls = acceptor.accept(tcp).await.unwrap();
        tls.write_all(b"ok").await.unwrap();
        tls.flush().await.unwrap();
    });

    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(ca.ca_cert_der().to_vec()))
        .expect("the CA cert must be a valid root");
    let mut client_config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
    // 客户端必须主动请求 ALPN，否则服务端没有可协商的协议（rustls 的
    // alpn_protocol 为 None 不是降级，是双方根本没谈这件事）。
    client_config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let connector = TlsConnector::from(Arc::new(client_config));
    let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    let name = ServerName::try_from(sni.to_string()).unwrap();
    let mut tls = connector.connect(name, tcp).await.unwrap();
    {
        let (_, conn) = tls.get_ref();
        assert_eq!(conn.alpn_protocol(), Some(&b"http/1.1"[..]));
        let chain: Vec<Vec<u8>> = conn
            .peer_certificates()
            .expect("the server must present a chain")
            .iter()
            .map(|cert| cert.to_vec())
            .collect();
        let mut buf = [0u8; 2];
        tls.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ok");
        server.await.unwrap();
        chain
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir().join(format!("envboard-m1-{}-{}", tag, std::process::id()));
    path.push(uuid_lite());
    path
}

/// 无外部依赖的小随机后缀（测试目录隔离用，不追求密码学质量）。
fn uuid_lite() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    format!("{:08x}{:04x}", nanos, std::process::id() & 0xffff)
}

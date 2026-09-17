//! rustls 接线：MITM 服务侧（按 SNI 现签）与上游客户侧（系统信任库严格校验）。
//!
//! ALPN 两边都只协商 http/1.1：数据面 v1 只实现 HTTP/1.1，声明 h2 会诱导客户端
//! 走我们不支持的协议面；浏览器看到 h1 会正常回退，强制 h2 的客户端（gRPC 等）
//! 会失败 —— 这是写进 README 已知限制的行为，不是静默降级。

use std::sync::Arc;

use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::{ClientConfig, RootCertStore, ServerConfig};

use envboard_core_api::{Error, ErrorCode};

use crate::ca::SharedCa;

/// 无 SNI 的 ClientHello 回退名。握手必须继续（拿到证书后才在主机名校验上失败），
/// 否则"无 SNI"这一类客户端连可诊断的错误都收不到。
pub const NO_SNI_FALLBACK_HOST: &str = "envboard.invalid";

/// 按 SNI 现签的服务端证书解析器：命中 SharedCa 的缓存则复用，未命中现签。
pub struct SniResolver {
    ca: Arc<SharedCa>,
}

impl SniResolver {
    pub fn new(ca: Arc<SharedCa>) -> Self {
        Self { ca }
    }
}

impl std::fmt::Debug for SniResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SniResolver").finish_non_exhaustive()
    }
}

impl ResolvesServerCert for SniResolver {
    fn resolve(&self, client_hello: ClientHello) -> Option<Arc<rustls::sign::CertifiedKey>> {
        let host = match client_hello.server_name() {
            Some(name) => name.to_string(),
            None => NO_SNI_FALLBACK_HOST.to_string(),
        };
        // 签发失败没有"更安全的下一步"：返回 None 让握手失败，错误由数据面带进
        // 日志与响应（M-P2），绝不在这里悄悄换一张别的证书。
        self.ca.issue(&host.to_ascii_lowercase()).ok()
    }
}

/// MITM 服务侧配置：客户端与我们握手拿到的就是按 SNI 现签的链。
pub fn mitm_server_config(ca: Arc<SharedCa>) -> Result<Arc<ServerConfig>, Error> {
    let mut config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(rustls_error)?
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(SniResolver::new(ca)));
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// 上游客户侧配置：系统信任库严格校验。信任库来源从 certifi 换成系统库的影响
/// （哪些域名免名单通过可能变化）由 CHANGELOG 的 v3 段落登记。
/// 按域名放宽（insecure_hosts）不在这份配置里 —— 那是数据面的 TlsPolicy 接缝，
/// 放宽必须走显式描述符，禁止用"关掉校验的全局 client config"表达。
pub fn strict_upstream_client_config() -> Result<Arc<ClientConfig>, Error> {
    let mut roots = RootCertStore::empty();
    let loaded = rustls_native_certs::load_native_certs();
    let mut added = 0usize;
    for cert in loaded.certs {
        if roots.add(cert).is_ok() {
            added += 1;
        }
    }
    if added == 0 {
        let detail: Vec<String> = loaded.errors.iter().map(ToString::to_string).collect();
        return Err(Error::new(
            ErrorCode::InternalError,
            format!(
                "the system trust store yielded no usable roots: {}",
                detail.join("; ")
            ),
        ));
    }
    let mut config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(rustls_error)?
            .with_root_certificates(roots)
            .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

fn rustls_error(error: rustls::Error) -> Error {
    Error::new(ErrorCode::InternalError, format!("rustls: {error}"))
}

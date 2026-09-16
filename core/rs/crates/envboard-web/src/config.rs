//! 工作台的配置与**安全模型**。
//!
//! 三条不可省的约束：
//!
//! 1. **token 鉴权默认启用**：未给 `--token` 时自动生成随机 token（启动日志打印
//!    `dashboard: …/?token=…` 供点击直达）。显式 `--without-token` 才能关掉，且
//!    **非回环监听拒绝关闭** —— 不允许无鉴权对外。
//! 2. **校验 `Host` 头**必须等于配置的监听地址 —— 防 DNS rebinding。
//! 3. **变更类路由必须带自定义头** —— 浏览器跨站简单请求带不了自定义头
//!    （fetch 会被 preflight 挡下），所以这一条就挡住了 CSRF。
//!
//! 这三条不是可选项：工作台能起进程、改规则，等于能改本机流量走向。

use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use envboard_core_api::Error;

/// 变更类请求必须携带的头（前端统一带）。
pub const REQUEST_HEADER: &str = "x-envboard-request";
/// token 头（配置了 token 时才要求）。
pub const TOKEN_HEADER: &str = "x-envboard-token";

/// CSP：**不含任何 `unsafe-inline`** —— 因为资产是外置的 `app.css` / `app.js`。
///
/// v1 的教训是"内联脚本被宿主 CSP 拒绝，而 curl 断言查不出来"；我们自己的宿主更要把
/// 这条钉死：CSP 与资产形态必须一致，否则样式或脚本会静默失效。
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
     connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

#[derive(Debug, Clone)]
pub struct WebConfig {
    pub listen: SocketAddr,
    /// 非本机监听时必填。
    pub token: Option<String>,
    /// 生成 `proxy_command` 用的状态目录（仅用于展示与日志）。
    pub state_dir: PathBuf,
}

impl WebConfig {
    /// 解析 `--listen`（`host:port`）并做安全校验。
    ///
    /// token 语义：`--token` 显式指定 → 原样使用；未指定且未 `--without-token`
    /// → **自动生成**随机 token（见 [`generate_token`]）；`--without-token` → 关闭，
    /// 但非回环监听拒绝（工作台能起进程、改规则，无鉴权对外不设此例）。
    /// `--token` 与 `--without-token` 同给是配置冲突，加载即失败。
    pub fn parse(
        listen: &str,
        token: Option<String>,
        without_token: bool,
        state_dir: &std::path::Path,
    ) -> Result<Self, Error> {
        let listen: SocketAddr = listen.parse().map_err(|_| {
            Error::invalid_config(
                "web.listen",
                format!("expected <host>:<port>, got {listen:?}"),
            )
        })?;
        if listen.port() == 0 {
            return Err(Error::invalid_config(
                "web.listen",
                "port 0 is not a valid listen port",
            ));
        }

        let local = is_loopback(listen.ip());

        if token.is_some() && without_token {
            return Err(Error::invalid_config(
                "web.token",
                "--token and --without-token are mutually exclusive: pick one",
            ));
        }
        if without_token {
            if !local {
                return Err(Error::invalid_config(
                    "web.token",
                    format!(
                        "refusing --without-token on a non-loopback listen ({}): the workbench \
                         can start processes and rewrite rules, so unauthenticated exposure is \
                         not allowed; bind 127.0.0.1 or keep the token",
                        listen.ip()
                    ),
                ));
            }
            return Ok(Self {
                listen,
                token: None,
                state_dir: state_dir.to_path_buf(),
            });
        }

        // 默认启用：显式值优先（trim 去空白），否则自动生成随机 token
        let token = match token {
            Some(value) if !value.trim().is_empty() => value.trim().to_string(),
            _ => generate_token(),
        };

        Ok(Self {
            listen,
            token: Some(token),
            state_dir: state_dir.to_path_buf(),
        })
    }

    /// 允许的 `Host` 头取值 —— 由**配置**推导，不硬编码端口（否则改端口就失效）。
    pub fn allowed_hosts(&self) -> Vec<String> {
        let port = self.listen.port();
        let mut hosts = vec![format!("{}:{port}", self.listen.ip())];
        if is_loopback(self.listen.ip()) {
            hosts.push(format!("localhost:{port}"));
            // 回环地址的不同写法都要认，否则用户换个写法访问就被 403
            hosts.push(format!("127.0.0.1:{port}"));
            hosts.push(format!("[::1]:{port}"));
        }
        hosts.sort();
        hosts.dedup();
        hosts
    }
}

pub fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

/// 生成随机 token（32 个十六进制字符 = 128 bit 熵）。
///
/// 依赖来源刻意**不加**：从 `/dev/urandom` 读 16 字节就够了。读不到（非 Unix、
/// 极端容器环境）才退化到时间+进程号混合 —— 仍远强于固定 token，且这条路径
/// 在 Linux/macOS 上不该被走到。
pub fn generate_token() -> String {
    let mut bytes = [0u8; 16];
    let ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .is_ok();
    if !ok {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id() as u128;
        let mixed = nanos ^ (pid << 64) ^ (nanos.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        bytes = mixed.to_le_bytes();
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_default_generates_a_token() {
        let config =
            WebConfig::parse("127.0.0.1:8900", None, false, std::path::Path::new("/tmp")).unwrap();
        assert_eq!(config.listen.port(), 8900);
        // 默认启用：不给 --token 就自动生成（32 hex 字符），且两次生成不同
        let token = config.token.as_deref().expect("token is default-enabled");
        assert_eq!(token.len(), 32);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        let again =
            WebConfig::parse("127.0.0.1:8900", None, false, std::path::Path::new("/tmp")).unwrap();
        assert_ne!(config.token, again.token);
        let hosts = config.allowed_hosts();
        assert!(hosts.contains(&"127.0.0.1:8900".to_string()));
        assert!(hosts.contains(&"localhost:8900".to_string()));
    }

    #[test]
    fn without_token_disables_and_non_loopback_refuses() {
        // 回环上可以显式关闭
        let config =
            WebConfig::parse("127.0.0.1:8900", None, true, std::path::Path::new("/tmp")).unwrap();
        assert_eq!(config.token, None);
        // 非回环拒绝关闭：无鉴权对外不允许
        let error =
            WebConfig::parse("0.0.0.0:8900", None, true, std::path::Path::new("/tmp")).unwrap_err();
        assert_eq!(error.field.as_deref(), Some("web.token"));
        // 两个旗标同给是配置冲突
        let error = WebConfig::parse(
            "127.0.0.1:8900",
            Some("secret".into()),
            true,
            std::path::Path::new("/tmp"),
        )
        .unwrap_err();
        assert_eq!(error.field.as_deref(), Some("web.token"));
    }

    #[test]
    fn explicit_token_wins_and_non_loopback_needs_one() {
        let config = WebConfig::parse(
            "0.0.0.0:8900",
            Some("secret".into()),
            false,
            std::path::Path::new("/tmp"),
        )
        .unwrap();
        assert_eq!(config.token.as_deref(), Some("secret"));
        // 非回环时不接受 localhost 这种写法
        assert_eq!(config.allowed_hosts(), vec!["0.0.0.0:8900".to_string()]);
    }

    #[test]
    fn allowed_hosts_follow_the_configured_port() {
        // 这条是"别硬编码端口"的守门测试：换端口后 Host 校验必须跟着走
        let config =
            WebConfig::parse("127.0.0.1:9999", None, true, std::path::Path::new("/tmp")).unwrap();
        assert!(
            config
                .allowed_hosts()
                .iter()
                .all(|host| host.ends_with(":9999"))
        );
    }

    #[test]
    fn csp_forbids_inline_scripts_and_styles() {
        assert!(!CONTENT_SECURITY_POLICY.contains("unsafe-inline"));
        assert!(CONTENT_SECURITY_POLICY.contains("script-src 'self'"));
        assert!(CONTENT_SECURITY_POLICY.contains("style-src 'self'"));
    }

    #[test]
    fn generated_tokens_have_enough_entropy() {
        for _ in 0..8 {
            let token = generate_token();
            assert_eq!(token.len(), 32);
            assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }
}

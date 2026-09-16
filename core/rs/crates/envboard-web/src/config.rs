//! 工作台的配置与**安全模型**。
//!
//! 三条不可省的约束：
//!
//! 1. **默认只监听 `127.0.0.1`**；非本机监听必须给 token（不允许无鉴权对外）。
//! 2. **校验 `Host` 头**必须等于配置的监听地址 —— 防 DNS rebinding。
//! 3. **变更类路由必须带自定义头** —— 浏览器跨站简单请求带不了自定义头
//!    （fetch 会被 preflight 挡下），所以这一条就挡住了 CSRF。
//!
//! 这三条不是可选项：工作台能起进程、改规则，等于能改本机流量走向。

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
    pub fn parse(
        listen: &str,
        token: Option<String>,
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
        if !local {
            match token.as_deref() {
                Some(value) if !value.trim().is_empty() => {}
                _ => {
                    return Err(Error::invalid_config(
                        "web.token",
                        format!(
                            "refusing to listen on {} without a token: the workbench can start \
                             processes and rewrite rules, so unauthenticated exposure is not allowed",
                            listen.ip()
                        ),
                    ));
                }
            }
        }

        Ok(Self {
            listen,
            token,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_default_is_allowed_without_a_token() {
        let config =
            WebConfig::parse("127.0.0.1:8900", None, std::path::Path::new("/tmp")).unwrap();
        assert_eq!(config.listen.port(), 8900);
        let hosts = config.allowed_hosts();
        assert!(hosts.contains(&"127.0.0.1:8900".to_string()));
        assert!(hosts.contains(&"localhost:8900".to_string()));
    }

    #[test]
    fn non_loopback_requires_a_token() {
        let error =
            WebConfig::parse("0.0.0.0:8900", None, std::path::Path::new("/tmp")).unwrap_err();
        assert_eq!(error.field.as_deref(), Some("web.token"));

        let config = WebConfig::parse(
            "0.0.0.0:8900",
            Some("secret".into()),
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
            WebConfig::parse("127.0.0.1:9999", None, std::path::Path::new("/tmp")).unwrap();
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
}

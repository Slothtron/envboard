//! 工作台的配置与**安全模型**。
//!
//! 三条不可省的约束：
//!
//! 1. **鉴权按绑定地址定档**：回环监听（127.0.0.1/::1）默认**免鉴权** —— 本机即本机
//!    用户，token 挡不住同机进程，只添摩擦；`--token` 在任何监听上都可显式启用
//!    （裸给 = 自动生成随机值，启动日志打印 `dashboard: …/?token=…` 可点链接）。
//!    **非回环监听必须显式给 token，否则启动即 `invalid_config`** —— 工作台能起进程、
//!    改规则，不允许不知情地把这种能力暴露到网络上。
//! 2. **校验 `Host` 头**必须等于配置的监听地址 —— 防 DNS rebinding。这道门与 token
//!    无关，任何档位都不豁免。
//! 3. **变更类路由必须带自定义头** —— 浏览器跨站简单请求带不了自定义头
//!    （fetch 会被 preflight 挡下），所以这一条就挡住了 CSRF。回环免鉴权时，
//!    这是仍然立着的第二道闸：恶意网页能猜中 127.0.0.1:8900，但递不进这条头。

use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use envboard_engine::Error;

/// 变更类请求必须携带的头（前端统一带）。
pub const REQUEST_HEADER: &str = "x-envboard-request";
/// token 头（启用鉴权时才要求）。
pub const TOKEN_HEADER: &str = "x-envboard-token";

/// CSP：**不含任何 `unsafe-inline`** —— 资产是外置的 Vite 构建产物（dist）。
///
/// v1 的教训是"内联脚本被宿主 CSP 拒绝，而 curl 断言查不出来"；我们自己的宿主更要把
/// 这条钉死：CSP 与资产形态必须一致，否则样式或脚本会静默失效。
///
/// `style-src` 上唯一的 hash 豁免是给 react-aria 的运行时注入：pressable 组件首挂载
/// 时注入一张固定内容的 `<style>`（`[data-react-aria-pressable]{touch-action:…}`，
/// 88 B）。内容恒定 → hash 恒定；react-aria 升级若改动内容，live 层的
/// "零 CSP 报错"断言会红（升级时同步此 hash）。
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; \
     style-src 'self' 'sha256-38RhXrc7EdReTKsOm23ZPOCUgniTUUcjky8QOOrQx6o='; \
     img-src 'self' data:; \
     connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

#[derive(Debug, Clone)]
pub struct WebConfig {
    pub listen: SocketAddr,
    /// 启用则每个端点（两个静态资产除外）都要过 token；`None` = 免鉴权。
    pub token: Option<String>,
    /// 生成 `proxy_command` 用的状态目录（仅用于展示与日志）。
    pub state_dir: PathBuf,
}

impl WebConfig {
    /// 解析 `--listen`（`host:port`）并做安全校验。
    ///
    /// token 语义（`token` 参数来自 clap 的 `--token [T]`：不给 = `None`，
    /// 裸给 = `Some("")`，带值 = `Some(T)`）：
    ///
    /// * `Some(非空)` → 原样启用（trim 去空白）；
    /// * `Some(空串)` → 自动生成随机 token 并启用（见 [`generate_token`]）；
    /// * `None` 且**回环**监听 → 免鉴权（默认）；
    /// * `None` 且**非回环**监听 → 启动即失败（`invalid_config`，字段 `web.token`）——
    ///   对外暴露鉴权必须是知情动作，不给"忘了开就裸奔"留路径。
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

        let token = match token {
            Some(value) if !value.trim().is_empty() => Some(value.trim().to_string()),
            Some(_) => Some(generate_token()),
            None if is_loopback(listen.ip()) => None,
            None => {
                return Err(Error::invalid_config(
                    "web.token",
                    format!(
                        "non-loopback listen ({}) requires an explicit token: pass \"--token <T>\"",
                        listen.ip()
                    ),
                ));
            }
        };

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

    fn tmp() -> std::path::PathBuf {
        std::path::Path::new("/tmp").to_path_buf()
    }

    #[test]
    fn loopback_is_open_by_default() {
        // 新默认：回环 + 不给 --token = 免鉴权。
        let config = WebConfig::parse("127.0.0.1:8900", None, &tmp()).unwrap();
        assert_eq!(config.listen.port(), 8900);
        assert_eq!(config.token, None);
        let hosts = config.allowed_hosts();
        assert!(hosts.contains(&"127.0.0.1:8900".to_string()));
        assert!(hosts.contains(&"localhost:8900".to_string()));
    }

    #[test]
    fn explicit_token_enables_auth_on_any_bind() {
        let config = WebConfig::parse("127.0.0.1:8900", Some("secret".into()), &tmp()).unwrap();
        assert_eq!(config.token.as_deref(), Some("secret"));
        let config = WebConfig::parse("0.0.0.0:8900", Some("secret".into()), &tmp()).unwrap();
        assert_eq!(config.token.as_deref(), Some("secret"));
        // 非回环时不接受 localhost 这种写法
        assert_eq!(config.allowed_hosts(), vec!["0.0.0.0:8900".to_string()]);
    }

    #[test]
    fn bare_token_autogenerates() {
        // `--token` 裸给（空串）→ 自动生成 32 hex，且每次不同。
        let config = WebConfig::parse("127.0.0.1:8900", Some(String::new()), &tmp()).unwrap();
        let token = config.token.clone().expect("bare --token enables auth");
        assert_eq!(token.len(), 32);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        let again = WebConfig::parse("127.0.0.1:8900", Some("  ".into()), &tmp()).unwrap();
        assert_ne!(config.token, again.token);
        // 非回环同样认这个形态：显式裸给 = 知情启用
        assert!(WebConfig::parse("0.0.0.0:8900", Some(String::new()), &tmp()).is_ok());
    }

    #[test]
    fn non_loopback_without_token_refuses_to_start() {
        // 对外暴露必须显式给 token —— 不给就拒启动，错误要指路。
        for bind in ["0.0.0.0:8900", "192.168.1.7:8900"] {
            let error = WebConfig::parse(bind, None, &tmp()).unwrap_err();
            assert_eq!(error.field.as_deref(), Some("web.token"));
            assert!(error.message.contains("--token"));
        }
    }

    #[test]
    fn allowed_hosts_follow_the_configured_port() {
        // 这条是"别硬编码端口"的守门测试：换端口后 Host 校验必须跟着走
        let config = WebConfig::parse("127.0.0.1:9999", None, &tmp()).unwrap();
        assert!(
            config
                .allowed_hosts()
                .iter()
                .all(|host| host.ends_with(":9999"))
        );
    }

    #[test]
    fn bad_listen_still_fails_loudly() {
        let error = WebConfig::parse("nope", None, &tmp()).unwrap_err();
        assert_eq!(error.field.as_deref(), Some("web.listen"));
        let error = WebConfig::parse("127.0.0.1:0", None, &tmp()).unwrap_err();
        assert_eq!(error.field.as_deref(), Some("web.listen"));
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

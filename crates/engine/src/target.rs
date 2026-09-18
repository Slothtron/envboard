//! 建连目标描述符：基础配置播下种子，由 hosts 规则修订，引擎最终按
//! 到达态执行 —— 这是"能力都发生在建连决策"这件事的唯一接缝。
//!
//! v2 的对应物是注入器里"改 data.server.address + 按域名换 TLS context"两处
//! 钩子补丁；在这里它们变成同一个结构体的两个字段。
//! 字段语义与不变量：
//!
//! * authority：客户端请求的 authority（Host / CONNECT 目标）。只读基准。
//! * sni：上游 TLS 的 SNI 基准（authority 的主机名；IP 字面量则 None）。
//!   改写 SNI 是显式能力，本版本不开放（见本文件末尾的扩展位说明）。
//! * resolved_addr：实际连接地址。None = 按 authority 解析 DNS。
//! * tls_policy：Verify（默认）或 Insecure —— Insecure 只能由 insecure_hosts
//!   的精确命中产生（判定复用 rules::insecure_matches），
//!   没有任何"全局关校验"的表达路径。
//! * chained_proxy：上游二级代理位。本版本恒 None，形状先行（后续由扩展
//!   填充），避免到时改接口惊动全部实现。

use std::net::SocketAddr;

use crate::text::parse_ip_literal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsPolicy {
    Verify,
    /// 仅当 insecure_hosts 精确命中（SNI 优先、无 SNI 时回退上连地址）才允许。
    Insecure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxySpec {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectTarget {
    pub authority: String,
    pub host: String,
    pub port: u16,
    pub sni: Option<String>,
    pub resolved_addr: Option<SocketAddr>,
    pub tls_policy: TlsPolicy,
    pub chained_proxy: Option<ProxySpec>,
}

impl ConnectTarget {
    /// 基础层的种子目标（hosts 规则在其上修订）。
    ///
    /// host 是已归一化的小写主机名；IP 字面量时 sni 为 None（SNI 对 IP 无意义，
    /// 校验走 IP SAN 路径），resolved_addr 直接落位。
    pub fn seed(host: &str, port: u16) -> Self {
        let ip = parse_ip_literal(host);
        ConnectTarget {
            authority: match ip {
                Some(addr) if addr.is_ipv6() => format!("[{host}]:{port}"),
                _ => format!("{host}:{port}"),
            },
            host: host.to_string(),
            port,
            sni: if ip.is_some() {
                None
            } else {
                Some(host.to_string())
            },
            resolved_addr: ip.map(|ip| SocketAddr::new(ip, port)),
            tls_policy: TlsPolicy::Verify,
            chained_proxy: None,
        }
    }

    /// 放宽判定后的最终策略修订。
    pub fn apply_insecure_match(&mut self, matched: bool) {
        if matched {
            self.tls_policy = TlsPolicy::Insecure;
        }
    }
}

// SNI 改写（Mirror 的 SNI/Host/证书 CN 三策略）与二级代理填充是后续
// 扩展位；本版本的种子与判定语义已把接口钉死，扩展只加不改。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_follow_the_v2_field_semantics() {
        let named = ConnectTarget::seed("svc.test", 8443);
        assert_eq!(named.authority, "svc.test:8443");
        assert_eq!(named.sni.as_deref(), Some("svc.test"));
        assert_eq!(named.resolved_addr, None);
        assert_eq!(named.tls_policy, TlsPolicy::Verify);

        let v4 = ConnectTarget::seed("10.0.0.7", 80);
        assert_eq!(v4.resolved_addr.unwrap().ip().to_string(), "10.0.0.7");
        assert_eq!(v4.resolved_addr.unwrap().port(), 80);
        assert_eq!(v4.sni, None);

        let v6 = ConnectTarget::seed("::1", 443);
        assert_eq!(v6.authority, "[::1]:443");
        assert_eq!(v6.resolved_addr.unwrap().ip().to_string(), "::1");
    }

    #[test]
    fn insecure_only_ever_flips_on_match() {
        let mut target = ConnectTarget::seed("svc.test", 443);
        target.apply_insecure_match(false);
        assert_eq!(target.tls_policy, TlsPolicy::Verify);
        target.apply_insecure_match(true);
        assert_eq!(target.tls_policy, TlsPolicy::Insecure);
    }
}

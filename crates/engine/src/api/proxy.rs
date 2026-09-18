//! v3 共享词汇表 —— 编排层（manager / web / cli）只认识这里的类型。
//!
//! 引擎本体在本 crate 的 engine 模块（进程内实例），抽象面即 ProxyEngine
//! trait；这里只留**宿主无关的共享值类型**：监听地址、core 信息、能力声明。
//! 能力矩阵见 spec/capabilities.md 的「引擎能力矩阵」。
//!
//! 两条纪律：
//!
//! * 实例配置只有**一等字段**这一条路（监听地址、放行域名、代理凭据）。历史上曾有
//!   一个任意 options 透传通道，它让"放宽上游证书校验"这类安全控制可以绕过契约 ——
//!   已删除，禁止回来；
//! * 能力声明是"这个核心支持什么"的唯一通道：管理面禁止 if core == "..." 式的
//!   判断，不支持的能力必须显式声明 false（缺能力以 invalid_config 响亮失败，
//!   禁止静默降级）。

use std::fmt;
use std::net::{IpAddr, Ipv4Addr};

use serde::{Deserialize, Serialize};

/// 默认监听地址：只在 core 定义一次，适配器禁止再来一份。
pub const DEFAULT_LISTEN_HOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// 环境的监听地址 —— 环境的**外部身份**（一个环境 = 一个实例 = 一个端口）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Listen {
    pub host: IpAddr,
    pub port: u16,
}

impl Listen {
    pub fn new(host: IpAddr, port: u16) -> Self {
        Self { host, port }
    }

    pub fn localhost(port: u16) -> Self {
        Self {
            host: DEFAULT_LISTEN_HOST,
            port,
        }
    }
}

impl fmt::Display for Listen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.host {
            // IPv6 要加方括号才是合法的 host:port 写法
            IpAddr::V6(addr) => write!(f, "[{addr}]:{}", self.port),
            IpAddr::V4(addr) => write!(f, "{addr}:{}", self.port),
        }
    }
}

/// core 的名字与版本，用于展示与兼容性判断。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreInfo {
    pub name: String,
    pub version: String,
}

/// 这个 core 支持哪些能力（矩阵见 spec/capabilities.md 的「引擎能力矩阵」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CoreCapabilities {
    /// 监听指定端口；EADDRINUSE 以 port_conflict 状态如实上报，绝不自动换端口。
    pub listen: bool,
    /// 按 SNI 现签叶子证书（中间人）。
    pub dynamic_certs: bool,
    /// 改写连接目标（hosts 规则；只改"连到哪"，不动请求内容与 Host 头）。
    pub rewrite_upstream: bool,
    /// 按域名放宽上游证书校验（insecure_hosts，精确匹配、无通配）。
    pub per_domain_insecure: bool,
    /// 全环境共用一张 CA（confdir 里与 mitmproxy 兼容的文件形态）。
    pub shared_ca: bool,
    /// 数据面只实现 HTTP/1.1（ALPN 只协商 h1；强制 h2 的客户端会失败）。
    /// 这是**登记过的已知限制**，不是静默降级 —— README 必须随包披露。
    pub http1_only: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_display_quotes_ipv6() {
        let v6 = Listen::new("::1".parse().unwrap(), 16_300);
        assert_eq!(v6.to_string(), "[::1]:16300");
        let v4 = Listen::localhost(16_301);
        assert_eq!(v4.to_string(), "127.0.0.1:16301");
    }

    #[test]
    fn listen_is_the_serialized_identity_shape() {
        let raw = serde_json::to_string(&Listen::localhost(8080)).unwrap();
        assert_eq!(raw, r#"{"host":"127.0.0.1","port":8080}"#);
        let back: Listen = serde_json::from_str(&raw).unwrap();
        assert_eq!(back, Listen::localhost(8080));
    }
}

//! 文本层面的共用解析 —— IP 字面量。
//!
//! domain（`listen.host`）与 rules（规则里的 ip token）都要判断"这是不是一个裸 IP
//! 字面量"，所以定义在 core-api 里**只此一份**：两处各写一遍迟早会分叉，
//! 而分叉的表现是"某个地址在环境里合法、在规则文件里非法"这种难查的不一致。

use std::net::IpAddr;

/// 解析裸 IP 字面量：接受 IPv4、IPv6，以及 `[::1]` 形式。
///
/// **刻意不接受**：主机名、`ip:port`、带 zone id 的 IPv6（`fe80::1%eth0`）、
/// 前导零形式的 IPv4（`010.0.0.1`）。前三条是"这不是一个地址"，
/// 最后一条是"看起来像但不是"——标准解析器会拒绝，我们跟着拒绝。
///
/// 注意：返回值是**原始文本**（剥掉方括号后），不做规范化。
/// 规则文件的 entries 因此保留用户写的拼法（`2001:0db8::1` 不会被改写成
/// `2001:db8::1`）——这是 v1 的语义，见 `core/spec/rules.md` §3.2。
pub fn parse_ip_literal(raw: &str) -> Option<IpAddr> {
    let trimmed = raw.trim();
    let bare = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(trimmed);
    bare.parse::<IpAddr>().ok()
}

/// 是不是裸 IP 字面量（规则文件解析器靠它判断 `ip host` 还是 `host ip` 写法）。
pub fn is_ip_literal(raw: &str) -> bool {
    parse_ip_literal(raw).is_some()
}

/// 剥掉 IPv6 的方括号（`[::1]` → `::1`），其它原样返回。
pub fn strip_brackets(raw: &str) -> &str {
    let trimmed = raw.trim();
    trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ipv4_ipv6_and_brackets() {
        assert!(is_ip_literal("10.0.0.1"));
        assert!(is_ip_literal("2001:db8::1"));
        assert!(is_ip_literal("[2001:db8::1]"));
        assert_eq!(parse_ip_literal("[::1]").unwrap().to_string(), "::1");
    }

    #[test]
    fn rejects_hostnames_ports_and_malformed_octets() {
        for raw in [
            "api.example.com",
            "localhost",
            "127.0.0.1:8080",
            "300.1.2.3",
            "010.0.0.1",
        ] {
            assert!(!is_ip_literal(raw), "{raw} must not be an IP literal");
        }
    }

    #[test]
    fn rejects_ipv6_zone_id() {
        // Python 的 ipaddress 接受 `fe80::1%eth0`，Rust 的解析器不接受 ——
        // 契约按后者（见 core/spec/rules.md §3.2），所以两侧都要拒绝它。
        assert!(!is_ip_literal("fe80::1%eth0"));
    }
}

//! hosts 规则改写（引擎 connect 路径的内部直调步骤）。
//!
//! 与 v2 注入器的 `server_connect` 逐项等价：只改连接目标（resolved_addr），
//! 不动 Host 头、不动 SNI 基准、不动 authority。命中规则表且目标不是 IP
//! 字面量 → 响亮失败（fail-closed 502），而不是按名连接。

use std::collections::BTreeMap;

use crate::rules::{normalize_host, parse_hosts_text};
use crate::target::ConnectTarget;

/// 从 hosts 风格文本装配查找表（解析永不失败：非法行进 skipped 统计，
/// 与规则导入面同一套语义）。
pub fn rules_table(rules_text: Option<&str>) -> BTreeMap<String, String> {
    match rules_text {
        Some(text) => parse_hosts_text(text).entries,
        None => BTreeMap::new(),
    }
}

/// connect 阶段修订建连描述符。Err 的 detail 面向 502 正文与终局日志。
pub fn apply(table: &BTreeMap<String, String>, target: &mut ConnectTarget) -> Result<(), String> {
    let Some(ip_text) = table.get(&normalize_host(&target.host)) else {
        return Ok(());
    };
    match crate::text::parse_ip_literal(ip_text) {
        Some(ip) => {
            target.resolved_addr = Some(std::net::SocketAddr::new(ip, target.port));
            Ok(())
        }
        // 导入面只收 IP 字面量；走到这里说明规则表被旁路塞了脏数据。
        None => Err(format!("rule target {ip_text:?} is not an IP literal")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hosts_rules_rewrites_only_the_address() {
        let table = rules_table(Some("10.0.0.7 svc.a\n"));
        let mut hit = ConnectTarget::seed("svc.a", 8443);
        apply(&table, &mut hit).unwrap();
        assert_eq!(hit.resolved_addr.unwrap().ip().to_string(), "10.0.0.7");
        assert_eq!(hit.resolved_addr.unwrap().port(), 8443, "端口跟原请求");
        assert_eq!(hit.sni.as_deref(), Some("svc.a"), "SNI 基准不动");
        assert_eq!(hit.authority, "svc.a:8443", "authority 不动");

        let mut miss = ConnectTarget::seed("svc.b", 8443);
        apply(&table, &mut miss).unwrap();
        assert_eq!(miss.resolved_addr, None, "未覆盖 = 不改写，不是拦截");
    }

    #[tokio::test]
    async fn dirty_rule_target_is_loud() {
        let table = BTreeMap::from([("svc.a".to_string(), "not-an-ip.example".to_string())]);
        let mut target = ConnectTarget::seed("svc.a", 80);
        let error = apply(&table, &mut target).unwrap_err();
        assert!(error.contains("not an IP literal"), "{error}");
    }
}

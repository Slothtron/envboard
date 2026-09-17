//! 内置插件：hosts-rules 与 request-log。
//!
//! 它们走的是与未来扩展插件完全相同的接口 —— 这是插件契约被产品功能自举
//! 验证的地方，不是"内部人特权通道"：hosts-rules 在 connect 阶段修订
//! ConnectTarget（与 v2 注入器的 \`server_connect\` 逐项等价：只改连接目标，
//! 不动 Host 头、不动 SNI 基准），request-log 在 log 阶段消费终局记录。

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::plugin::{LogRecord, LogWriter, Plugin, PluginError};
use crate::target::ConnectTarget;
use envboard_rules::{normalize_host, parse_hosts_text};

/// hosts 规则改写（消费环境的 \`rules\` 一等字段；字段形状不变）。
#[derive(Debug)]
pub struct HostsRules {
    table: BTreeMap<String, String>,
}

impl HostsRules {
    /// 从 hosts 风格文本装配（解析永不失败：非法行进 skipped 统计，与规则
    /// 导入面同一套语义）。
    pub fn from_text(rules_text: Option<&str>) -> Self {
        let table = match rules_text {
            Some(text) => parse_hosts_text(text).entries,
            None => BTreeMap::new(),
        };
        HostsRules { table }
    }

    pub fn from_table(table: BTreeMap<String, String>) -> Self {
        HostsRules { table }
    }

    pub fn entries(&self) -> &BTreeMap<String, String> {
        &self.table
    }
}

#[async_trait]
impl Plugin for HostsRules {
    fn id(&self) -> &'static str {
        "hosts-rules"
    }

    async fn on_connect(&self, target: &mut ConnectTarget) -> Result<(), PluginError> {
        let Some(ip_text) = self.table.get(&normalize_host(&target.host)) else {
            return Ok(());
        };
        match envboard_core_api::text::parse_ip_literal(ip_text) {
            Some(ip) => {
                target.resolved_addr = Some(std::net::SocketAddr::new(ip, target.port));
                Ok(())
            }
            // 导入面只收 IP 字面量；走到这里说明规则表被旁路塞了脏数据。
            // 响亮失败（fail-closed 502）而不是按名连接。
            None => Err(PluginError::new(format!(
                "rule target {ip_text:?} is not an IP literal"
            ))),
        }
    }
}

/// 请求日志（默认启用；\`on_error = Bypass\` 由装配器设定 —— 观测失败不得阻断流量）。
#[derive(Debug)]
pub struct RequestLog {
    writer: Arc<dyn LogWriter>,
}

impl RequestLog {
    pub fn new(writer: Arc<dyn LogWriter>) -> Self {
        RequestLog { writer }
    }

    /// 与 v2 实例日志行对账用的稳定格式（字段增减都走契约）。
    pub fn format(record: &LogRecord) -> String {
        let seconds = record.timestamp_unix_ms / 1000;
        let (h, m, s) = ((seconds / 3600) % 24, (seconds / 60) % 60, seconds % 60);
        let millis = record.timestamp_unix_ms % 1000;
        let mut line = format!(
            "[{h:02}:{m:02}:{s:02}.{millis:03}] {} {}{} -> {} (req {}B resp {}B {}ms",
            record.method,
            record.authority,
            record.path,
            record.status,
            record.request_bytes,
            record.response_bytes,
            record.duration_ms,
        );
        if record.rewritten {
            line.push_str(", rewritten");
        }
        if record.insecure {
            line.push_str(", insecure");
        }
        if let Some(error) = &record.error {
            line.push_str(&format!(", error: {error}"));
        }
        line.push(')');
        line
    }
}

impl Plugin for RequestLog {
    fn id(&self) -> &'static str {
        "request-log"
    }

    fn on_log(&self, event: Arc<LogRecord>) {
        // 契约：on_log 永不外溢 —— 写失败丢行，但绝不 panic、绝不影响流量。
        self.writer.write_line(&RequestLog::format(&event));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::LogRecord;
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct Capture(Mutex<Vec<String>>);

    impl LogWriter for Capture {
        fn write_line(&self, line: &str) {
            self.0.lock().unwrap().push(line.to_string());
        }
    }

    #[tokio::test]
    async fn hosts_rules_rewrites_only_the_address() {
        let rules = HostsRules::from_text(Some("10.0.0.7 svc.a\n"));
        let mut hit = ConnectTarget::seed("svc.a", 8443);
        rules.on_connect(&mut hit).await.unwrap();
        assert_eq!(hit.resolved_addr.unwrap().ip().to_string(), "10.0.0.7");
        assert_eq!(hit.resolved_addr.unwrap().port(), 8443, "端口跟原请求");
        assert_eq!(hit.sni.as_deref(), Some("svc.a"), "SNI 基准不动");
        assert_eq!(hit.authority, "svc.a:8443", "authority 不动");

        let mut miss = ConnectTarget::seed("svc.b", 8443);
        rules.on_connect(&mut miss).await.unwrap();
        assert_eq!(miss.resolved_addr, None, "未覆盖 = 不改写，不是拦截");
    }

    #[tokio::test]
    async fn dirty_rule_target_is_loud() {
        let rules = HostsRules::from_table(BTreeMap::from([(
            "svc.a".to_string(),
            "not-an-ip.example".to_string(),
        )]));
        let mut target = ConnectTarget::seed("svc.a", 80);
        let error = rules.on_connect(&mut target).await.unwrap_err();
        assert!(
            error.detail.contains("not an IP literal"),
            "{}",
            error.detail
        );
    }

    #[test]
    fn request_log_formats_the_stable_line() {
        let capture = Arc::new(Capture::default());
        let logger = RequestLog::new(capture.clone());
        let record = LogRecord {
            timestamp_unix_ms: 3_723_789, // 1 小时 2 分 3 秒 + 789ms（测试内自洽的固定时刻）
            method: "GET".into(),
            authority: "svc.a:8443".into(),
            path: "/x".into(),
            status: 502,
            request_bytes: 0,
            response_bytes: 12,
            duration_ms: 34,
            rewritten: true,
            insecure: false,
            error: Some("upstream connect failed".into()),
        };
        logger.on_log(Arc::new(record.clone()));
        let lines = capture.0.lock().unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0],
            "[01:02:03.789] GET svc.a:8443/x -> 502 (req 0B resp 12B 34ms, rewritten, error: upstream connect failed)"
        );
    }
}

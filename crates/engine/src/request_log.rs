//! 请求终局日志（引擎请求终点的内部直调步骤）。
//!
//! 每个终局请求一行、稳定格式（与 v2 实例日志对账；字段增减都走契约）。
//! 经有界总线投递：写失败丢行，但绝不 panic、绝不影响流量（数据面永不
//! 阻塞在写日志上）。

use std::sync::Arc;

use crate::ports::LineWriter;

/// 终局记录：这一请求的全部可见事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord {
    pub timestamp_unix_ms: u64,
    pub method: String,
    pub authority: String,
    pub path: String,
    pub status: u16,
    pub request_bytes: usize,
    pub response_bytes: usize,
    pub duration_ms: u64,
    /// 连接目标被 hosts 规则改写（resolved_addr 从 None 变为 Some）。
    pub rewritten: bool,
    /// 上游 TLS 是严格校验还是名单放宽。
    pub insecure: bool,
    /// 请求以引擎错误告终时的原因。
    pub error: Option<String>,
}

impl LogRecord {
    pub fn format(&self) -> String {
        let seconds = self.timestamp_unix_ms / 1000;
        let (h, m, s) = ((seconds / 3600) % 24, (seconds / 60) % 60, seconds % 60);
        let millis = self.timestamp_unix_ms % 1000;
        let mut line = format!(
            "[{h:02}:{m:02}:{s:02}.{millis:03}] {} {}{} -> {} (req {}B resp {}B {}ms",
            self.method,
            self.authority,
            self.path,
            self.status,
            self.request_bytes,
            self.response_bytes,
            self.duration_ms,
        );
        if self.rewritten {
            line.push_str(", rewritten");
        }
        if self.insecure {
            line.push_str(", insecure");
        }
        if let Some(error) = &self.error {
            line.push_str(&format!(", error: {error}"));
        }
        line.push(')');
        line
    }

    /// 投递进日志出口（永不外溢：写失败丢行）。
    pub fn emit(self, writer: &Arc<dyn LineWriter>) {
        writer.write_line(&self.format());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct Capture(Mutex<Vec<String>>);

    impl LineWriter for Capture {
        fn write_line(&self, line: &str) {
            self.0.lock().unwrap().push(line.to_string());
        }
    }

    #[test]
    fn request_log_formats_the_stable_line() {
        let capture = Arc::new(Capture::default());
        let writer: Arc<dyn LineWriter> = capture.clone();
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
        record.clone().emit(&writer);
        let lines = capture.0.lock().unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0],
            "[01:02:03.789] GET svc.a:8443/x -> 502 (req 0B resp 12B 34ms, rewritten, error: upstream connect failed)"
        );
    }
}

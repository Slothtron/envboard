//! 数据面请求轨迹事件词汇。
//!
//! 一条 HTTP 请求的轨迹由 `request_id` 贯穿关联（业务 id 由引擎拥有并跨
//! 进程稳定，读者只做确定性关联）。事件按阶段成对出现：`request/start` 起、
//! `request/end` 止；中间的事件族按需追加。**正文与头部不入轨迹**：轨迹
//! 记录"发生了什么"，内容排障看 `<env>.log` 与抓包。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 静态只读注册表：数据面事件的全部类型名。新增变体必须同步登记这里。
pub static KNOWN_DATA_KINDS: &[&str] = &[
    "request/start",
    "request/upstream",
    "request/body",
    "response/head",
    "request/end",
    "custom",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum DataEvent {
    /// 请求起点：分流结果与 TLS 档位在此定型。
    #[serde(rename = "request/start")]
    RequestStart {
        request_id: u64,
        method: String,
        authority: String,
        path: String,
        /// 上游 TLS 的 SNI 基准（IP 直连等无 SNI 场景为 None）。
        sni: Option<String>,
        /// 上游证书校验是否被 insecure_hosts 放宽。
        insecure: bool,
    },
    /// hosts 规则改写结果。未命中规则不发这条（resolved = authority 自解析）。
    #[serde(rename = "request/upstream")]
    RequestUpstream {
        request_id: u64,
        resolved_addr: String,
        rewritten: bool,
    },
    /// 请求体规模（仅字节数，不进内容）。
    #[serde(rename = "request/body")]
    RequestBody { request_id: u64, bytes: u64 },
    /// 响应头定型（含 502 失败；正文规模一并记录）。
    #[serde(rename = "response/head")]
    ResponseHead {
        request_id: u64,
        status: u16,
        bytes: u64,
    },
    /// 请求终点。`error` 非空 = 这条请求以引擎错误告终（status 恒 502）。
    #[serde(rename = "request/end")]
    RequestEnd {
        request_id: u64,
        duration_ms: u64,
        error: Option<String>,
    },
    /// 保留扩展位，语义同控制面的 `custom`。
    #[serde(rename = "custom")]
    Custom { kind: String, payload: Value },
}

impl DataEvent {
    pub fn kind(&self) -> &'static str {
        match self {
            DataEvent::RequestStart { .. } => "request/start",
            DataEvent::RequestUpstream { .. } => "request/upstream",
            DataEvent::RequestBody { .. } => "request/body",
            DataEvent::ResponseHead { .. } => "response/head",
            DataEvent::RequestEnd { .. } => "request/end",
            DataEvent::Custom { .. } => "custom",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::Envelope;

    #[test]
    fn known_kinds_cover_every_variant() {
        let samples = [
            DataEvent::RequestStart {
                request_id: 1,
                method: "GET".into(),
                authority: "svc.a:80".into(),
                path: "/".into(),
                sni: None,
                insecure: false,
            },
            DataEvent::RequestUpstream {
                request_id: 1,
                resolved_addr: "10.0.0.7:80".into(),
                rewritten: true,
            },
            DataEvent::RequestBody {
                request_id: 1,
                bytes: 0,
            },
            DataEvent::ResponseHead {
                request_id: 1,
                status: 200,
                bytes: 12,
            },
            DataEvent::RequestEnd {
                request_id: 1,
                duration_ms: 5,
                error: None,
            },
            DataEvent::Custom {
                kind: "x/y".into(),
                payload: Value::Null,
            },
        ];
        assert_eq!(samples.len(), KNOWN_DATA_KINDS.len());
        for event in samples {
            assert!(KNOWN_DATA_KINDS.contains(&event.kind()));
        }
    }

    #[test]
    fn a_request_trajectory_is_one_request_id() {
        let start = Envelope::new(
            1,
            1,
            DataEvent::RequestStart {
                request_id: 7,
                method: "GET".into(),
                authority: "svc.a:8443".into(),
                path: "/x".into(),
                sni: Some("svc.a".into()),
                insecure: false,
            },
        );
        let line = serde_json::to_string(&start).unwrap();
        let back: Envelope<DataEvent> = serde_json::from_str(&line).unwrap();
        assert_eq!(back.event.kind(), "request/start");
        assert!(
            line.contains(r#""request_id":7"#),
            "request_id 必须原样序列化: {line}"
        );
    }
}

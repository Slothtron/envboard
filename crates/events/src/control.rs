//! 控制面审计事件词汇。
//!
//! 覆盖环境生命周期、期望状态变更、规则导入、引擎应用与 reconcile 结果。
//! 规则内容**不入事件**（只记 `name` + `hash`）：正文随 `<rules_dir>` 落盘，
//! 事件负责"何时、谁、结果如何"的审计问题。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 静态只读注册表：控制面事件的全部类型名。新增变体必须同步登记这里。
pub static KNOWN_CONTROL_KINDS: &[&str] = &[
    "environment/created",
    "environment/updated",
    "environment/deleted",
    "rules/imported",
    "rules/deleted",
    "engine/applied",
    "engine/rejected",
    "instance/reconciled",
    "custom",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ControlEvent {
    #[serde(rename = "environment/created")]
    EnvironmentCreated {
        name: String,
        listen: String,
    },
    /// 只记被 PATCH 改动的字段名清单（值在 state 里，不在事件里）。
    #[serde(rename = "environment/updated")]
    EnvironmentUpdated {
        name: String,
        fields: Vec<String>,
    },
    #[serde(rename = "environment/deleted")]
    EnvironmentDeleted { name: String },
    /// 规则正文不入事件：hash 可对账（与账本里那份正文比对）。
    #[serde(rename = "rules/imported")]
    RulesImported {
        name: String,
        rules_name: String,
        rules_sha256: String,
    },
    #[serde(rename = "rules/deleted")]
    RulesDeleted {
        name: String,
        rules_name: String,
    },
    /// 配置热应用成功（回执语义）。
    #[serde(rename = "engine/applied")]
    EngineApplied {
        name: String,
        config_hash: String,
        epoch: u64,
    },
    /// 配置被整套拒绝（invalid_config）；旧快照继续服务。
    #[serde(rename = "engine/rejected")]
    EngineRejected {
        name: String,
        reason: String,
    },
    /// reconcile 决策落地：状态字面量（starting/running/failed/...）。
    #[serde(rename = "instance/reconciled")]
    InstanceReconciled {
        name: String,
        from: String,
        to: String,
    },
    /// 保留扩展位：新事件类型落地前的临时通道。`kind` 必须带域前缀
    /// （如 `web/custom-note`），载荷必须无损 JSON。
    #[serde(rename = "custom")]
    Custom { kind: String, payload: Value },
}

impl ControlEvent {
    pub fn kind(&self) -> &'static str {
        match self {
            ControlEvent::EnvironmentCreated { .. } => "environment/created",
            ControlEvent::EnvironmentUpdated { .. } => "environment/updated",
            ControlEvent::EnvironmentDeleted { .. } => "environment/deleted",
            ControlEvent::RulesImported { .. } => "rules/imported",
            ControlEvent::RulesDeleted { .. } => "rules/deleted",
            ControlEvent::EngineApplied { .. } => "engine/applied",
            ControlEvent::EngineRejected { .. } => "engine/rejected",
            ControlEvent::InstanceReconciled { .. } => "instance/reconciled",
            ControlEvent::Custom { .. } => "custom",
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
            ControlEvent::EnvironmentCreated {
                name: "a".into(),
                listen: "127.0.0.1:1".into(),
            },
            ControlEvent::EnvironmentUpdated {
                name: "a".into(),
                fields: vec!["description".into()],
            },
            ControlEvent::EnvironmentDeleted { name: "a".into() },
            ControlEvent::RulesImported {
                name: "a".into(),
                rules_name: "r".into(),
                rules_sha256: "x".into(),
            },
            ControlEvent::RulesDeleted {
                name: "a".into(),
                rules_name: "r".into(),
            },
            ControlEvent::EngineApplied {
                name: "a".into(),
                config_hash: "h".into(),
                epoch: 1,
            },
            ControlEvent::EngineRejected {
                name: "a".into(),
                reason: "bad".into(),
            },
            ControlEvent::InstanceReconciled {
                name: "a".into(),
                from: "stopped".into(),
                to: "running".into(),
            },
            ControlEvent::Custom {
                kind: "web/note".into(),
                payload: Value::Null,
            },
        ];
        assert_eq!(samples.len(), KNOWN_CONTROL_KINDS.len());
        for event in samples {
            assert!(
                KNOWN_CONTROL_KINDS.contains(&event.kind()),
                "{} 未登记进 KNOWN_CONTROL_KINDS",
                event.kind()
            );
        }
    }

    #[test]
    fn envelope_serializes_the_tagged_shape() {
        let envelope = Envelope::new(
            1,
            1_700_000_000_000,
            ControlEvent::EnvironmentCreated {
                name: "dev".into(),
                listen: "127.0.0.1:9000".into(),
            },
        );
        let line = serde_json::to_string(&envelope).unwrap();
        assert!(
            line.contains(r#""type":"environment/created""#)
                && line.contains(r#""seq":1"#)
                && !line.contains("ignorable"),
            "{line}"
        );
        let back: Envelope<ControlEvent> = serde_json::from_str(&line).unwrap();
        assert_eq!(back, envelope);
    }
}

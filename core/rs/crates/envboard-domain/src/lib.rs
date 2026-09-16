//! envboard 的领域模型 —— **纯逻辑**：不碰文件系统、网络、宿主。
//!
//! 契约见包根 `core/spec/`：
//! * 环境校验与合并 → `capabilities.md` §领域不变量、`fixtures/environment|merge/`
//! * 端口选择 → `capabilities.md` §端口分配、`fixtures/ports/`
//! * reconcile 决策 → `capabilities.md` §实例生命周期、`fixtures/lifecycle/`

pub mod environment;
pub mod ports;
pub mod reconcile;

pub use environment::{
    DESCRIPTION_MAX_CHARS, Environment, KNOWN_FIELDS, NAME_MAX_LEN, PATH, normalize_name,
    normalize_rules, parse_ip_literal, validate_name, validate_rules_name,
};
pub use ports::{
    DEFAULT_MAX_ATTEMPTS, DEFAULT_PORT_RANGE, PORT_FIELD, PortDecision, PortRequest, Warning,
    candidate_ports, occupancy_from, seed_from, select_port,
};
pub use reconcile::{
    Action, Desired, InstanceRecord, ReconcilePlan, ReconcileWarning, plan_reconcile,
};

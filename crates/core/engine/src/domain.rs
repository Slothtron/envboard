//! envboard 的领域模型 —— **纯逻辑**：不碰文件系统、网络、宿主（原 domain crate，
//! 并入 engine）。
//!
//! 契约住在各子模块文档，golden fixture 在 `tests/contract-tests/fixtures/`：
//! * 环境校验与合并 → [`environment`] 模块文档（fixture：environment/、merge/）
//! * 上游代理实体 → [`upstream`] 模块文档（fixture：proxy/）
//! * 端口选择 → [`ports`] 模块文档（fixture：ports/）
//! * reconcile 决策 → [`reconcile`] 模块文档（fixture：lifecycle/）

pub mod environment;
pub mod ports;
pub mod reconcile;
pub mod upstream;

pub use environment::{
    DESCRIPTION_MAX_CHARS, Environment, KNOWN_FIELDS, NAME_MAX_LEN, PATH, PROXY_PASSWORD_MAX_LEN,
    PROXY_USER_MAX_LEN, TOMBSTONE_FIELDS, normalize_name, normalize_rules, normalize_upstream,
    parse_ip_literal, validate_name, validate_rules_name, validate_upstream_name,
};
pub use ports::{
    DEFAULT_MAX_ATTEMPTS, DEFAULT_PORT_RANGE, PORT_FIELD, PortDecision, PortRequest, Warning,
    candidate_ports, occupancy_from, seed_from, select_port,
};
pub use reconcile::{Action, Desired, InstanceRecord, ReconcilePlan, plan_reconcile};
pub use upstream::{
    PASSWORD_MAX_LEN, UPSTREAM_KNOWN_FIELDS, UPSTREAM_PATH, USER_MAX_LEN, UpstreamProxy,
};

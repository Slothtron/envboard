//! envboard 环境管理器。
//!
//! 分层（依赖单向，`scripts/rust_dependency_lint.py` 强制）：
//!
//! ```text
//! envboard-core-api（trait / 契约词汇）
//!        ↑
//! envboard-domain（纯逻辑）  envboard-rules（纯逻辑）
//!        ↑                        ↑
//!        └──── envboard-manager ──┘
//!                     ↑
//!              ports（trait） + infra（真实实现）
//! ```
//!
//! 管理器只依赖 [`ProxyCore`](envboard_core_api::ProxyCore) 抽象，不认识 mitmproxy。

pub mod agent;
pub mod infra;
pub mod logs;
pub mod manager;
pub mod ports;
pub mod state;

pub use agent::{AGENT_CONFIG_VERSION, AgentConfig};
pub use manager::{EnvView, Manager, ReconcileReport};
pub use ports::{
    FilePort, MemoryFiles, MemoryPortProbe, MemoryProcessTable, MemoryStateRepo, PortProbe,
    ProcessTable, StateRepo,
};
pub use state::{
    ConfigSeal, InstanceLock, JsonFileStateRepo, ManagerConfig, PersistedState, STATE_VERSION,
    StoredError, StoredRules,
};

//! envboard 环境管理器。
//!
//! 分层（依赖单向，由 `envboard-policy-tests` 的 `deps` 门禁强制）：
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
//! 管理器只依赖 [`ProxyEngine`](envboard_core_api::ProxyEngine) 抽象，不认识任何具体引擎实现。

pub mod agent;
pub mod infra;
pub mod logs;
pub mod manager;
pub mod ports;
pub mod state;

pub use manager::{EnvView, Manager, ReconcileReport};
pub use ports::{FilePort, MemoryFiles, MemoryPortProbe, MemoryStateRepo, PortProbe, StateRepo};
pub use state::{
    InstanceLock, JsonFileStateRepo, ManagerConfig, PersistedState, STATE_VERSION, StoredError,
    StoredRules,
};

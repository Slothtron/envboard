//! envboard 引擎库：编排层共享词汇（api）、hosts 规则解析与渲染（rules）、
//! 领域纯逻辑（domain）与进程内代理引擎本体的唯一合并 crate。
//!
//! 契约见包根 `spec/`。依赖纪律：本 crate 不依赖 axum/web 层；外部世界只经
//! 参数注入的路径与标准网络类型（端口语义由 manager 接线时再收口）。

pub mod api;
pub mod auth;
pub mod backend;
pub mod ca;
pub mod capture;
pub mod config;
pub mod der;
pub mod domain;
pub mod engine;
pub mod hosts;
pub mod http;
pub mod logsink;
pub mod request_log;
pub mod rules;
pub mod target;
pub mod tls;
pub mod trajectory;

/// 编排层共享词汇的根再导出：`crate::EngineSpec` 等接缝类型、
/// 错误码、端口（ClockPort/LineWriter/LoggerPort）从这里拿。
pub use api::*;
pub use backend::EngineBackend;
pub use ca::{CaOutcome, SharedCa};
pub use config::{CompiledConfig, EngineConfig};
pub use engine::{EngineInstance, EngineState, EngineStatus};
pub use request_log::LogRecord;
pub use target::{ConnectTarget, TlsPolicy};
pub use tls::{
    NO_SNI_FALLBACK_HOST, SniResolver, mitm_server_config, strict_upstream_client_config,
};

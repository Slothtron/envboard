//! envboard 的纯库引擎。
//!
//! 数据面（CONNECT、HTTP/1.1、鉴权门）、ConnectTarget 与内置 hosts 改写 /
//! 请求日志按 core/spec/capabilities.md 的「引擎能力矩阵」。
//!
//! 依赖纪律：本 crate 是库，不依赖 axum/web 层；外部世界只经参数注入的路径与
//! 标准网络类型（端口语义由 manager 接线时再收口）。

pub mod auth;
pub mod backend;
pub mod ca;
pub mod config;
pub mod der;
pub mod engine;
pub mod hosts;
pub mod http;
pub mod logsink;
pub mod request_log;
pub mod target;
pub mod tls;

pub use backend::EngineBackend;
pub use ca::{CaOutcome, SharedCa};
pub use config::{CompiledConfig, EngineConfig};
pub use engine::{EngineInstance, EngineState, EngineStatus};
pub use target::{ConnectTarget, TlsPolicy};
pub use tls::{
    NO_SNI_FALLBACK_HOST, SniResolver, mitm_server_config, strict_upstream_client_config,
};

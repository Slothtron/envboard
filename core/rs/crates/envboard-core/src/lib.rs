//! envboard v3 的纯库引擎（分阶段落地）。
//!
//! 当前阶段的内容是**共享 CA 与 TLS 接线**：加载/物化 mitmproxy 兼容的 confdir
//! CA、按 SNI 现签叶子证书并缓存、产出 MITM 服务侧与上游严格校验客户侧的 rustls
//! 配置。数据面（CONNECT、HTTP/1.1、鉴权门）、ConnectTarget 与插件管道按
//! core/spec/capabilities.md 的「引擎能力矩阵」。
//!
//! 依赖纪律：本 crate 是库，不依赖 axum/web 层；外部世界只经参数注入的路径与
//! 标准网络类型（端口语义由 manager 接线时再收口）。

pub mod auth;
pub mod backend;
pub mod builtin;
pub mod ca;
pub mod config;
pub mod der;
pub mod engine;
pub mod http;
pub mod logsink;
pub mod plugin;
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

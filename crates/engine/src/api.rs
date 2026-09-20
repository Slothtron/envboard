//! envboard 的 proxy core 抽象与统一错误码（原 core-api crate，并入 engine）。
//!
//! 不依赖 crate 内其它引擎模块（依赖方向由工程门禁强制）。契约见包根 `spec/`。

pub mod engine;
pub mod error;
pub mod ports;
pub mod proxy;
pub mod sha256;
pub mod text;

pub use engine::{
    EngineHandle, EngineReport, EngineSpec, InstanceState, ProxyEngine, UpstreamSpec,
};
// 抓包会话词汇是 UI 边界形状，唯一定义在 envboard-protocol；引擎经这里
// 以原名读写（`crate::CaptureView` 等路径不变）。
pub use envboard_protocol::{CaptureDelta, CaptureView, SessionInfo};
pub use error::{Error, ErrorCode, Retryability};
pub use ports::{
    ClockPort, FixedClock, LineWriter, LogLevel, LoggerPort, ManualClock, NullLineWriter,
    NullLogger,
};
pub use proxy::{CoreCapabilities, CoreInfo, DEFAULT_LISTEN_HOST, Listen};
pub use text::{is_ip_literal, parse_ip_literal, strip_brackets};

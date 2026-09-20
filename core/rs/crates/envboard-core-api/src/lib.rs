//! envboard 的 proxy core 抽象与统一错误码。
//!
//! 这是 core 层的**根 crate**：不依赖任何其它内部 crate（依赖方向由工程门禁强制，
//! 见 `envboard-policy-tests` 的 `deps`）。契约见包根 `core/spec/`。

pub mod engine;
pub mod error;
pub mod ports;
pub mod proxy;
pub mod sha256;
pub mod text;

pub use engine::{
    EngineHandle, EngineReport, EngineSpec, InstanceState, ProxyEngine, UpstreamSpec,
};
pub use error::{Error, ErrorCode, Retryability};
pub use ports::{
    ClockPort, FixedClock, LineWriter, LogLevel, LoggerPort, ManualClock, NullLineWriter,
    NullLogger,
};
pub use proxy::{CoreCapabilities, CoreInfo, DEFAULT_LISTEN_HOST, Listen};
pub use text::{is_ip_literal, parse_ip_literal, strip_brackets};

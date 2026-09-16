//! envboard 的 proxy core 抽象与统一错误码。
//!
//! 这是 core 层的**根 crate**：不依赖任何其它内部 crate（依赖方向由
//! `scripts/rust_dependency_lint.py` 强制）。契约见包根 `core/spec/`。

pub mod error;
pub mod ports;
pub mod proxy;
pub mod sha256;
pub mod text;

pub use error::{Error, ErrorCode, Retryability};
pub use ports::{ClockPort, FixedClock, LogLevel, LoggerPort, ManualClock, NullLogger};
pub use proxy::{
    CONFIG_FILE_NAME, CoreCapabilities, CoreInfo, DEFAULT_LISTEN_HOST, InstanceHandle,
    InstanceHealth, InstanceSpec, Listen, ProcessIdentity, ProxyCore, RULES_LINK_NAME,
    StatusReport, redact_cmdline,
};
pub use text::{is_ip_literal, parse_ip_literal, strip_brackets};

//! core 层需要的通用端口（core 只通过端口访问外界，不碰全局）。
//!
//! 只放**跨 crate 共用**的端口（时钟、日志）。更专门的端口（状态存储、进程表、
//! 端口探活、文件存在性）定义在使用它们的 crate 里（`envboard-manager`），
//! 具体实现放各自的 `infra` 模块 —— 这样 `core` 侧的逻辑永远可以被 Fake 掉。

/// 时间源。core 领域**禁止**直接读系统时间（那样测试就得靠 sleep 猜）。
pub trait ClockPort: Send + Sync {
    /// Unix 秒。
    fn now_unix(&self) -> u64;
}

/// 日志级别 —— 只要够用，不求与任何日志框架对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }
}

/// 日志出口。core 里**禁止**直接 `println!`/`eprintln!`：
/// 那些调用没法在测试里断言，也没法在 TUI/工作台里改写去向。
pub trait LoggerPort: Send + Sync {
    fn log(&self, level: LogLevel, message: &str);
}

/// 什么都不做的日志 —— 测试与"静默模式"用。
#[derive(Debug, Default)]
pub struct NullLogger;

impl LoggerPort for NullLogger {
    fn log(&self, _level: LogLevel, _message: &str) {}
}

/// 固定时钟 —— 测试用（让 `updated_at` 与新鲜度判定完全确定）。
#[derive(Debug, Clone, Copy)]
pub struct FixedClock {
    pub now: u64,
}

impl ClockPort for FixedClock {
    fn now_unix(&self) -> u64 {
        self.now
    }
}

/// 可推进的时钟 —— 需要模拟"状态文件变陈旧"时用。
#[derive(Debug, Default)]
pub struct ManualClock {
    now: std::sync::atomic::AtomicU64,
}

impl ManualClock {
    pub fn new(now: u64) -> Self {
        Self {
            now: std::sync::atomic::AtomicU64::new(now),
        }
    }

    pub fn advance(&self, seconds: u64) {
        self.now
            .fetch_add(seconds, std::sync::atomic::Ordering::SeqCst);
    }
}

impl ClockPort for ManualClock {
    fn now_unix(&self) -> u64 {
        self.now.load(std::sync::atomic::Ordering::SeqCst)
    }
}
/// 行式日志出口（v3 引擎日志通道）。契约：实现不得把背压回传给调用方 ——
/// 引擎侧以有界队列 + 丢弃计数保证数据面永不阻塞在写日志上（对齐 v2
/// 子进程不得阻塞在管道上的教训，实现形态从落盘直写换成内存总线）。
pub trait LineWriter: std::fmt::Debug + Send + Sync {
    fn write_line(&self, line: &str);
}

/// 什么都不做的行出口 —— 测试与无日志形态。
#[derive(Debug, Default)]
pub struct NullLineWriter;

impl LineWriter for NullLineWriter {
    fn write_line(&self, _line: &str) {}
}

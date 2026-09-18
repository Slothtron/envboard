//! envboard 的事件模型。
//!
//! 设计对齐「事件日志即可审计的记录」：一条 append-only 的强类型事件日志
//! 记录控制面动作与数据面请求轨迹，信封与语义在此唯一定义。
//!
//! 硬规则：
//!
//! * 信封 `{ seq, time, type, data }`：`seq` 会话内单调连续，`time` 是 Unix
//!   epoch 毫秒；`ignorable` 默认 false（必需）。
//! * **未知事件 fail-closed**：读者遇到不认识且未标 `ignorable: true` 的
//!   事件必须拒绝，而不是静默跳过 —— 忘标 ignorable 的代价是"过度拒绝"，
//!   不是"静默丢事件"。
//! * 封闭核心枚举 + 一个保留的扩展变体（`Custom { kind, payload }`）：
//!   新增事件类型 = 枚举加变体并登记进 `KNOWN_*_KINDS`；载荷超限截断标
//!   `truncated: true` 由写入方负责。
//! * 存储格式：JSONL，首行 `{"version":1}`；格式版本是单一整数，只有结构
//!   变化才 bump，普通加事件类型不 bump。

mod control;
mod data;
mod envelope;
mod store;

pub use control::{ControlEvent, KNOWN_CONTROL_KINDS};
pub use data::{DataEvent, KNOWN_DATA_KINDS};
pub use envelope::Envelope;
pub use store::{FORMAT_VERSION, ParseError, ParsedEvent, append_line, header_line, parse_log};

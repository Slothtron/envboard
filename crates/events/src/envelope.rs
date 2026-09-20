//! 事件信封：`{ seq, time, type, data, ignorable? }`。
//!
//! `type` / `data` 由事件枚举经 serde 的 `tag = "type", content = "data"`
//! 序列化产生；信封只补 `seq` / `time` / `ignorable` 三个公共字段。
//! 未知类型的 fail-closed 判定在 [`crate::store`]（serde 对未知 tag 本来就
//! 报错，但 ignorable 跳过需要先看信封字段，所以那里手工分两步）。

use serde::{Deserialize, Serialize};

/// 强类型事件信封。`T` 是控制面或数据面的事件枚举。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope<T> {
    /// 会话（一份日志）内单调连续：从 1 起，逐条 +1。
    pub seq: u64,
    /// Unix epoch 毫秒。
    pub time: u64,
    /// 默认 false = 必需。显式标 true 的事件允许被不认识它的读者跳过。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ignorable: bool,
    #[serde(flatten)]
    pub event: T,
}

impl<T> Envelope<T> {
    pub fn new(seq: u64, time: u64, event: T) -> Self {
        Envelope {
            seq,
            time,
            ignorable: false,
            event,
        }
    }

    /// 标记为"可忽略"（新增事件类型时，老读者可以安全跳过它）。
    pub fn ignorable(mut self) -> Self {
        self.ignorable = true;
        self
    }
}

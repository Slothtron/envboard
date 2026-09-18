//! 数据面请求轨迹的记录器。
//!
//! 每环境一个 [`TrajectoryRecorder`]：`seq` 与 `request_id` 都由它分配
//! （实例生命周期内单调连续，热更新不重置 —— 轨迹是同一份日志的延续）。
//! 事件经有界总线落盘到 `trajectories/<env>.jsonl`；写失败丢行不阻断流量。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use envboard_events::{DataEvent, Envelope};

use crate::LineWriter;

pub struct TrajectoryRecorder {
    writer: Arc<dyn LineWriter>,
    next_seq: AtomicU64,
    next_request_id: AtomicU64,
}

impl std::fmt::Debug for TrajectoryRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrajectoryRecorder")
            .field("next_seq", &self.next_seq.load(Ordering::Relaxed))
            .field(
                "next_request_id",
                &self.next_request_id.load(Ordering::Relaxed),
            )
            .finish_non_exhaustive()
    }
}

impl TrajectoryRecorder {
    pub fn start(writer: Arc<dyn LineWriter>) -> Self {
        TrajectoryRecorder {
            writer,
            next_seq: AtomicU64::new(1),
            next_request_id: AtomicU64::new(1),
        }
    }

    pub fn next_request_id(&self) -> u64 {
        self.next_request_id.fetch_add(1, Ordering::AcqRel)
    }

    /// 记录一条事件。永不失败：格式化失败就丢（有界总线的丢弃计数可见）。
    pub fn record(&self, event: DataEvent) {
        let seq = self.next_seq.fetch_add(1, Ordering::AcqRel);
        let envelope = Envelope::new(seq, crate::engine::unix_ms(), event);
        match serde_json::to_string(&envelope) {
            Ok(line) => self.writer.write_line(&line),
            Err(_) => {} // 无损 JSON 在发射点拦截；序列化失败 = 编程错误，不外溢
        }
    }
}

//! 控制面审计事件日志（E-B：`state.json` 仍是权威，事件是可审计的观察）。
//!
//! * `<state_dir>/events.jsonl`：首行 version header，随后每行一个
//!   `Envelope<ControlEvent>`；append-only、单写者（Manager 内部串行）。
//! * **先 state 后事件**：发射点全部在 `Manager::commit` 里，save 成功才 emit。
//! * **事件写失败不阻断控制面**：磁盘满/权限只留 WARN + 计数（丢弃计数进
//!   `/api/status`）—— 事件是观察，不是权威，不能反过来卡住权威的写入。
//! * 体积封顶：超过 `max_event_bytes` 就地轮转（writer 每次追加都是
//!   open-write-close，没有长持 fd，rename 轮转是安全的）。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use envboard_engine::{Error, ErrorCode, LogLevel, LoggerPort};
use envboard_events::{Envelope, ParseError};

use crate::ports::EventStorePort;

/// 默认体积上限（与实例日志同量级）。
pub const DEFAULT_MAX_EVENT_BYTES: usize = 8 * 1024 * 1024;

/// 控制面事件日志。
pub struct ControlEventLog {
    port: Arc<dyn EventStorePort>,
    path: PathBuf,
    max_bytes: usize,
    logger: Arc<dyn LoggerPort>,
    next_seq: AtomicU64,
    dropped: AtomicU64,
}

impl std::fmt::Debug for ControlEventLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlEventLog")
            .field("path", &self.path)
            .field("next_seq", &self.next_seq.load(Ordering::Relaxed))
            .field("dropped", &self.dropped.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl ControlEventLog {
    /// 打开日志：从既有文件恢复 seq 游标（崩溃/重启后 seq 必须继续单调，
    /// 否则历史回读会撞上 seq 断档拒绝）。
    pub fn open(
        port: Arc<dyn EventStorePort>,
        path: PathBuf,
        max_bytes: usize,
        logger: Arc<dyn LoggerPort>,
    ) -> Self {
        // 新文件先落 version header（parse_log 的第一行契约）。
        if port.size(&path).is_none() {
            let _ = port.append(&path, envboard_events::header_line() + "\n");
        }
        let next_seq = Self::recover_seq(&port, &path);
        ControlEventLog {
            port,
            path,
            max_bytes,
            logger,
            next_seq: AtomicU64::new(next_seq),
            dropped: AtomicU64::new(0),
        }
    }

    /// 追加一条事件。永不失败：写不进去就丢 + 计数 + WARN。
    pub fn emit(&self, time: u64, event: envboard_events::ControlEvent) {
        let seq = self.next_seq.fetch_add(1, Ordering::AcqRel);
        let envelope = Envelope::new(seq, time, event);
        let mut line = String::new();
        if let Err(error) = envboard_events::append_line(&mut line, &envelope) {
            self.record_drop(&error.to_string());
            return;
        }
        if self.should_rotate()
            && let Err(error) = self.port.rotate(&self.path)
        {
            // 轮转失败不丢事件：继续往原文件追加（体积上限变为"尽力"）。
            self.logger.log(
                LogLevel::Warn,
                &format!("cannot rotate {}: {error}", self.path.display()),
            );
        }
        if let Err(error) = self.port.append(&self.path, line) {
            self.record_drop(&error.to_string());
        }
    }

    /// 回读历史（拉取式；`name` 非空时只留该环境的事件）。事件尾从文件读，
    /// seq 连续性由 parse_log 判定 —— 断档说明文件被外力截断，如实报错。
    pub fn history(
        &self,
        name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>, Error> {
        const TAIL_BYTES: u64 = 1024 * 1024;
        let text = self.port.read_tail(&self.path, TAIL_BYTES)?;
        let parsed = envboard_events::parse_log(&text).map_err(|error| match error {
            ParseError::SeqGap { .. } => Error::new(
                ErrorCode::InternalError,
                format!("events log has a seq gap (truncated externally?): {error}"),
            ),
            other => Error::new(
                ErrorCode::InternalError,
                format!("events log is unreadable: {other}"),
            ),
        })?;
        let selected: Vec<serde_json::Value> = parsed
            .into_iter()
            .filter(|entry| match (&entry.envelope.event, name) {
                (envboard_events::ControlEvent::Custom { .. }, Some(_)) => false,
                (_, None) => true,
                (event, Some(wanted)) => serde_json::to_value(event)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("data")
                            .and_then(|data| data.get("name"))
                            .and_then(|value| value.as_str())
                            .map(str::to_owned)
                    })
                    .is_some_and(|owner| owner == wanted),
            })
            .map(|entry| serde_json::to_value(&entry.envelope).unwrap_or(serde_json::Value::Null))
            .collect();
        let start = selected.len().saturating_sub(limit);
        Ok(selected[start..].to_vec())
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn recover_seq(port: &Arc<dyn EventStorePort>, path: &Path) -> u64 {
        const TAIL_BYTES: u64 = 1024 * 1024;
        port.read_tail(path, TAIL_BYTES)
            .ok()
            .and_then(|text| {
                envboard_events::parse_log::<envboard_events::ControlEvent>(&text).ok()
            })
            .and_then(|events| events.last().map(|last| last.envelope.seq + 1))
            .unwrap_or(1)
    }

    fn should_rotate(&self) -> bool {
        self.max_bytes > 0
            && self
                .port
                .size(&self.path)
                .is_some_and(|size| size >= self.max_bytes as u64)
    }

    fn record_drop(&self, error: &str) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
        self.logger.log(
            LogLevel::Warn,
            &format!("event dropped (control plane keeps serving): {error}"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::MemoryEventStore;
    use envboard_events::ControlEvent;

    struct FixedClock;
    impl envboard_engine::ClockPort for FixedClock {
        fn now_unix(&self) -> u64 {
            1_700_000_000
        }
    }
    impl envboard_engine::LoggerPort for FixedClock {
        fn log(&self, _level: envboard_engine::LogLevel, _message: &str) {}
    }

    fn log_with(store: &Arc<MemoryEventStore>) -> ControlEventLog {
        ControlEventLog::open(
            store.clone(),
            std::path::PathBuf::from("/state/events.jsonl"),
            DEFAULT_MAX_EVENT_BYTES,
            Arc::new(FixedClock),
        )
    }

    #[test]
    fn seq_continues_across_reopen() {
        let store = Arc::new(MemoryEventStore::default());
        let log = log_with(&store);
        log.emit(1, ControlEvent::EnvironmentDeleted { name: "a".into() });
        log.emit(2, ControlEvent::EnvironmentDeleted { name: "a".into() });

        let reopened = log_with(&store);
        reopened.emit(3, ControlEvent::EnvironmentDeleted { name: "a".into() });
        let history = reopened.history(None, 10).unwrap();
        let seqs: Vec<u64> = history
            .iter()
            .map(|value| {
                value
                    .get("seq")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap()
            })
            .collect();
        assert_eq!(seqs, vec![1, 2, 3]);
    }

    #[test]
    fn history_filters_by_environment_name() {
        let store = Arc::new(MemoryEventStore::default());
        let log = log_with(&store);
        log.emit(
            1,
            ControlEvent::EnvironmentCreated {
                name: "dev".into(),
                listen: "127.0.0.1:9000".into(),
            },
        );
        log.emit(
            2,
            ControlEvent::EnvironmentCreated {
                name: "prod".into(),
                listen: "127.0.0.1:9001".into(),
            },
        );
        log.emit(
            3,
            ControlEvent::RulesDeleted {
                rules_name: "r".into(),
            },
        );
        assert_eq!(log.history(Some("dev"), 10).unwrap().len(), 1);
        assert_eq!(log.history(None, 10).unwrap().len(), 3);
        assert_eq!(log.history(None, 2).unwrap().len(), 2, "limit 取尾部");
    }

    #[test]
    fn write_failure_drops_and_counts_instead_of_failing() {
        struct BrokenStore;
        impl crate::ports::EventStorePort for BrokenStore {
            fn append(&self, _path: &Path, _line: String) -> Result<(), Error> {
                Err(Error::new(ErrorCode::InternalError, "disk full"))
            }
            fn size(&self, _path: &Path) -> Option<u64> {
                None
            }
            fn rotate(&self, _path: &Path) -> Result<(), Error> {
                Ok(())
            }
            fn read_tail(&self, _path: &Path, _max_bytes: u64) -> Result<String, Error> {
                Ok(String::new())
            }
            fn read_from(
                &self,
                _path: &Path,
                _offset: u64,
            ) -> Result<Option<(u64, String)>, Error> {
                Ok(None)
            }
        }
        let log = ControlEventLog::open(
            Arc::new(BrokenStore),
            std::path::PathBuf::from("/state/events.jsonl"),
            DEFAULT_MAX_EVENT_BYTES,
            Arc::new(FixedClock),
        );
        log.emit(1, ControlEvent::EnvironmentDeleted { name: "a".into() });
        assert_eq!(log.dropped(), 1);
    }
}

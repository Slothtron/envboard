//! 域：可观测性 —— 历史、轨迹、日志、维护与只读的装配信息。
//!
//! 收什么：控制面历史（`history`）、数据面轨迹（`trajectory` /
//! `trajectory_since`，SSE 续传游标语义在此）、实例日志尾部与轮转照看
//! （`logs_tail` / `maintain_logs` / `maintain`）、丢弃计数
//! （`events_dropped`）、账本代次（`state_generation`），以及只读的装配
//! 信息（`config` / `core_info` / `capabilities` / `capabilities_v3`）。
//!
//! 本域全部是拉取式只读（`maintain` 例外：它是常驻循环的周期性维护）；
//! 实时流（SSE）在 web crate，消费的就是这里的原语。

use std::sync::atomic::Ordering;

use envboard_engine::domain::Environment;
use envboard_engine::{CoreCapabilities, Error, ErrorCode, LogLevel};
use serde_json::Value;

use crate::manager::Manager;
use crate::ports::EventStorePort;
use crate::state::ManagerConfig;

/// v3 能力表：引擎全部原生能力在线（不再有"某个 core 不支持"的分叉）。
pub fn capabilities_v3() -> CoreCapabilities {
    CoreCapabilities {
        listen: true,
        dynamic_certs: true,
        rewrite_upstream: true,
        per_domain_insecure: true,
        shared_ca: true,
        // 已知限制：数据面 v1 只有 HTTP/1.1（ALPN 只协商 h1）。
        http1_only: true,
    }
}

impl Manager {
    /// 账本代次（每次控制面写入 +1）。
    pub fn state_generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// 轨迹增量读取（SSE 跟随）：从 `offset` 起的完整事件行。
    /// 文件被轮转/截断（offset > size）→ 从头重发基线。
    pub fn trajectory_since(&self, name: &str, offset: u64) -> Result<(u64, Vec<Value>), Error> {
        self.require(name)?;
        let Some(path) = self.config.trajectory_file(name) else {
            return Ok((offset, Vec::new()));
        };
        let store = crate::infra::RealEventStore;
        let Some((file_len, _)) = store.size(&path).map(|size| (size, ())) else {
            return Ok((0, Vec::new()));
        };
        let offset = if offset > file_len { 0 } else { offset };
        let Some((new_offset, text)) = store.read_from(&path, offset)? else {
            return Ok((offset, Vec::new()));
        };
        if text.is_empty() {
            return Ok((new_offset, Vec::new()));
        }
        // 轨迹是窗口化日志（无头、seq 每实例会话重启）：账本式 parse_log 对中段
        // 切片必报 SeqGap，合成头也救不了 —— 用窗口解析。
        let events = envboard_events::parse_window::<envboard_events::DataEvent>(&text).map_err(
            |error| {
                Error::new(
                    ErrorCode::InternalError,
                    format!("trajectory log is unreadable: {error}"),
                )
            },
        )?;
        let values = events
            .into_iter()
            .map(|entry| serde_json::to_value(&entry.envelope).unwrap_or(Value::Null))
            .collect();
        Ok((new_offset, values))
    }

    /// 事件累计丢弃数（`/api/status` 暴露；磁盘故障的可见面）。
    pub fn events_dropped(&self) -> u64 {
        self.events.dropped()
    }

    /// 控制面历史（拉取式只读）。`name` 过滤该环境的事件；`limit` 取尾部。
    pub fn history(&self, name: Option<&str>, limit: usize) -> Result<Vec<Value>, Error> {
        self.events.history(name, limit)
    }

    /// 数据面请求轨迹尾部（拉取式只读；实时流走 SSE）。
    /// 返回 `(cursor, events)`：cursor 是**读之前的文件字节数**（窗口末端偏移），
    /// SSE `baseline` 帧与 REST 响应体用它续传 —— 前端把它原样传回 `?cursor=`。
    pub fn trajectory(&self, name: &str, limit: usize) -> Result<(u64, Vec<Value>), Error> {
        self.require(name)?;
        let Some(path) = self.config.trajectory_file(name) else {
            return Ok((0, Vec::new()));
        };
        let store = crate::infra::RealEventStore;
        let cursor = store.size(&path).unwrap_or(0);
        let text = match store.read_tail(&path, 1024 * 1024) {
            Ok(text) => text,
            Err(_) => return Ok((0, Vec::new())), // 文件不存在 = 还没有轨迹
        };
        // 窗口解析（同上）：read_tail 的起点大概率在一行中间，撕裂首行被逐行跳过，
        // 而不是像账本读者那样整份拒绝。
        let events = envboard_events::parse_window::<envboard_events::DataEvent>(&text).map_err(
            |error| {
                Error::new(
                    ErrorCode::InternalError,
                    format!("trajectory log is unreadable: {error}"),
                )
            },
        )?;
        let selected: Vec<Value> = events
            .into_iter()
            .map(|entry| serde_json::to_value(&entry.envelope).unwrap_or(Value::Null))
            .collect();
        let start = selected.len().saturating_sub(limit);
        Ok((cursor, selected[start..].to_vec()))
    }

    pub fn config(&self) -> &ManagerConfig {
        &self.config
    }

    pub fn core_info(&self) -> envboard_engine::CoreInfo {
        self.engine.describe()
    }

    pub fn capabilities(&self) -> CoreCapabilities {
        capabilities_v3()
    }

    /// 实例日志尾部（文件为唯一落点；崩溃后必须还能读到 —— 崩溃现场正是最需要
    /// 日志的时刻）。有界读并跨轮转拼接。
    pub fn logs_tail(&self, name: &str, lines: usize) -> Result<Vec<String>, Error> {
        self.require(name)?;
        let Some(path) = self.config.log_file(name) else {
            return Ok(Vec::new());
        };
        Ok(crate::logs::tail_with_rotated(&path, lines))
    }

    /// 照看日志体积：超过 max_log_bytes 的环境就地轮转（copytruncate）。幂等且便宜。
    pub fn maintain_logs(&self) -> Vec<String> {
        let cap = self.config.max_log_bytes;
        if cap == 0 || self.config.log_dir.is_none() {
            return Vec::new();
        }
        let mut rotated = Vec::new();
        for raw in &self.state.lock().unwrap().environments {
            let Ok(environment) = Environment::from_json(raw) else {
                continue;
            };
            let name = environment.name().to_string();
            let Some(path) = self.config.log_file(&name) else {
                continue;
            };
            match crate::logs::rotate_if_needed(&path, cap) {
                Ok(true) => {
                    self.logger.log(
                        LogLevel::Info,
                        &format!("{name}: rotated {} past {cap} bytes", path.display()),
                    );
                    rotated.push(name);
                }
                Ok(false) => {}
                Err(error) => self.logger.log(
                    LogLevel::Warn,
                    &format!("{name}: cannot rotate {}: {error}", path.display()),
                ),
            }
        }
        rotated
    }

    /// 常驻循环的周期性维护：日志轮转 + 规则库对账。
    pub fn maintain(&self) -> Vec<String> {
        let mut messages = self.maintain_logs();
        match self.reconcile_rules() {
            Ok(repaired) => messages.extend(repaired),
            Err(error) => self.logger.log(
                LogLevel::Warn,
                &format!("rules reconciliation failed: {error}"),
            ),
        }
        messages
    }
}

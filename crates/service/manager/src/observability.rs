//! 域：可观测性 —— 历史、日志、维护与只读的装配信息。
//!
//! 收什么：控制面历史（`history`）、实例日志尾部与轮转照看
//! （`logs_tail` / `maintain_logs` / `maintain`）、丢弃计数
//! （`events_dropped`）、账本代次（`state_generation`），以及只读的装配
//! 信息（`config` / `core_info` / `capabilities` / `capabilities_v3`）。
//!
//! 本域全部是拉取式只读（`maintain` 例外：它是常驻循环的周期性维护）。
//! 数据面轨迹仍由引擎写 `<state_dir>/trajectories/<env>.jsonl`（见
//! `envboard-events` 的窗口解析），本域不再提供读取入口（ADR-012）。

use std::sync::atomic::Ordering;

use envboard_engine::domain::Environment;
use envboard_engine::{CoreCapabilities, Error, LogLevel};
use serde_json::Value;

use crate::manager::Manager;
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

    /// 事件累计丢弃数（`/api/status` 暴露；磁盘故障的可见面）。
    pub fn events_dropped(&self) -> u64 {
        self.events.dropped()
    }

    /// 控制面历史（拉取式只读）。`name` 过滤该环境的事件；`limit` 取尾部。
    pub fn history(&self, name: Option<&str>, limit: usize) -> Result<Vec<Value>, Error> {
        self.events.history(name, limit)
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

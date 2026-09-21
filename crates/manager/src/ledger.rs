//! 域：账本的私有机制 —— 引擎装配单的拼装、文件写线与端口记账。
//!
//! 收什么：`engine_spec_locked`（从账本拼出引擎 spec）、`log_writer` /
//! `trajectory_writer`（指向环境的 `FileLineWriter`）、`allocate_port`
//! （端口记账：已分配集合 + 随机候选 + 探测挑选）与 `FileLineWriter` 本体。
//!
//! 账本读写本身（`new` 的载入校验、`commit` 的先 save 后 emit）留在
//! manager.rs —— 那是全部写入域共享的唯一发射点，不属于这里。

use std::collections::BTreeSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use envboard_engine::domain::{
    Environment, PortRequest, UpstreamProxy, candidate_ports, seed_from, select_port,
};
use envboard_engine::{EngineSpec, Error, LineWriter, UpstreamSpec};
use serde_json::Value;

use crate::manager::Manager;
use crate::state::PersistedState;

/// 环境日志的文件写线（引擎经有界总线投递到这里；写失败只丢行，不回流）。
/// 放在 manager 侧：日志文件的主人一直是管理器。
pub(crate) struct FileLineWriter {
    path: PathBuf,
}

impl std::fmt::Debug for FileLineWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FileLineWriter({})", self.path.display())
    }
}

impl LineWriter for FileLineWriter {
    fn write_line(&self, line: &str) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(file, "{line}");
        }
    }
}

impl Manager {
    /// 从账本拼出引擎 spec：规则正文只取账本的 rendered、上游代理只取账本的
    /// 定义（账本唯一真相；物化产物不作输入面）。账本条目在载入与保存时都
    /// 校验过，这里解析失败即内部不变量被破坏 —— panic 而不是静默直连。
    pub(crate) fn engine_spec_locked(
        &self,
        state: &PersistedState,
        environment: &Environment,
    ) -> EngineSpec {
        let rules_name = environment.rules();
        let rules_text = rules_name
            .and_then(|name| state.rules_entry(name))
            .map(|entry| {
                if entry.rendered.ends_with('\n') {
                    entry.rendered.clone()
                } else {
                    format!("{}\\n", entry.rendered)
                }
            });
        let upstream = environment.upstream().map(|name| {
            let raw = state
                .proxy_entry(name)
                .unwrap_or_else(|| panic!("upstream proxy {name:?} vanished from the ledger"));
            let proxy = UpstreamProxy::from_json(raw)
                .unwrap_or_else(|error| panic!("ledger entry {name:?} is invalid: {error}"));
            UpstreamSpec {
                host: proxy.host().to_string(),
                port: proxy.port(),
                user: proxy.user().map(str::to_string),
                password: proxy.password().map(str::to_string),
            }
        });
        EngineSpec {
            listen: environment.listen(),
            insecure_hosts: environment.insecure_hosts().to_vec(),
            proxy_user: environment.proxy_user().map(str::to_string),
            proxy_password: environment.proxy_password().map(str::to_string),
            upstream,
            rules_text,
            rules_source: rules_name.map(|name| self.config.rules_path(name)),
            log: self.log_writer(environment.name()),
            trajectory: self.trajectory_writer(environment.name()),
            capture: environment.capture(),
            capture_budget: self.config.capture_budget,
        }
    }

    pub(crate) fn log_writer(&self, name: &str) -> Option<Arc<dyn LineWriter>> {
        self.config
            .log_file(name)
            .map(|path| Arc::new(FileLineWriter { path }) as Arc<dyn LineWriter>)
    }

    fn trajectory_writer(&self, name: &str) -> Option<Arc<dyn LineWriter>> {
        self.config
            .trajectory_file(name)
            .map(|path| Arc::new(FileLineWriter { path }) as Arc<dyn LineWriter>)
    }

    /// 区间内随机挑一个空闲端口。探测只服务于分配；启动以绑定为准。
    pub(crate) fn allocate_port(
        &self,
        host: std::net::IpAddr,
        exclude: &BTreeSet<u16>,
    ) -> Result<u16, Error> {
        let allocated: BTreeSet<u16> = {
            let state = self.state.lock().unwrap();
            state
                .environments
                .iter()
                .filter_map(|raw| {
                    raw.get("listen")
                        .and_then(|listen| listen.get("port"))
                        .and_then(Value::as_u64)
                        .and_then(|port| u16::try_from(port).ok())
                })
                .chain(exclude.iter().copied())
                .collect()
        };

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let seed = seed_from(nanos, std::process::id());
        let candidates = candidate_ports(self.config.port_range, &allocated, seed);

        let probe = &self.probe;
        let mut is_occupied = |port: u16| !probe.is_free(host, port);
        let decision = select_port(
            &PortRequest {
                existing: None,
                requested: None,
                candidates,
                range: self.config.port_range,
                max_attempts: self.config.max_attempts,
            },
            &mut is_occupied,
        )?;
        Ok(decision.port)
    }
}

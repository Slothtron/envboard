//! ProxyEngine 的真实现：按环境持有引擎实例的簿记层。
//!
//! 这里只做三件事：句柄登记（start/stop）、热更转发（apply）、内存报告
//! （report）。没有进程监督、没有状态文件、没有试绑 —— 全部生命周期事实
//! 由 [EngineInstance] 的内存状态回答。日志出口经有界总线（logsink），
//! 丢弃计数随报告上抛。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use envboard_core_api::{
    CoreInfo, EngineHandle, EngineReport, EngineSpec, Error, ErrorCode, InstanceState, LineWriter,
    ProxyEngine,
};

use crate::ca::SharedCa;
use crate::config::EngineConfig;
use crate::engine::{EngineInstance, EngineState};
use crate::logsink::BoundedLinePump;

/// config_hash 的唯一真相是 core-api 的 EngineSpec::hashable_json 摘要；
/// 引擎内部编译哈希只做自身幂等，绝不作为对外回执（禁止第二真相）。
fn receipt_of(spec: &EngineSpec) -> String {
    envboard_core_api::sha256::hex(spec.hashable_json().as_bytes())
}

struct Entry {
    engine: EngineInstance,
    pump: Arc<BoundedLinePump>,
    /// 最近一次成功装配的 spec 回执（start 与 apply 更新）。
    receipt: Mutex<String>,
}

/// v3 引擎后端（进程内、多线程安全：簿记锁只保护 map，不跨 await 持有）。
pub struct EngineBackend {
    ca: Arc<SharedCa>,
    entries: Mutex<BTreeMap<String, Entry>>,
}

impl EngineBackend {
    pub fn new(ca: Arc<SharedCa>) -> Self {
        EngineBackend {
            ca,
            entries: Mutex::new(BTreeMap::new()),
        }
    }
}

fn spec_to_config(spec: &EngineSpec, log: Option<Arc<dyn LineWriter>>) -> EngineConfig {
    EngineConfig {
        listen: spec.listen,
        insecure_hosts: spec.insecure_hosts.clone(),
        proxy_user: spec.proxy_user.clone(),
        proxy_password: spec.proxy_password.clone(),
        rules_text: spec.rules_text.clone(),
        max_buffered_body: None,
        log_writer: log,
        extra_plugins: Vec::new(),
    }
}

fn map_state(state: EngineState) -> InstanceState {
    match state {
        EngineState::Starting => InstanceState::Starting,
        EngineState::Running => InstanceState::Running,
        EngineState::Stopped => InstanceState::Stopped,
        EngineState::PortConflict { port } => InstanceState::PortConflict { port },
        EngineState::Unhealthy { reason } => InstanceState::Unhealthy { reason },
        EngineState::Failed { reason } => InstanceState::Failed { reason },
    }
}

#[async_trait]
impl ProxyEngine for EngineBackend {
    fn describe(&self) -> CoreInfo {
        CoreInfo {
            name: "envboard-core".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    async fn start(&self, env: String, spec: EngineSpec) -> Result<EngineHandle, Error> {
        if self.entries.lock().unwrap().contains_key(&env) {
            return Err(Error::new(
                ErrorCode::Conflict,
                format!("engine for environment {env:?} is already registered; stop it first"),
            ));
        }
        let sink: Arc<dyn LineWriter> = spec
            .log
            .clone()
            .unwrap_or_else(|| Arc::new(envboard_core_api::NullLineWriter));
        let pump = BoundedLinePump::start(sink);
        let engine = EngineInstance::start(
            spec_to_config(&spec, Some(pump.writer())),
            Arc::clone(&self.ca),
        )?;
        let handle = EngineHandle {
            env: env.clone(),
            listen: spec.listen,
        };
        let receipt = receipt_of(&spec);
        self.entries.lock().unwrap().insert(
            env,
            Entry {
                engine,
                pump,
                receipt: Mutex::new(receipt),
            },
        );
        Ok(handle)
    }

    async fn apply(&self, handle: &EngineHandle, spec: EngineSpec) -> Result<String, Error> {
        let guard = self.entries.lock().unwrap();
        let entry = guard.get(&handle.env).ok_or_else(|| {
            Error::new(
                ErrorCode::NotFound,
                format!("no engine registered for environment {:?}", handle.env),
            )
        })?;
        // apply 是同步语义（ArcSwap 换入即生效），锁内直接转发是安全的：
        // EngineInstance::apply_config 不持簿记锁、不做 I/O。
        entry.engine.apply_config(spec_to_config(&spec, None))?;
        let receipt = receipt_of(&spec);
        *entry.receipt.lock().unwrap() = receipt.clone();
        Ok(receipt)
    }

    async fn stop(&self, handle: &EngineHandle) -> Result<(), Error> {
        let entry = self.entries.lock().unwrap().remove(&handle.env);
        match entry {
            Some(entry) => {
                entry.engine.stop();
                Ok(())
            }
            None => Err(Error::new(
                ErrorCode::NotFound,
                format!("no engine registered for environment {:?}", handle.env),
            )),
        }
    }

    fn report(&self, handle: &EngineHandle) -> EngineReport {
        let guard = self.entries.lock().unwrap();
        let Some(entry) = guard.get(&handle.env) else {
            return EngineReport {
                state: InstanceState::Stopped,
                config_hash: String::new(),
                epoch: 0,
                last_error: None,
                bypass_counts: Vec::new(),
                log_drops: 0,
            };
        };
        let status = entry.engine.status();
        EngineReport {
            state: map_state(status.state),
            config_hash: entry.receipt.lock().unwrap().clone(),
            epoch: status.epoch,
            last_error: status.last_error,
            bypass_counts: entry
                .engine
                .bypass_counts()
                .into_iter()
                .map(|(id, count)| (id.to_string(), count))
                .collect(),
            log_drops: entry.pump.dropped(),
        }
    }
}

impl std::fmt::Debug for EngineBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineBackend")
            .field(
                "environments",
                &self.entries.lock().unwrap().keys().collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

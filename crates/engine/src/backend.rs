//! ProxyEngine 的真实现：按环境持有引擎实例的簿记层。
//!
//! 这里只做三件事：句柄登记（start/stop）、热更转发（apply）、内存报告
//! （report）。没有进程监督、没有状态文件、没有试绑 —— 全部生命周期事实
//! 由 [EngineInstance] 的内存状态回答。日志出口经有界总线（logsink），
//! 丢弃计数随报告上抛。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::{
    CoreInfo, EngineHandle, EngineReport, EngineSpec, Error, ErrorCode, InstanceState, LineWriter,
    ProxyEngine,
};

use crate::ca::SharedCa;
use crate::config::EngineConfig;
use crate::engine::{EngineInstance, EngineState};
use crate::logsink::BoundedLinePump;
use crate::trajectory::TrajectoryRecorder;

/// config_hash 的唯一真相是 core-api 的 EngineSpec::hashable_json 摘要；
/// 引擎内部编译哈希只做自身幂等，绝不作为对外回执（禁止第二真相）。
fn receipt_of(spec: &EngineSpec) -> String {
    crate::sha256::hex(spec.hashable_json().as_bytes())
}

struct Entry {
    /// None = 被注入过 failed（引擎实例已停并已释放监听），墓碑留在表里，
    /// 报告如实说 failed —— 与真线程死亡后的可观察形态一致。
    engine: Option<EngineInstance>,
    pump: Arc<BoundedLinePump>,
    trajectory_pump: Option<Arc<BoundedLinePump>>,
    /// 最近一次成功装配的 spec 回执（start 与 apply 更新）。
    receipt: Mutex<String>,
    injected: Option<String>,
}

impl Entry {
    /// 取出活的引擎；被注入过 failed 的实例对此回答"不可操作"。
    fn live_engine(&self) -> Result<&EngineInstance, Error> {
        self.engine.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::Conflict,
                "instance is in failed state; stop and start it again".to_string(),
            )
        })
    }
}

/// v3 引擎后端（进程内、多线程安全：簿记锁只保护 map，不跨 await 持有）。
pub struct EngineBackend {
    ca: Arc<SharedCa>,
    entries: Mutex<BTreeMap<String, Entry>>,
    /// 轨迹记录器与会话 id，**独立于 entries 的生命周期**：stop 移除 Entry 后
    /// 再 start，seq/request_id 仍要续着上一次的走（否则轨迹文件断档）。
    recorders: Mutex<BTreeMap<String, (Arc<TrajectoryRecorder>, u64)>>,
}

impl EngineBackend {
    pub fn new(ca: Arc<SharedCa>) -> Self {
        EngineBackend {
            ca,
            entries: Mutex::new(BTreeMap::new()),
            recorders: Mutex::new(BTreeMap::new()),
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
        trajectory_writer: spec.trajectory.clone(),
        capture: spec.capture,
        capture_budget: spec.capture_budget,
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
            name: "envboard-engine".to_string(),
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
            .unwrap_or_else(|| Arc::new(crate::NullLineWriter));
        let pump = BoundedLinePump::start(sink);
        let trajectory_pump = spec.trajectory.clone().map(BoundedLinePump::start);
        // recorder 与会话 id 跨 stop/start 复用（轨迹 seq 连续性）；抓包会话
        // 则随本次实例全新开始。
        let (recorder, session_id) = {
            let mut recorders = self.recorders.lock().unwrap();
            let slot = recorders.entry(env.clone()).or_insert_with(|| {
                (
                    Arc::new(TrajectoryRecorder::start(
                        spec.trajectory
                            .clone()
                            .unwrap_or_else(|| Arc::new(crate::NullLineWriter)),
                    )),
                    1,
                )
            });
            let session_id = slot.1;
            slot.1 += 1;
            (slot.0.clone(), session_id)
        };
        let engine = EngineInstance::start(
            spec_to_config(&spec, Some(pump.writer())),
            Arc::clone(&self.ca),
            recorder,
            session_id,
        )?;
        let handle = EngineHandle {
            env: env.clone(),
            listen: spec.listen,
        };
        let receipt = receipt_of(&spec);
        self.entries.lock().unwrap().insert(
            env,
            Entry {
                engine: Some(engine),
                pump,
                trajectory_pump,
                receipt: Mutex::new(receipt),
                injected: None,
            },
        );
        Ok(handle)
    }

    fn apply(&self, handle: &EngineHandle, spec: EngineSpec) -> Result<String, Error> {
        let guard = self.entries.lock().unwrap();
        let entry = guard.get(&handle.env).ok_or_else(|| {
            Error::new(
                ErrorCode::NotFound,
                format!("no engine registered for environment {:?}", handle.env),
            )
        })?;
        // apply 是同步语义（ArcSwap 换入即生效），锁内直接转发是安全的：
        // EngineInstance::apply_config 不持簿记锁、不做 I/O。
        let engine = entry.live_engine()?;
        engine.apply_config(spec_to_config(&spec, None))?;
        let receipt = receipt_of(&spec);
        *entry.receipt.lock().unwrap() = receipt.clone();
        Ok(receipt)
    }

    fn inject_failed(&self, env: &str, reason: &str) -> bool {
        let mut guard = self.entries.lock().unwrap();
        let Some(entry) = guard.get_mut(env) else {
            return false;
        };
        if let Some(engine) = entry.engine.take() {
            // stop 内部 join 引擎线程：返回即监听已释放、端口重新可用。
            engine.stop();
        }
        entry.injected = Some(format!("injected failure: {reason}"));
        true
    }

    fn capture_view(&self, env: &str, limit: usize) -> Option<crate::CaptureView> {
        let guard = self.entries.lock().unwrap();
        let engine = guard.get(env)?.engine.as_ref()?;
        let shared = engine.shared_handle();
        Some(crate::CaptureView {
            session: crate::SessionInfo {
                id: shared.capture.session_id(),
                started_at: shared.capture.started_at(),
                generation: shared.capture.generation(),
            },
            captured: shared.capture.captured(),
            dropped: shared.capture.dropped(),
            records: shared.capture.tail(limit),
        })
    }

    fn capture_delta(&self, env: &str, after: u64, limit: usize) -> Option<crate::CaptureDelta> {
        let guard = self.entries.lock().unwrap();
        let engine = guard.get(env)?.engine.as_ref()?;
        let shared = engine.shared_handle();
        Some(crate::CaptureDelta {
            session: crate::SessionInfo {
                id: shared.capture.session_id(),
                started_at: shared.capture.started_at(),
                generation: shared.capture.generation(),
            },
            captured: shared.capture.captured(),
            dropped: shared.capture.dropped(),
            oldest: shared.capture.oldest(),
            records: shared.capture.since(after, limit),
        })
    }

    fn capture_get(&self, env: &str, request_id: u64) -> Option<serde_json::Value> {
        let guard = self.entries.lock().unwrap();
        let engine = guard.get(env)?.engine.as_ref()?;
        engine.shared_handle().capture.get(request_id)
    }

    fn clear_capture(&self, env: &str) -> bool {
        let guard = self.entries.lock().unwrap();
        let Some(engine) = guard.get(env).and_then(|entry| entry.engine.as_ref()) else {
            return false;
        };
        engine.shared_handle().capture.clear();
        true
    }

    async fn stop(&self, handle: &EngineHandle) -> Result<(), Error> {
        let entry = self.entries.lock().unwrap().remove(&handle.env);
        match entry {
            Some(entry) => {
                if let Some(engine) = entry.engine {
                    engine.stop();
                }
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
                log_drops: 0,
                trajectory_drops: 0,
            };
        };
        match &entry.engine {
            Some(engine) => {
                let status = engine.status();
                EngineReport {
                    state: map_state(status.state),
                    config_hash: entry.receipt.lock().unwrap().clone(),
                    epoch: status.epoch,
                    last_error: status.last_error,
                    log_drops: entry.pump.dropped(),
                    trajectory_drops: entry
                        .trajectory_pump
                        .as_ref()
                        .map(|pump| pump.dropped())
                        .unwrap_or(0),
                }
            }
            None => EngineReport {
                state: InstanceState::Failed {
                    reason: entry
                        .injected
                        .clone()
                        .unwrap_or_else(|| "engine is gone".to_string()),
                },
                config_hash: entry.receipt.lock().unwrap().clone(),
                epoch: 0,
                last_error: entry.injected.clone(),
                log_drops: entry.pump.dropped(),
                trajectory_drops: entry
                    .trajectory_pump
                    .as_ref()
                    .map(|pump| pump.dropped())
                    .unwrap_or(0),
            },
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

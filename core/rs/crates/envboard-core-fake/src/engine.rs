//! FakeEngine：v3 ProxyEngine 的测试替身。
//!
//! 与 v2 FakeCore 的分工一致，但形态对齐 v3：**只维护内存里的生命周期事实**。
//! 它真的绑定端口（port_conflict 是实测的，不是编造的），不代理任何流量；
//! 故障注入用旋钮直写报告（live 断言里"实例崩溃"类用例从 SIGKILL 改到这里，
//! 是契约迁移的一部分）。
//!
//! 迁移期与 v2 的 FakeCore 并存；Manager 接线到 ProxyEngine 后 FakeCore 退场。

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;

use envboard_core_api::{
    CoreInfo, EngineHandle, EngineReport, EngineSpec, Error, ErrorCode, InstanceState, ProxyEngine,
    sha256,
};
use tokio::net::TcpListener;

#[derive(Debug)]
struct FakeInstance {
    report: EngineReport,
    /// 持有监听即占住端口（与真引擎"绑定即真相"同构）；绑定失败后为 None。
    _listener: Option<TcpListener>,
}

#[derive(Debug, Default)]
pub struct FakeEngine {
    entries: Mutex<BTreeMap<String, FakeInstance>>,
}

impl FakeEngine {
    pub fn new() -> Self {
        Self::default()
    }

    fn base_report(spec: &EngineSpec) -> EngineReport {
        EngineReport {
            state: InstanceState::Running,
            config_hash: sha256::hex(spec.hashable_json().as_bytes()),
            epoch: 1,
            last_error: None,
            bypass_counts: Vec::new(),
            log_drops: 0,
        }
    }

    /// 故障注入：把某环境的报告改写成任意状态（live 故障路径的替身旋钮）。
    pub fn inject_state(&self, env: &str, state: InstanceState) -> bool {
        let mut guard = self.entries.lock().unwrap();
        match guard.get_mut(env) {
            Some(instance) => {
                let reason = state.reason_text();
                instance.report.state = state;
                instance.report.last_error = reason;
                true
            }
            None => false,
        }
    }

    /// 故障注入：伪造日志丢弃计数。
    pub fn inject_log_drops(&self, env: &str, drops: u64) -> bool {
        let mut guard = self.entries.lock().unwrap();
        match guard.get_mut(env) {
            Some(instance) => {
                instance.report.log_drops = drops;
                true
            }
            None => false,
        }
    }
}

trait ReasonText {
    fn reason_text(&self) -> Option<String>;
}

impl ReasonText for InstanceState {
    fn reason_text(&self) -> Option<String> {
        match self {
            InstanceState::Unhealthy { reason } | InstanceState::Failed { reason } => {
                Some(reason.clone())
            }
            _ => None,
        }
    }
}

#[async_trait]
impl ProxyEngine for FakeEngine {
    fn describe(&self) -> CoreInfo {
        CoreInfo {
            name: "envboard-core-fake".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    async fn start(&self, env: String, spec: EngineSpec) -> Result<EngineHandle, Error> {
        if self.entries.lock().unwrap().contains_key(&env) {
            return Err(Error::new(
                ErrorCode::Conflict,
                format!("fake engine {env:?} already started"),
            ));
        }
        // 簿记锁绝不跨 await（bind 是异步的）；插入前复查同名，竞态也不会双登记。
        let bind = TcpListener::bind((spec.listen.host, spec.listen.port)).await;
        let mut guard = self.entries.lock().unwrap();
        if guard.contains_key(&env) {
            return Err(Error::new(
                ErrorCode::Conflict,
                format!("fake engine {env:?} already started"),
            ));
        }
        let (listener, report) = match bind {
            Ok(listener) => (Some(listener), Self::base_report(&spec)),
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                // 与真引擎同构：start 成功 = 已登记；端口占用是**报告里的状态**，
                // 不是启动错误 —— 管理面的"新建冲突自动重试一次"契约依赖这一点。
                let mut report = Self::base_report(&spec);
                report.state = InstanceState::PortConflict {
                    port: spec.listen.port,
                };
                report.last_error = Some(format!("port occupied: {error}"));
                (None, report)
            }
            Err(error) => {
                return Err(Error::new(
                    ErrorCode::InternalError,
                    format!("fake bind failed: {error}"),
                ));
            }
        };
        guard.insert(
            env.clone(),
            FakeInstance {
                report,
                _listener: listener,
            },
        );
        Ok(EngineHandle {
            env,
            listen: spec.listen,
        })
    }

    async fn apply(&self, handle: &EngineHandle, spec: EngineSpec) -> Result<String, Error> {
        let mut guard = self.entries.lock().unwrap();
        let instance = guard.get_mut(&handle.env).ok_or_else(|| {
            Error::new(
                ErrorCode::NotFound,
                format!("no fake engine for {:?}", handle.env),
            )
        })?;
        let hash = sha256::hex(spec.hashable_json().as_bytes());
        instance.report.config_hash = hash.clone();
        instance.report.epoch += 1;
        Ok(hash)
    }

    async fn stop(&self, handle: &EngineHandle) -> Result<(), Error> {
        self.entries
            .lock()
            .unwrap()
            .remove(&handle.env)
            .map(|_| ())
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::NotFound,
                    format!("no fake engine for {:?}", handle.env),
                )
            })
    }

    fn report(&self, handle: &EngineHandle) -> EngineReport {
        let guard = self.entries.lock().unwrap();
        match guard.get(&handle.env) {
            Some(instance) => instance.report.clone(),
            None => EngineReport {
                state: InstanceState::Stopped,
                config_hash: String::new(),
                epoch: 0,
                last_error: None,
                bypass_counts: Vec::new(),
                log_drops: 0,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::sync::Arc;

    fn spec(port: u16) -> EngineSpec {
        EngineSpec {
            listen: envboard_core_api::Listen::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
            insecure_hosts: Vec::new(),
            proxy_user: None,
            proxy_password: None,
            rules_text: None,
            rules_source: None,
            log: None,
        }
    }

    use std::net::IpAddr;

    #[tokio::test]
    async fn start_apply_report_round_trip() {
        let engine = Arc::new(FakeEngine::new());
        let handle = engine.start("beta".into(), spec(17_401)).await.unwrap();
        let report = engine.report(&handle);
        assert_eq!(report.state, InstanceState::Running);
        assert_eq!(report.epoch, 1);

        let next_hash = engine.apply(&handle, spec(17_401)).await.unwrap();
        let after = engine.report(&handle);
        assert_eq!(after.config_hash, next_hash);
        assert_eq!(after.epoch, 2);

        engine.stop(&handle).await.unwrap();
        assert_eq!(engine.report(&handle).state, InstanceState::Stopped);
    }

    #[tokio::test]
    async fn port_collision_surfaces_as_state_not_startup_error() {
        let engine = FakeEngine::new();
        let first = engine.start("a".into(), spec(17_410)).await.unwrap();
        let second = engine
            .start("b".into(), spec(17_410))
            .await
            .expect("registration succeeds; the conflict is in the report");
        assert_eq!(
            engine.report(&second).state,
            InstanceState::PortConflict { port: 17_410 }
        );
        assert_eq!(engine.report(&first).state, InstanceState::Running);
    }

    #[tokio::test]
    async fn double_start_conflicts() {
        let engine = FakeEngine::new();
        engine.start("beta".into(), spec(17_402)).await.unwrap();
        let error = engine
            .start("beta".into(), spec(17_403))
            .await
            .expect_err("same env twice must conflict");
        assert_eq!(error.code, ErrorCode::Conflict);
    }

    #[tokio::test]
    async fn injected_state_survives_reports() {
        let engine = FakeEngine::new();
        let handle = engine.start("gamma".into(), spec(17_404)).await.unwrap();
        assert!(engine.inject_state(
            "gamma",
            InstanceState::Failed {
                reason: "boom".into()
            }
        ));
        let report = engine.report(&handle);
        assert_eq!(
            report.state,
            InstanceState::Failed {
                reason: "boom".into()
            }
        );
        assert_eq!(report.last_error.as_deref(), Some("boom"));
    }
}

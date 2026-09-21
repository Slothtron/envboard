//! 环境管理器 —— v3 的编排层（引擎是纯库，manager 是它的薄壳）。
//!
//! 职责：环境 CRUD、端口分配与持久化、引擎实例编排、状态持久化、健康判定、
//! reconcile、规则账本。它只认识 ProxyEngine，不认识任何具体实现。
//!
//! 域拆分（`Manager` 是单一类型，各域以 `impl Manager` 分布在平级模块）：
//!
//! * [`lifecycle`] —— 环境 CRUD 与启停（create/start/stop/restart/remove/update）；
//! * [`rules_store`] —— 规则账本：导入、清单、读取、删除与对账；
//! * [`proxies`] —— 上游代理账本；
//! * [`reconcile_health`] —— 期望状态对齐与健康判定；
//! * [`ledger`] —— 账本私有机制：引擎 spec 拼装、文件写线、端口记账；
//! * [`observability`] —— 历史、轨迹、日志、维护与只读装配信息；
//! * [`debug_capture`] —— 抓包会话、调试单例与 HAR 导入；
//! * [`projection`] —— EnvView 投影构造与跨环境对比。
//!
//! 本文件只留：`Manager` 结构体与构造（new / with_event_store）、跨域共享的
//! 私有机制（commit 单一发射点、require/not_found、mark、hot_update_engine、
//! verdict）以及读取入口（list / get）。
//!
//! 与 v2 的整条"进程监督"链路相比，这里的形状变化：
//!
//! * 健康 = 内存报告（ProxyEngine::report）：没有状态文件、没有 TTL、没有
//!   收敛窗口、没有 /proc 身份 —— 引擎与管理器同进程，"生效与否"由
//!   config_hash 回执与 epoch 同步确认；config_mismatch 随热更新同步化而消亡。
//! * 端口 = 绑定即真相：启动流程等待定态（running / port_conflict / failed）；
//!   冲突照旧"新建自动重试一次、已持久化只标记"。探测（PortProbe）只服务于
//!   分配候选挑选，不再是启动前置。
//! * 热更新 = 装配即生效：insecure_hosts / rules 绑定 / 规则内容 / description
//!   全部经 apply 同步换快照；失败的旧快照继续服务并把 invalid_config 标出来
//!   （v2 的 config_error 语义换了载体，纪律不变）。
//! * 崩溃 = 任务级：报告 failed → reconcile 按 desired 自动重启。
//!
//! 契约里"容易实现错"的规则仍然逐条有测试钉住：端口分配矩阵、desired/actual
//! 解耦、先清理后的 reconcile 顺序、账本唯一真相、凭据不回显。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use envboard_engine::domain::{Desired, Environment, UpstreamProxy};
use envboard_engine::{
    ClockPort, EngineHandle, Error, ErrorCode, InstanceState, LogLevel, LoggerPort, ProxyEngine,
};
use envboard_protocol::EnvView;
use serde_json::Value;

use crate::events::ControlEventLog;
use crate::ports::{EventStorePort, FilePort, PortProbe, StateRepo};
use crate::state::{ManagerConfig, PersistedState, StoredError};
use envboard_events::ControlEvent;

pub use crate::observability::capabilities_v3;

/// 管理器。状态只有一个写者：进程内由 Mutex 串行化，进程间由 flock 保证。
pub struct Manager {
    pub(crate) config: ManagerConfig,
    pub(crate) engine: Arc<dyn ProxyEngine>,
    pub(crate) probe: Arc<dyn PortProbe>,
    pub(crate) files: Arc<dyn FilePort>,
    pub(crate) clock: Arc<dyn ClockPort>,
    pub(crate) logger: Arc<dyn LoggerPort>,
    repo: Arc<dyn StateRepo>,
    /// 目录锁。None 表示调用方自己持有（仅供测试与嵌入场景）。
    _lock: Option<crate::state::InstanceLock>,
    pub(crate) state: Mutex<PersistedState>,
    pub(crate) handles: Mutex<BTreeMap<String, EngineHandle>>,
    /// 控制面审计事件日志（E-B：权威仍是 state.json，事件是观察）。
    pub(crate) events: ControlEventLog,
    /// 账本代次：每次 commit +1（SSE 客户端用它跳过未变更的快照轮询）。
    pub(crate) generation: AtomicU64,
    /// 调试会话目标（工作区级单例）：同一时刻至多一个环境在抓包。
    pub(crate) debug_target: Mutex<Option<String>>,
    /// 导入的 HAR 会话库（只读、进程生命周期、多会话并存）。
    pub(crate) har_library: crate::har::HarLibrary,
}

impl Manager {
    #[allow(clippy::too_many_arguments)] // 依赖注入是显式的：端口、时钟、仓储都不藏进构造函数
    pub fn new(
        config: ManagerConfig,
        engine: Arc<dyn ProxyEngine>,
        probe: Arc<dyn PortProbe>,
        files: Arc<dyn FilePort>,
        clock: Arc<dyn ClockPort>,
        logger: Arc<dyn LoggerPort>,
        repo: Arc<dyn StateRepo>,
        acquire_lock: bool,
    ) -> Result<Self, Error> {
        config.validate()?;
        let lock = if acquire_lock {
            Some(crate::state::InstanceLock::acquire(&config.lock_file())?)
        } else {
            None
        };
        let state = repo.load()?;
        // 载入即重新校验：状态文件是外部输入，坏掉要响亮失败。
        for raw in &state.environments {
            Environment::from_json(raw)?;
        }
        for raw in &state.proxies {
            UpstreamProxy::from_json(raw)?;
        }
        // 悬空引用加载即拒（ADR-6）：删除被引用代理已被挡住、绑定已被校验，
        // 稳态下不该出现；出现只能来自手改状态文件 —— 静默降级会谎报链路。
        for raw in &state.environments {
            let environment = Environment::from_json(raw)?;
            if let Some(name) = environment.upstream()
                && state.proxy_entry(name).is_none()
            {
                return Err(Error::invalid_config(
                    "environment.upstream",
                    format!(
                        "environment {:?} references upstream proxy {name:?}, which is not in \
                         the proxy ledger (a dangling reference would silently connect direct)",
                        environment.name()
                    ),
                ));
            }
        }
        let events = ControlEventLog::open(
            Arc::new(crate::infra::RealEventStore),
            config.events_file(),
            crate::events::DEFAULT_MAX_EVENT_BYTES,
            logger.clone(),
        );
        let manager = Self {
            config,
            engine,
            probe,
            files,
            clock,
            logger,
            repo,
            _lock: lock,
            state: Mutex::new(state),
            handles: Mutex::new(BTreeMap::new()),
            events,
            generation: AtomicU64::new(1),
            debug_target: Mutex::new(None),
            har_library: crate::har::HarLibrary::default(),
        };
        for message in manager.reconcile_rules()? {
            manager.logger.log(LogLevel::Info, &message);
        }
        Ok(manager)
    }

    /// 注入事件存储实现（测试用；产品路径是 `infra::RealEventStore`）。
    /// 必须在任何会发事件的操作之前调用。
    pub fn with_event_store(mut self, port: Arc<dyn EventStorePort>) -> Self {
        self.events = ControlEventLog::open(
            port,
            self.config.events_file(),
            crate::events::DEFAULT_MAX_EVENT_BYTES,
            self.logger.clone(),
        );
        self
    }

    /// 控制面事件的单一发射点：**先 save 后 emit**。事件写失败不阻断控制面
    /// （`ControlEventLog::emit` 内部丢弃 + 计数 + WARN）。
    pub(crate) fn commit(&self, state: &PersistedState, event: ControlEvent) -> Result<(), Error> {
        self.repo.save(state)?;
        self.events.emit(self.clock.now_unix_ms(), event);
        self.generation.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    pub(crate) fn require(&self, name: &str) -> Result<Value, Error> {
        self.state
            .lock()
            .unwrap()
            .find(name)
            .cloned()
            .ok_or_else(|| self.not_found(name))
    }

    pub(crate) fn not_found(&self, name: &str) -> Error {
        Error::new(
            ErrorCode::NotFound,
            format!("environment {name:?} does not exist"),
        )
    }

    /// 失败标记的落账点（start / reconcile / hot_update 都会撞到这里）。
    pub(crate) fn mark(&self, name: &str, error: &Error) {
        let mut state = self.state.lock().unwrap();
        state.marks.insert(
            name.into(),
            StoredError {
                code: error.code,
                message: error.message.clone(),
            },
        );
        // 端口冲突与装配失败保留"期望运行"（用户要它跑，只是被挡住了 —— 工作台
        // 显示"受阻"而不是"被放弃"）；其它失败按停止处理（改配置才能再试）。
        let desired = if matches!(
            error.code,
            ErrorCode::PortConflict | ErrorCode::InvalidConfig
        ) {
            Desired::Running
        } else {
            Desired::Stopped
        };
        state.desired.insert(name.into(), desired);
        if let Err(save_error) = self.commit(
            &state,
            ControlEvent::EngineRejected {
                name: name.to_string(),
                reason: error.message.clone(),
            },
        ) {
            self.logger.log(
                LogLevel::Error,
                &format!("cannot persist state: {save_error}"),
            );
        }
    }

    /// 热应用：同步换快照。失败留标记（invalid_config，旧快照继续服务）；
    /// 成功清标记 —— v2 的 config_error 语义换到内存通道，纪律不变。
    pub(crate) fn hot_update_engine(&self, name: &str) {
        let handle = self.handles.lock().unwrap().get(name).cloned();
        let Some(handle) = handle else {
            return;
        };
        let Ok(raw) = self.require(name) else {
            return;
        };
        let Ok(environment) = Environment::from_json(&raw) else {
            return;
        };
        let spec = {
            let state = self.state.lock().unwrap();
            self.engine_spec_locked(&state, &environment)
        };
        match self.engine.apply(&handle, spec) {
            Ok(hash) => {
                let mut state = self.state.lock().unwrap();
                state.marks.remove(name);
                let _ = self.commit(
                    &state,
                    ControlEvent::EngineApplied {
                        name: name.to_string(),
                        config_hash: hash.clone(),
                    },
                );
                self.logger.log(
                    LogLevel::Info,
                    &format!("{name}: hot-applied configuration {hash}"),
                );
            }
            Err(error) => {
                self.logger.log(
                    LogLevel::Warn,
                    &format!(
                        "{name}: apply failed, previous snapshot still serving: {}",
                        error.message
                    ),
                );
                self.mark(name, &error);
            }
        }
    }

    /// 统一健康裁决（视图与权威判定同一实现）。
    pub(crate) fn verdict(
        &self,
        name: &str,
        environment: &Environment,
        state: &PersistedState,
    ) -> InstanceState {
        if let Some(mark) = state.marks.get(name) {
            match mark.code {
                ErrorCode::PortConflict => {
                    return InstanceState::PortConflict {
                        port: environment.listen().port,
                    };
                }
                ErrorCode::InvalidConfig => {
                    return InstanceState::Unhealthy {
                        reason: format!(
                            "configuration rejected, previous snapshot still serving: {}",
                            mark.message
                        ),
                    };
                }
                _ => {}
            }
        }
        if state.desired_of(name) != Desired::Running {
            return InstanceState::Stopped;
        }
        let handle = self.handles.lock().unwrap().get(name).cloned();
        match handle {
            Some(handle) => self.engine.report(&handle).state,
            None => InstanceState::Stopped,
        }
    }

    // ---------------------------------------------------------------- 读取

    pub fn list(&self) -> Result<Vec<EnvView>, Error> {
        let state = self.state.lock().unwrap().clone();
        let mut views = Vec::new();
        for raw in &state.environments {
            views.push(self.view_of(&state, raw)?);
        }
        Ok(views)
    }

    pub fn get(&self, name: &str) -> Result<EnvView, Error> {
        let state = self.state.lock().unwrap().clone();
        let raw = state
            .find(name)
            .cloned()
            .ok_or_else(|| self.not_found(name))?;
        self.view_of(&state, &raw)
    }
}

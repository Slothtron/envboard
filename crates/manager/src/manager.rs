//! 环境管理器 —— v3 的编排层（引擎是纯库，manager 是它的薄壳）。
//!
//! 职责：环境 CRUD、端口分配与持久化、引擎实例编排、状态持久化、健康判定、
//! reconcile、规则账本。它只认识 ProxyEngine，不认识任何具体实现。
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

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use envboard_engine::domain::{
    Action, Desired, Environment, InstanceRecord, PortRequest, ReconcilePlan, candidate_ports,
    plan_reconcile, seed_from, select_port,
};
use envboard_engine::{
    ClockPort, CoreCapabilities, EngineHandle, EngineSpec, Error, ErrorCode, InstanceState,
    LineWriter, Listen, LogLevel, LoggerPort, ProxyEngine,
};
use serde_json::{Map, Value, json};

use crate::events::ControlEventLog;
use crate::ports::{EventStorePort, FilePort, PortProbe, StateRepo};
use crate::state::{ManagerConfig, PersistedState, StoredError, StoredRules};
use envboard_events::ControlEvent;

/// 启动后等待绑定定态的预算（绑定是本地 bind，毫秒级；给足宽容只防极端调度）。
const SETTLE_BUDGET: Duration = Duration::from_secs(5);
const SETTLE_POLL: Duration = Duration::from_millis(10);

/// 一个环境对外的视图（工作台与 CLI 都渲染它）。
#[derive(Debug, Clone)]
pub struct EnvView {
    pub name: String,
    pub listen: Listen,
    pub rules: Option<String>,
    /// 按域名放宽上游校验的完整域名清单（不是凭据，可以回显）。
    pub insecure_hosts: Vec<String>,
    pub description: String,
    pub desired: Desired,
    pub health: InstanceState,
    /// 生效后的规则条数（账本解析得出）。
    pub rules_count: usize,
    /// 绑定了规则名，但账本里没有 —— 该环境当前**不覆盖任何域名**。
    pub rules_missing: bool,
    pub proxy_command: String,
    /// 代理鉴权是否启用。只给布尔，不回显凭据。
    pub proxy_auth_enabled: bool,
}

impl EnvView {
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "listen": {"host": self.listen.host.to_string(), "port": self.listen.port},
            "rules": self.rules,
            "insecure_hosts": self.insecure_hosts,
            "description": self.description,
            "desired": match self.desired { Desired::Running => "running", Desired::Stopped => "stopped" },
            "health": self.health.as_str(),
            "health_reason": self.health.reason(),
            "rules_count": self.rules_count,
            "rules_missing": self.rules_missing,
            "proxy_command": self.proxy_command,
            "proxy_auth_enabled": self.proxy_auth_enabled,
        })
    }
}

/// reconcile 的执行结果。
#[derive(Debug, Clone, Default)]
pub struct ReconcileReport {
    pub actions: Vec<(String, String)>,
    pub warnings: Vec<String>,
}

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

/// 管理器。状态只有一个写者：进程内由 Mutex 串行化，进程间由 flock 保证。
pub struct Manager {
    config: ManagerConfig,
    engine: Arc<dyn ProxyEngine>,
    probe: Arc<dyn PortProbe>,
    files: Arc<dyn FilePort>,
    clock: Arc<dyn ClockPort>,
    logger: Arc<dyn LoggerPort>,
    repo: Arc<dyn StateRepo>,
    /// 目录锁。None 表示调用方自己持有（仅供测试与嵌入场景）。
    _lock: Option<crate::state::InstanceLock>,
    state: Mutex<PersistedState>,
    handles: Mutex<BTreeMap<String, EngineHandle>>,
    /// 控制面审计事件日志（E-B：权威仍是 state.json，事件是观察）。
    events: ControlEventLog,
    /// 账本代次：每次 commit +1（SSE 客户端用它跳过未变更的快照轮询）。
    generation: AtomicU64,
}

/// 环境日志的文件写线（引擎经有界总线投递到这里；写失败只丢行，不回流）。
/// 放在 manager 侧：日志文件的主人一直是管理器。
struct FileLineWriter {
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
    fn commit(&self, state: &PersistedState, event: ControlEvent) -> Result<(), Error> {
        self.repo.save(state)?;
        self.events.emit(self.clock.now_unix_ms(), event);
        self.generation.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

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
        let events = envboard_events::parse_log::<envboard_events::DataEvent>(&format!(
            "{}\n{}",
            envboard_events::header_line(),
            text
        ))
        .map_err(|error| {
            Error::new(
                ErrorCode::InternalError,
                format!("trajectory log is unreadable: {error}"),
            )
        })?;
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
    pub fn trajectory(&self, name: &str, limit: usize) -> Result<Vec<Value>, Error> {
        self.require(name)?;
        let Some(path) = self.config.trajectory_file(name) else {
            return Ok(Vec::new());
        };
        let text = match crate::infra::RealEventStore.read_tail(&path, 1024 * 1024) {
            Ok(text) => text,
            Err(_) => return Ok(Vec::new()), // 文件不存在 = 还没有轨迹
        };
        let events =
            envboard_events::parse_log::<envboard_events::DataEvent>(&text).map_err(|error| {
                Error::new(
                    ErrorCode::InternalError,
                    format!("trajectory log is unreadable: {error}"),
                )
            })?;
        let selected: Vec<Value> = events
            .into_iter()
            .map(|entry| serde_json::to_value(&entry.envelope).unwrap_or(Value::Null))
            .collect();
        let start = selected.len().saturating_sub(limit);
        Ok(selected[start..].to_vec())
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

    // ---------------------------------------------------------------- 写入

    /// 新建环境。listen.port 缺省时自动分配。
    pub fn create(&self, input: &Value) -> Result<EnvView, Error> {
        let object = input.as_object().cloned().ok_or_else(|| {
            Error::invalid_config(
                envboard_engine::domain::PATH,
                "environment must be an object",
            )
        })?;

        let given_name = object
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let name = envboard_engine::domain::normalize_name(given_name);
        envboard_engine::domain::validate_name(&name)?;

        {
            let state = self.state.lock().unwrap();
            if state.find(&name).is_some() {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    format!("environment {name:?} already exists"),
                ));
            }
        }

        let explicit_port = object
            .get("listen")
            .and_then(Value::as_object)
            .and_then(|listen| listen.get("port"))
            .is_some();

        let candidate = if explicit_port {
            Value::Object(object)
        } else {
            let host = object
                .get("listen")
                .and_then(Value::as_object)
                .and_then(|listen| listen.get("host"))
                .and_then(Value::as_str)
                .and_then(envboard_engine::parse_ip_literal)
                .unwrap_or(envboard_engine::DEFAULT_LISTEN_HOST);
            let port = self.allocate_port(host, &BTreeSet::new())?;
            let mut object = object;
            let mut listen = object
                .get("listen")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_else(Map::new);
            listen.insert("port".into(), Value::from(port));
            object.insert("listen".into(), Value::Object(listen));
            Value::Object(object)
        };

        let environment = Environment::from_json(&candidate)?;
        let mut state = self.state.lock().unwrap();
        self.require_known_rules(&state, &environment)?;
        state.environments.push(environment.to_json());
        state.desired.insert(name.clone(), Desired::Stopped);
        state.auto_port.insert(name.clone(), !explicit_port);
        self.commit(
            &state,
            ControlEvent::EnvironmentCreated {
                name: name.clone(),
                listen: environment.listen().to_string(),
            },
        )?;
        self.logger.log(
            LogLevel::Info,
            &format!("created environment {name} on {}", environment.listen()),
        );
        let raw = state.find(&name).cloned().expect("just inserted");
        self.view_of(&state, &raw)
    }

    /// 启动一个环境。失败时留下标记并如实返回错误（不吞）。
    ///
    /// 配置只来自环境上持久化的字段（没有"本次启动临时覆盖"的通道）：
    /// 否则手动启动的实例与账本里的配置会分叉，reconcile 之后又按账本拉回。
    pub async fn start(&self, name: &str) -> Result<EnvView, Error> {
        let raw = self.require(name)?;
        let environment = Environment::from_json(&raw)?;

        // 已有登记先摘干净（上一代 handle 可能停在 failed；重启必须能从零开始）。
        let stale = self.handles.lock().unwrap().remove(name);
        if let Some(stale) = stale {
            let _ = self.engine.stop(&stale).await;
        }

        match self.launch(name, &environment).await {
            Ok(handle) => {
                self.record_started(name, &handle, None);
            }
            Err(error) if error.code == ErrorCode::PortConflict => {
                let auto = {
                    self.state
                        .lock()
                        .unwrap()
                        .auto_port
                        .get(name)
                        .copied()
                        .unwrap_or(false)
                };
                if !auto {
                    // 用户显式指定的端口：禁止静默重分配。
                    self.mark(name, &error);
                    return Err(error);
                }
                self.logger.log(
                    LogLevel::Warn,
                    &format!(
                        "{name}: port {} was taken between allocation and bind; retrying once with a new port",
                        environment.listen().port
                    ),
                );
                let retried = self.reallocate_and_retry(name, &environment).await;
                match retried {
                    Ok(handle) => self.record_started(name, &handle, Some(&error)),
                    Err(second) => {
                        // 重试后仍失败 → 这次是响亮失败。
                        self.mark(name, &second);
                        return Err(second);
                    }
                }
            }
            Err(error) => {
                self.mark(name, &error);
                return Err(error);
            }
        }

        self.get(name)
    }

    pub async fn stop(&self, name: &str) -> Result<EnvView, Error> {
        let handle = self.handles.lock().unwrap().remove(name);
        if let Some(handle) = handle {
            self.engine.stop(&handle).await?;
        }
        let mut state = self.state.lock().unwrap();
        state.desired.insert(name.into(), Desired::Stopped);
        state.marks.remove(name);
        self.commit(
            &state,
            ControlEvent::InstanceStopped {
                name: name.to_string(),
            },
        )?;
        drop(state);
        self.get(name)
    }

    pub async fn restart(&self, name: &str) -> Result<EnvView, Error> {
        self.stop(name).await?;
        self.start(name).await
    }

    pub fn remove(&self, name: &str) -> Result<(), Error> {
        {
            let state = self.state.lock().unwrap();
            let raw = state.find(name).ok_or_else(|| self.not_found(name))?;
            let _ = Environment::from_json(raw)?;
            if state.desired_of(name) == Desired::Running {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    format!("environment {name:?} is desired-running; stop it before removing"),
                ));
            }
        }
        if self.handles.lock().unwrap().contains_key(name) {
            return Err(Error::new(
                ErrorCode::Conflict,
                format!("environment {name:?} still has a live instance; stop it first"),
            ));
        }

        let mut state = self.state.lock().unwrap();
        state
            .environments
            .retain(|raw| raw.get("name").and_then(Value::as_str) != Some(name));
        state.desired.remove(name);
        state.auto_port.remove(name);
        state.marks.remove(name);
        self.commit(
            &state,
            ControlEvent::EnvironmentDeleted {
                name: name.to_string(),
            },
        )?;
        drop(state);

        // v2 遗留的 agent 目录（注入器时代）在这里彻底消失；规则库里的规则不动
        // （它可能还被别的环境绑着，删除规则是 rules.delete 的事）。
        let agent_dir = self.config.env_agent_dir(name);
        if agent_dir.exists() {
            std::fs::remove_dir_all(&agent_dir)?;
        }
        Ok(())
    }
}

impl Manager {
    /// 导入规则：解析 → 规范化渲染 → 记入账本 → 物化落盘（0600）→ 热应用。
    ///
    /// 账本是唯一真相：同名再导入就是覆盖；绑定它且在跑的环境当场换快照
    /// （v2 靠注入器轮询文件达成，v3 是同步 apply —— 这就是"规则内容热生效"）。
    /// 非法内容不让整次导入失败是有意的：一个笔误不该让其余几百条一起报废。
    pub fn import_rules(&self, name: &str, source_text: &str) -> Result<PathBuf, Error> {
        envboard_engine::domain::validate_rules_name(name)?;
        let import = envboard_engine::rules::parse_hosts_text(source_text);
        let rendered = envboard_engine::rules::render_rules(&import.entries, name)?;

        let entry = StoredRules {
            name: name.to_string(),
            source: None,
            rendered: rendered.clone(),
            entries: import.accepted(),
            skipped: import.skipped.len(),
            conflicts: import.conflicts.len(),
            imported_at: self.clock.now_unix(),
        };

        let mut state = self.state.lock().unwrap();
        match state
            .rules
            .iter_mut()
            .find(|existing| existing.name == name)
        {
            Some(existing) => *existing = entry,
            None => state.rules.push(entry),
        }
        state
            .rules
            .sort_by(|left, right| left.name.cmp(&right.name));
        let path = self.config.rules_path(name);
        crate::agent::write_private(&path, rendered.as_bytes())?;
        self.commit(
            &state,
            ControlEvent::RulesImported {
                rules_name: name.to_string(),
                rules_sha256: envboard_engine::sha256::hex(rendered.as_bytes()),
            },
        )?;
        let affected = self.environments_bound_locked(&state, name);
        drop(state);

        self.logger.log(
            LogLevel::Info,
            &format!(
                "imported rules {name}: {} entries, {} skipped, {} conflict(s)",
                import.accepted(),
                import.skipped.len(),
                import.conflicts.len()
            ),
        );
        for env in affected {
            self.hot_update_engine(&env);
        }
        Ok(path)
    }

    /// 规则库清单（从账本读，不依赖物化文件是否存在）。
    pub fn rules_list(&self) -> Result<Vec<String>, Error> {
        let state = self.state.lock().unwrap();
        Ok(state.rules.iter().map(|entry| entry.name.clone()).collect())
    }

    /// 读规则正文：账本里的 rendered 就是权威文本，所以物化文件被删也读得到。
    pub fn rules_read(&self, name: &str) -> Result<String, Error> {
        envboard_engine::domain::validate_rules_name(name)?;
        let state = self.state.lock().unwrap();
        if let Some(entry) = state.rules_entry(name) {
            return Ok(entry.rendered.clone());
        }
        // 账本里没有但文件在（例如有人手工放了文件而启动对账还没跑）：读文件。
        let path = self.config.rules_path(name);
        if self.files.exists(&path) {
            return self.files.read_to_string(&path);
        }
        Err(Error::at(
            ErrorCode::NotFound,
            "rules",
            format!("rules {name:?} is not in the rules library"),
        ))
    }

    /// 删除规则。被任何环境绑定时拒绝 —— 否则那个环境会静默失去覆盖。
    pub fn rules_delete(&self, name: &str) -> Result<(), Error> {
        envboard_engine::domain::validate_rules_name(name)?;
        let mut state = self.state.lock().unwrap();
        for raw in &state.environments {
            let environment = Environment::from_json(raw)?;
            if environment.rules() == Some(name) {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    format!(
                        "rules {name:?} is bound to environment {:?}; unbind it first",
                        environment.name()
                    ),
                ));
            }
        }
        state.rules.retain(|entry| entry.name != name);
        self.commit(
            &state,
            ControlEvent::RulesDeleted {
                rules_name: name.to_string(),
            },
        )?;
        drop(state);

        let path = self.config.rules_path(name);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    /// 规则库对账（幂等）：升级迁移与自愈。软链时代已随注入器退场。
    pub fn reconcile_rules(&self) -> Result<Vec<String>, Error> {
        let mut state = self.state.lock().unwrap();
        let messages = self.reconcile_rules_locked(&mut state)?;
        self.commit(
            &state,
            ControlEvent::Custom {
                kind: "manager/rules-reconciled".to_string(),
                payload: serde_json::json!({ "repaired": messages.len() }),
            },
        )?;
        Ok(messages)
    }

    /// 期望状态与实际状态对齐。顺序固定：先清理、后启动。
    ///
    /// v3 的"实际在跑" = 内存里有 handle 且报告处于 starting/running。端口占用
    /// 检测交给绑定本身：Start 路径会如实收到 port_conflict 并走标记/重试。
    pub async fn reconcile(&self) -> Result<ReconcileReport, Error> {
        for message in self.reconcile_rules()? {
            self.logger.log(LogLevel::Info, &message);
        }

        let state = self.state.lock().unwrap().clone();
        let mut records = Vec::new();
        for raw in &state.environments {
            let environment = Environment::from_json(raw)?;
            let name = environment.name().to_string();
            records.push(InstanceRecord {
                env: name.clone(),
                desired: state.desired_of(environment.name()),
                live: self.is_live(&name),
                listen_port: Some(environment.listen().port),
            });
        }

        // 占用探测**只服务于规划**（MarkConflict 的判据，v2 契约保留）：
        // 启动路径自身不探测 —— 绑定才是真相。没有它，reconcile 会把"端口被占、
        // 期望运行"的自动端口环境直接换端口拉走（静默重分配，契约禁止）。
        let mut occupied = BTreeSet::new();
        for raw in &state.environments {
            let environment = Environment::from_json(raw)?;
            let name = environment.name();
            if state.desired_of(name) != Desired::Running || self.is_live(name) {
                continue;
            }
            let listen = environment.listen();
            if !self.probe.is_free(listen.host, listen.port) {
                occupied.insert(listen.port);
            }
        }
        let plan: ReconcilePlan = plan_reconcile(&records, &occupied);
        let report = ReconcileReport {
            actions: plan
                .actions
                .iter()
                .map(|(env, action)| (env.clone(), action.as_str().into()))
                .collect(),
            warnings: plan.warnings,
        };

        for (env, action) in &plan.actions {
            match action {
                Action::Stop => {
                    self.stop(env).await?;
                }
                Action::Start => {
                    // 启动失败已记在 marks 里；reconcile 不因单个环境失败而中断。
                    if let Err(error) = self.start(env).await {
                        self.logger.log(
                            LogLevel::Warn,
                            &format!("reconcile: cannot start {env}: {error}"),
                        );
                    }
                }
                Action::MarkConflict => {
                    let raw = state
                        .find(env)
                        .cloned()
                        .ok_or_else(|| self.not_found(env))?;
                    let environment = Environment::from_json(&raw)?;
                    self.mark(
                        env,
                        &Error::new(
                            ErrorCode::PortConflict,
                            format!(
                                "port {} is in use by another program; release it or \
                                 re-allocate explicitly (the manager never reassigns silently)",
                                environment.listen().port
                            ),
                        ),
                    );
                }
                Action::Keep => {}
            }
        }

        Ok(report)
    }

    /// 健康判定（v3：内存报告；同步与异步共用同一实现，视图不可能与真相分叉）。
    pub async fn health(&self, name: &str) -> Result<InstanceState, Error> {
        let raw = self.require(name)?;
        let environment = Environment::from_json(&raw)?;
        let state = self.state.lock().unwrap().clone();
        Ok(self.verdict(name, &environment, &state))
    }
}

impl Manager {
    /// 热/停机矩阵（契约的一部分）：
    ///
    /// * 热：description、insecure_hosts、rules 绑定、规则内容 —— 全部经同步
    ///   apply 装配即生效；
    /// * 停机：name、listen、proxy_user、proxy_password —— 划分与 v2 一致：
    ///   换端口/换身份要动监听与客户端配置，换凭据是安全语义的变更，都该是
    ///   一次显式的停机重启。v3 凭据已不经 argv，但字段划分不因实现方便而漂移。
    pub fn update(&self, name: &str, patch: &Value) -> Result<EnvView, Error> {
        let raw = self.require(name)?;
        let environment = Environment::from_json(&raw)?;
        let merged = environment.merged(patch)?;

        let identity_changed =
            merged.name() != environment.name() || merged.listen() != environment.listen();
        let binding_changed = merged.rules() != environment.rules();
        let insecure_changed = merged.insecure_hosts() != environment.insecure_hosts();
        let credentials_changed = merged.proxy_user() != environment.proxy_user()
            || merged.proxy_password() != environment.proxy_password();
        let hot_changed = binding_changed || insecure_changed;
        if (identity_changed || credentials_changed) && self.is_live(name) {
            return Err(Error::new(
                ErrorCode::Conflict,
                format!(
                    "environment {name:?} is running; stop it before changing its name, listen \
                     address, proxy_user or proxy_password. Description, insecure_hosts and the \
                     rules binding are hot and can be changed while running."
                ),
            ));
        }

        // 改了端口/绑定/放行清单/鉴权，旧的失败标记就不再成立：留着它，界面会拿
        // 新端口号去报旧冲突 —— 等于在说谎。
        let mark_is_stale =
            identity_changed || binding_changed || insecure_changed || credentials_changed;

        let mut state = self.state.lock().unwrap();
        if merged.name() != environment.name() && state.find(merged.name()).is_some() {
            return Err(Error::new(
                ErrorCode::Conflict,
                format!("environment {:?} already exists", merged.name()),
            ));
        }
        self.require_known_rules(&state, &merged)?;
        replace_environment_by(&mut state, environment.name(), &merged)?;
        if mark_is_stale {
            state.marks.remove(environment.name());
        }
        if merged.name() != environment.name() {
            let new_name = merged.name().to_string();
            if let Some(value) = state.desired.remove(name) {
                state.desired.insert(new_name.clone(), value);
            }
            if let Some(value) = state.auto_port.remove(name) {
                state.auto_port.insert(new_name.clone(), value);
            }
            if let Some(value) = state.marks.remove(name) {
                state.marks.insert(new_name.clone(), value);
            }
            // 改名只允许停机状态；若有残留 handle（理论不该有），换键带走而不是丢弃。
            if let Some(handle) = self.handles.lock().unwrap().remove(name) {
                let moved = EngineHandle {
                    env: new_name.clone(),
                    ..handle
                };
                self.handles.lock().unwrap().insert(new_name, moved);
            }
            let old_dir = self.config.env_agent_dir(environment.name());
            if old_dir.exists() {
                let _ = std::fs::remove_dir_all(&old_dir);
            }
        }
        let mut changed: Vec<&str> = Vec::new();
        if merged.name() != environment.name() {
            changed.push("name");
        }
        if merged.listen() != environment.listen() {
            changed.push("listen");
        }
        if binding_changed {
            changed.push("rules");
        }
        if insecure_changed {
            changed.push("insecure_hosts");
        }
        if credentials_changed {
            changed.push("proxy_user/proxy_password");
        }
        if merged.description() != environment.description() {
            changed.push("description");
        }
        self.commit(
            &state,
            ControlEvent::EnvironmentUpdated {
                name: environment.name().to_string(),
                fields: changed.iter().map(|field| (*field).to_string()).collect(),
            },
        )?;
        self.logger.log(
            LogLevel::Info,
            &format!("updated {name}: changed {}", changed.join(", ")),
        );
        let raw = state.find(merged.name()).cloned().expect("just replaced");
        drop(state);
        if hot_changed && self.handles.lock().unwrap().contains_key(merged.name()) {
            self.hot_update_engine(merged.name());
        }
        self.view_of(&self.state.lock().unwrap().clone(), &raw)
    }

    /// 显式重分配端口（port_conflict 的解法之一，必须是用户点出来的动作）。
    pub fn reallocate_port(&self, name: &str) -> Result<EnvView, Error> {
        let raw = self.require(name)?;
        let environment = Environment::from_json(&raw)?;
        if self.is_live(name) {
            return Err(Error::new(
                ErrorCode::Conflict,
                format!("environment {name:?} is running; stop it before re-allocating its port"),
            ));
        }
        let mut exclude = BTreeSet::new();
        exclude.insert(environment.listen().port);
        let port = self.allocate_port(environment.listen().host, &exclude)?;

        let mut updated_json = environment.to_json();
        if let Some(listen) = updated_json
            .get_mut("listen")
            .and_then(Value::as_object_mut)
        {
            listen.insert("port".into(), Value::from(port));
        }
        let updated = Environment::from_json(&updated_json)?;

        let mut state = self.state.lock().unwrap();
        replace_environment_by(&mut state, name, &updated)?;
        state.marks.remove(name);
        // 端口换了仍然是"随机分配的"：将来再撞仍可自动换一次。
        state.auto_port.insert(name.into(), true);
        self.commit(
            &state,
            ControlEvent::EnvironmentUpdated {
                name: name.to_string(),
                fields: vec!["listen.port".to_string()],
            },
        )?;
        self.logger.log(
            LogLevel::Info,
            &format!(
                "{name}: port re-allocated to {port} — update your client proxy config accordingly"
            ),
        );
        let raw = state.find(name).cloned().expect("just replaced");
        drop(state);
        self.view_of(&self.state.lock().unwrap().clone(), &raw)
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

    /// 跨环境静态对比：某域名在各环境被覆盖成什么（不发任何请求就能回答）。
    pub fn compare(&self, host: &str) -> Result<Value, Error> {
        let wanted = envboard_engine::rules::normalize_host(host);
        let state = self.state.lock().unwrap().clone();
        let mut rows = Vec::new();
        for raw in &state.environments {
            let environment = Environment::from_json(raw)?;
            let ip = match environment.rules() {
                None => None,
                Some(name) => state.rules_entry(name).and_then(|entry| {
                    envboard_engine::rules::parse_hosts_text(&entry.rendered)
                        .entries
                        .get(&wanted)
                        .cloned()
                }),
            };
            rows.push(json!({
                "env": environment.name(),
                "port": environment.listen().port,
                "rules": environment.rules(),
                "ip": ip,
                "covered": ip.is_some(),
            }));
        }
        Ok(json!({"host": wanted, "environments": rows}))
    }

    /// 实例是否"活着"：有 handle 且报告处于 starting/running。
    pub fn is_live(&self, name: &str) -> bool {
        let handle = self.handles.lock().unwrap().get(name).cloned();
        let Some(handle) = handle else {
            return false;
        };
        matches!(
            self.engine.report(&handle).state,
            InstanceState::Running | InstanceState::Starting
        )
    }
}

impl Manager {
    /// 从账本拼出引擎 spec：规则正文只取账本的 rendered（账本唯一真相；
    /// 物化文件只是随账本维护的产物，不作为输入面）。
    fn engine_spec_locked(&self, state: &PersistedState, environment: &Environment) -> EngineSpec {
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
        EngineSpec {
            listen: environment.listen(),
            insecure_hosts: environment.insecure_hosts().to_vec(),
            proxy_user: environment.proxy_user().map(str::to_string),
            proxy_password: environment.proxy_password().map(str::to_string),
            rules_text,
            rules_source: rules_name.map(|name| self.config.rules_path(name)),
            log: self.log_writer(environment.name()),
            trajectory: self.trajectory_writer(environment.name()),
        }
    }

    fn log_writer(&self, name: &str) -> Option<Arc<dyn LineWriter>> {
        self.config
            .log_file(name)
            .map(|path| Arc::new(FileLineWriter { path }) as Arc<dyn LineWriter>)
    }

    fn trajectory_writer(&self, name: &str) -> Option<Arc<dyn LineWriter>> {
        self.config
            .trajectory_file(name)
            .map(|path| Arc::new(FileLineWriter { path }) as Arc<dyn LineWriter>)
    }

    /// 启动 + 等待绑定定态（绑定即真相，不试绑）。
    async fn launch(&self, name: &str, environment: &Environment) -> Result<EngineHandle, Error> {
        let spec = {
            let state = self.state.lock().unwrap();
            self.engine_spec_locked(&state, environment)
        };
        if let Some(path) = self.config.log_file(name) {
            // 启动前照看一次体积：新的一段日志不该写进"下一行就触发轮转"的文件。
            let _ = crate::logs::rotate_if_needed(&path, self.config.max_log_bytes);
        }
        // 运行标记（契约见 spec/capabilities.md「实例日志」）：每次启动一行、
        // 追加不截断 —— 跨重启的历史靠它分段，日志面板在第一个请求之前也有东西可看。
        if let Some(writer) = self.log_writer(name) {
            writer.write_line(&format!(
                "--- envboard: env={name} listen={} started={} ---",
                environment.listen(),
                self.clock.now_unix()
            ));
        }
        let handle = self.engine.start(name.to_string(), spec).await?;
        let deadline = tokio::time::Instant::now() + SETTLE_BUDGET;
        loop {
            match self.engine.report(&handle).state {
                InstanceState::Starting => {
                    if tokio::time::Instant::now() >= deadline {
                        return Ok(handle);
                    }
                    tokio::time::sleep(SETTLE_POLL).await;
                }
                InstanceState::PortConflict { port } => {
                    let _ = self.engine.stop(&handle).await;
                    return Err(Error::new(
                        ErrorCode::PortConflict,
                        format!("listen port {port} is already in use"),
                    ));
                }
                InstanceState::Failed { reason } => {
                    let _ = self.engine.stop(&handle).await;
                    return Err(Error::new(
                        ErrorCode::InternalError,
                        format!("engine failed at startup: {reason}"),
                    ));
                }
                _ => return Ok(handle),
            }
        }
    }

    fn record_started(&self, name: &str, handle: &EngineHandle, retried: Option<&Error>) {
        let mut state = self.state.lock().unwrap();
        state.desired.insert(name.into(), Desired::Running);
        state.marks.remove(name);
        if let Err(error) = self.commit(
            &state,
            ControlEvent::InstanceStarted {
                name: name.to_string(),
            },
        ) {
            self.logger
                .log(LogLevel::Error, &format!("cannot persist state: {error}"));
        }
        drop(state);
        self.handles
            .lock()
            .unwrap()
            .insert(name.into(), handle.clone());
        if let Some(error) = retried {
            self.logger.log(
                LogLevel::Warn,
                &format!("{name}: started after retrying a port conflict ({}); client config must be updated", error.message),
            );
        }
    }

    fn mark(&self, name: &str, error: &Error) {
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
    fn hot_update_engine(&self, name: &str) {
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
    fn verdict(
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

    /// 故障注入转发（live 断言面）：核心不支持或环境没在跑 → false，不假装。
    pub fn inject_fault(&self, name: &str, reason: &str) -> bool {
        self.engine.inject_failed(name, reason)
    }

    fn require(&self, name: &str) -> Result<Value, Error> {
        self.state
            .lock()
            .unwrap()
            .find(name)
            .cloned()
            .ok_or_else(|| self.not_found(name))
    }

    fn not_found(&self, name: &str) -> Error {
        Error::new(
            ErrorCode::NotFound,
            format!("environment {name:?} does not exist"),
        )
    }

    /// 绑定的规则名必须在规则库里（账本或物化文件）。不放行"绑一个还不存在的
    /// 名字"：那会变成静默失效 —— 用户以为覆盖上了，实际一条都没生效。
    fn require_known_rules(
        &self,
        state: &PersistedState,
        environment: &Environment,
    ) -> Result<(), Error> {
        let Some(name) = environment.rules() else {
            return Ok(());
        };
        let known = state.rules_entry(name).is_some()
            || self.files.exists(&self.config.rules_path(name))
            || self.config.rules_path(name).exists();
        if known {
            return Ok(());
        }
        Err(Error::at(
            ErrorCode::NotFound,
            "environment.rules",
            format!(
                "rules {name:?} is not in the rules library; import it first \\
                 (a binding to a missing name would silently override nothing)"
            ),
        ))
    }

    /// 把账本里的规则物化到 rules_dir/<name>.rules（缺了或内容不一致才写）。
    fn ensure_rules_file(&self, state: &PersistedState, name: &str) -> Result<bool, Error> {
        let Some(entry) = state.rules_entry(name) else {
            return Ok(false);
        };
        let path = self.config.rules_path(name);
        let up_to_date = std::fs::read(&path)
            .map(|existing| existing == entry.rendered.as_bytes())
            .unwrap_or(false);
        if up_to_date {
            return Ok(false);
        }
        crate::agent::write_private(&path, entry.rendered.as_bytes())?;
        Ok(true)
    }

    /// 绑定某条规则的全部环境名（热应用的影响面）。
    fn environments_bound_locked(&self, state: &PersistedState, rules: &str) -> Vec<String> {
        state
            .environments
            .iter()
            .filter_map(|raw| Environment::from_json(raw).ok())
            .filter(|environment| environment.rules() == Some(rules))
            .map(|environment| environment.name().to_string())
            .collect()
    }

    fn reconcile_rules_locked(&self, state: &mut PersistedState) -> Result<Vec<String>, Error> {
        let mut messages = Vec::new();

        // 1) 回填：物化目录里有、账本里没有的规则（升级迁移）。
        if let Ok(entries) = std::fs::read_dir(&self.config.rules_dir) {
            let mut found: Vec<(String, std::path::PathBuf)> = Vec::new();
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "rules")
                    && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
                    && envboard_engine::domain::validate_rules_name(stem).is_ok()
                    && state.rules_entry(stem).is_none()
                {
                    found.push((stem.to_string(), path));
                }
            }
            found.sort();
            for (name, path) in found {
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let import = envboard_engine::rules::parse_hosts_text(&text);
                state.rules.push(StoredRules {
                    name: name.clone(),
                    source: None,
                    rendered: text,
                    entries: import.accepted(),
                    skipped: import.skipped.len(),
                    conflicts: import.conflicts.len(),
                    imported_at: self.clock.now_unix(),
                });
                messages.push(format!(
                    "rules ledger backfilled from {}: {name}",
                    path.display()
                ));
            }
            state
                .rules
                .sort_by(|left, right| left.name.cmp(&right.name));
        }

        // 2) 自愈：账本里有、物化文件缺失或被改坏。
        let names: Vec<String> = state.rules.iter().map(|entry| entry.name.clone()).collect();
        for name in names {
            if self.ensure_rules_file(state, &name)? {
                messages.push(format!("rules {name} was rebuilt from the ledger"));
            }
        }

        Ok(messages)
    }

    fn view_of(&self, state: &PersistedState, raw: &Value) -> Result<EnvView, Error> {
        let environment = Environment::from_json(raw)?;
        let name = environment.name().to_string();
        // 列表不能因为某个环境的规则缺失就整体失败：缺失在视图里明确显示
        // （rules_missing），条数为 0 —— 该字段含义是"实例实际会应用多少条"。
        let rules_missing = environment
            .rules()
            .is_some_and(|bound| state.rules_entry(bound).is_none());
        let rules_count = if rules_missing {
            0
        } else {
            self.expected_rules_count(&environment, state).unwrap_or(0)
        };
        let listen = environment.listen();
        // 视图不回显凭据：启用了鉴权的环境，用户复制后自行补上。
        let proxy_command =
            format!("export https_proxy=http://{listen} http_proxy=http://{listen}");
        Ok(EnvView {
            name: name.clone(),
            listen,
            rules: environment.rules().map(str::to_string),
            insecure_hosts: environment.insecure_hosts().to_vec(),
            description: environment.description().to_string(),
            desired: state.desired_of(&name),
            health: self.verdict(&name, &environment, state),
            rules_count,
            rules_missing,
            proxy_command,
            proxy_auth_enabled: environment.proxy_auth_enabled(),
        })
    }

    /// 期望的规则条数：账本里那条规则被接受多少条（不读物化文件 —— 期望什么
    /// 不该依赖自愈瞬间的文件状态）。
    fn expected_rules_count(
        &self,
        environment: &Environment,
        state: &PersistedState,
    ) -> Result<usize, Error> {
        let Some(name) = environment.rules() else {
            return Ok(0);
        };
        Ok(state
            .rules_entry(name)
            .map(|entry| envboard_engine::rules::parse_hosts_text(&entry.rendered).accepted())
            .unwrap_or(0))
    }

    async fn reallocate_and_retry(
        &self,
        name: &str,
        environment: &Environment,
    ) -> Result<EngineHandle, Error> {
        let mut exclude = BTreeSet::new();
        exclude.insert(environment.listen().port);
        let port = self.allocate_port(environment.listen().host, &exclude)?;

        let mut raw = environment.to_json();
        if let Some(listen) = raw.get_mut("listen").and_then(Value::as_object_mut) {
            listen.insert("port".into(), Value::from(port));
        }
        let updated = Environment::from_json(&raw)?;
        {
            let mut state = self.state.lock().unwrap();
            replace_environment(&mut state, &updated)?;
            self.commit(
                &state,
                ControlEvent::EnvironmentUpdated {
                    name: name.to_string(),
                    fields: vec!["listen.port".to_string()],
                },
            )?;
        }
        self.logger
            .log(LogLevel::Info, &format!("{name}: re-allocated port {port}"));
        self.launch(name, &updated).await
    }

    /// 区间内随机挑一个空闲端口。探测只服务于分配；启动以绑定为准。
    fn allocate_port(&self, host: std::net::IpAddr, exclude: &BTreeSet<u16>) -> Result<u16, Error> {
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

fn replace_environment(state: &mut PersistedState, environment: &Environment) -> Result<(), Error> {
    replace_environment_by(state, environment.name(), environment)
}

/// 按旧名字定位并替换 —— 改名时新名字当然还不在状态里。
fn replace_environment_by(
    state: &mut PersistedState,
    old_name: &str,
    environment: &Environment,
) -> Result<(), Error> {
    for raw in state.environments.iter_mut() {
        if raw.get("name").and_then(Value::as_str) == Some(old_name) {
            *raw = environment.to_json();
            return Ok(());
        }
    }
    Err(Error::new(
        ErrorCode::NotFound,
        format!("environment {old_name:?} does not exist"),
    ))
}

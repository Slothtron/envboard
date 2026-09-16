//! 环境管理器 —— v2 的产品本体。
//!
//! 职责：环境 CRUD、端口分配与持久化、进程编排、状态持久化、健康检查、reconcile。
//! 它只认识 [`ProxyCore`]，不认识 mitmproxy。
//!
//! 这里把契约里几条"容易实现错"的规则集中落地，每条都有测试钉住：
//!
//! * 端口：自动分配 / 显式指定 / 启动失败重试一次 / 已持久化端口**禁止静默重分配**；
//! * 生命周期：期望状态与实际状态解耦，reconcile 顺序固定（先清理、后启动），
//!   上一代实例（没有配置哈希回执）走 `Restart`；
//! * 健康：**状态文件为主、探活为辅**，并且要比对"生效配置回执"（配置哈希 + 规则条数），
//!   两者都受**收敛窗口**保护 —— 刚写完配置的那几秒不该报 `config_mismatch`；
//! * 规则库：账本是唯一真相，物化文件与每环境软链都是**可再生**的产物，缺了就自愈；
//! * 配置下发：`config.json` 是 Rust 与注入器之间唯一的通道，注入器只认自己目录旁边的
//!   固定名，所以"放宽域名清单"与"换规则绑定"都能热生效。

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use envboard_core_api::{
    ClockPort, CoreCapabilities, CoreInfo, Error, ErrorCode, InstanceHandle, InstanceHealth,
    InstanceSpec, Listen, LogLevel, LoggerPort, ProcessIdentity, ProxyCore, RULES_LINK_NAME,
    StatusReport,
};
use envboard_domain::{
    Action, Desired, Environment, InstanceRecord, PortRequest, ReconcilePlan, candidate_ports,
    plan_reconcile, seed_from, select_port,
};
use serde_json::{Map, Value, json};

use crate::agent::{AGENT_CONFIG_VERSION, AgentConfig};
use crate::ports::{FilePort, PortProbe, ProcessTable, StateRepo};
use crate::state::{
    ConfigSeal, InstanceLock, ManagerConfig, PersistedState, StoredError, StoredRules,
};

/// 收敛窗口在轮询间隔之外多给的余量（秒）：注入器可能正好在"刚写完"的那一刻轮询过。
const CONVERGENCE_MARGIN_SECS: u64 = 3;

/// 一个环境对外的视图（工作台与 CLI 都渲染它）。
#[derive(Debug, Clone)]
pub struct EnvView {
    pub name: String,
    pub listen: Listen,
    pub rules: Option<String>,
    /// 按域名放宽上游校验的完整域名清单（**不是凭据，可以回显**）。
    pub insecure_hosts: Vec<String>,
    pub description: String,
    pub desired: Desired,
    pub health: InstanceHealth,
    /// 生效后的规则条数（读物化文件解析得出）。
    pub rules_count: usize,
    /// 绑定了规则名，但链/物化文件不在 —— 该环境当前**不覆盖任何域名**。
    ///
    /// 这是"链接缺失即忽略"这条新语义的可观测面：不报错，但绝不静默。
    pub rules_missing: bool,
    pub proxy_command: String,
    /// 代理鉴权是否启用。只给布尔，**不回显凭据** —— 视图会被列表/详情/SSE 广播，
    /// 凭据只存在状态存储（0600）与实例启动参数里。
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
            // "最后一公里"：用户不需要理解架构，复制粘贴就能用。
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

/// 管理器。**状态只有一个写者**：进程内由 `Mutex` 串行化，进程间由 flock 保证。
pub struct Manager {
    config: ManagerConfig,
    core: Arc<dyn ProxyCore>,
    probe: Arc<dyn PortProbe>,
    processes: Arc<dyn ProcessTable>,
    files: Arc<dyn FilePort>,
    clock: Arc<dyn ClockPort>,
    logger: Arc<dyn LoggerPort>,
    repo: Arc<dyn StateRepo>,
    /// 目录锁。`None` 表示调用方自己持有（仅供测试与嵌入场景）。
    _lock: Option<InstanceLock>,
    state: Mutex<PersistedState>,
    handles: Mutex<BTreeMap<String, InstanceHandle>>,
}

impl Manager {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: ManagerConfig,
        core: Arc<dyn ProxyCore>,
        probe: Arc<dyn PortProbe>,
        processes: Arc<dyn ProcessTable>,
        files: Arc<dyn FilePort>,
        clock: Arc<dyn ClockPort>,
        logger: Arc<dyn LoggerPort>,
        repo: Arc<dyn StateRepo>,
        acquire_lock: bool,
    ) -> Result<Self, Error> {
        config.validate()?;
        let lock = if acquire_lock {
            Some(InstanceLock::acquire(&config.lock_file())?)
        } else {
            None
        };
        let state = repo.load()?;
        // 载入即重新校验：状态文件是外部输入，坏掉要**响亮失败**，
        // 而不是把坏记录带进编排。
        for raw in &state.environments {
            Environment::from_json(raw)?;
        }
        let manager = Self {
            config,
            core,
            probe,
            processes,
            files,
            clock,
            logger,
            repo,
            _lock: lock,
            state: Mutex::new(state),
            handles: Mutex::new(BTreeMap::new()),
        };
        // 启动时对账一次：回填账本（升级迁移）+ 自愈物化文件 + 修链。
        for message in manager.reconcile_rules()? {
            manager.logger.log(LogLevel::Info, &message);
        }
        Ok(manager)
    }

    pub fn config(&self) -> &ManagerConfig {
        &self.config
    }

    pub fn core_info(&self) -> CoreInfo {
        self.core.describe()
    }

    pub fn capabilities(&self) -> CoreCapabilities {
        self.core.capabilities()
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

    /// 新建环境。`listen.port` 缺省时**自动分配**。
    pub fn create(&self, input: &Value) -> Result<EnvView, Error> {
        let object = input.as_object().cloned().ok_or_else(|| {
            Error::invalid_config(envboard_domain::PATH, "environment must be an object")
        })?;

        let given_name = object
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let name = envboard_domain::normalize_name(given_name);
        envboard_domain::validate_name(&name)?;

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
                .and_then(envboard_core_api::parse_ip_literal)
                .unwrap_or(envboard_core_api::DEFAULT_LISTEN_HOST);
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
        // 物化 agent 目录：只有"配置 + 软链"落地了，视图里的 `rules_missing`
        // 才不会在第一次 `start` 之前说谎。
        self.materialize_env(&mut state, &environment)?;
        self.repo.save(&state)?;
        self.logger.log(
            LogLevel::Info,
            &format!("created environment {name} on {}", environment.listen()),
        );
        let raw = state.find(&name).cloned().expect("just inserted");
        self.view_of(&state, &raw)
    }

    /// 启动一个环境。失败时**留下标记**并如实返回错误（不吞）。
    ///
    /// 配置**只来自环境上持久化的字段**（没有"本次启动临时覆盖"的通道）：
    /// 否则手动启动的实例与账本里的配置会分叉，reconcile 之后又按账本把它拉回另一套值。
    pub async fn start(&self, name: &str) -> Result<EnvView, Error> {
        let raw = self.require(name)?;
        let environment = Environment::from_json(&raw)?;
        // 启动前把配置与软链写到位。规则缺失**不再是启动失败**（新语义）：
        // 该环境就不覆盖任何域名，视图会给出提示。
        {
            let mut state = self.state.lock().unwrap();
            self.materialize_env(&mut state, &environment)?;
            self.repo.save(&state)?;
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
                    // 用户显式指定的端口：**禁止静默重分配**。
                    self.mark(name, &error);
                    return Err(error);
                }
                self.logger.log(
                    LogLevel::Warn,
                    &format!(
                        "{name}: port {} was taken between probe and start; retrying once with a new port",
                        environment.listen().port
                    ),
                );
                let retried = self.reallocate_and_retry(name, &environment).await;
                match retried {
                    Ok(handle) => self.record_started(name, &handle, Some(&error)),
                    Err(second) => {
                        // 重试后仍失败 → 这次是**响亮失败**。
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
            self.core.stop(&handle).await?;
        } else if self.capabilities().external_processes {
            // 管理器重启后的孤儿：按**身份**终止（PID + 启动时刻 + cmdline 都匹配才动）。
            let record = { self.state.lock().unwrap().records.get(name).cloned() };
            if let Some(record) = record {
                let processes = Arc::clone(&self.processes);
                tokio::task::spawn_blocking(move || processes.terminate(&record))
                    .await
                    .map_err(|error| Error::internal_error(format!("join error: {error}")))??;
            }
        }

        let mut state = self.state.lock().unwrap();
        state.desired.insert(name.into(), Desired::Stopped);
        state.records.remove(name);
        state.marks.remove(name);
        self.repo.save(&state)?;
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
        state.records.remove(name);
        state.auto_port.remove(name);
        state.marks.remove(name);
        state.config_seals.remove(name);
        self.repo.save(&state)?;
        drop(state);

        // agent 目录（含规则软链）跟着环境一起消失；规则库里的规则**不动**
        // （它可能还被别的环境绑着，删除规则是 rules.delete 的事）。
        let agent_dir = self.config.env_agent_dir(name);
        if agent_dir.exists() {
            std::fs::remove_dir_all(&agent_dir)?;
        }
        Ok(())
    }

    /// 导入规则：解析 → **规范化渲染** → 记入账本 → 物化落盘（0600）。
    ///
    /// 输入由人手写、不保证整洁；输出是确定性的（`core/spec/rules.md` §6）。
    /// 注意"非法内容不让整次导入失败"是**有意的**：一个笔误不该让其余几百条一起报废。
    ///
    /// 账本是唯一真相：同名的再次导入就是**覆盖**（内容热生效 —— 绑定它的实例按
    /// 目标文件的 mtime 重载，不必重启）。
    pub fn import_rules(&self, name: &str, source_text: &str) -> Result<PathBuf, Error> {
        envboard_domain::validate_rules_name(name)?;
        let import = envboard_rules::parse_hosts_text(source_text);
        let rendered = envboard_rules::render_rules(&import.entries, name)?;

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
        self.repo.save(&state)?;
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
        Ok(path)
    }

    /// 规则库清单（**从账本读**，不依赖物化文件是否存在）。
    pub fn rules_list(&self) -> Result<Vec<String>, Error> {
        let state = self.state.lock().unwrap();
        Ok(state.rules.iter().map(|entry| entry.name.clone()).collect())
    }

    /// 读规则正文：账本里的 `rendered` 就是权威文本，所以物化文件被删也读得到。
    pub fn rules_read(&self, name: &str) -> Result<String, Error> {
        envboard_domain::validate_rules_name(name)?;
        let state = self.state.lock().unwrap();
        if let Some(entry) = state.rules_entry(name) {
            return Ok(entry.rendered.clone());
        }
        // 账本里没有但文件在（例如有人手工放了文件而启动对账还没跑）：读文件，别装作没有。
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

    /// 删除规则。**被任何环境绑定时拒绝** —— 否则那个环境会静默失去覆盖。
    ///
    /// 删除 = 账本移除 + 物化文件删除，两件事一起做（账本才是真相）。
    pub fn rules_delete(&self, name: &str) -> Result<(), Error> {
        envboard_domain::validate_rules_name(name)?;
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
        self.repo.save(&state)?;
        drop(state);

        let path = self.config.rules_path(name);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    /// 规则库与软链的对账（幂等）：**升级迁移**与**自愈**都在这一步。
    ///
    /// 1. 物化目录里有、账本里没有的 `*.rules` → 回填账本（读文件重新 parse 得统计）；
    /// 2. 账本里有、物化文件缺失或被改坏 → 按 `rendered` 逐字节重建；
    /// 3. 每个环境的固定名软链与它的绑定对齐（绑定在账本里但物化文件不在 → 不建悬空链）。
    pub fn reconcile_rules(&self) -> Result<Vec<String>, Error> {
        let mut state = self.state.lock().unwrap();
        let messages = self.reconcile_rules_locked(&mut state)?;
        self.repo.save(&state)?;
        Ok(messages)
    }

    /// 期望状态与实际状态对齐。顺序固定：**先清理孤儿，再拉起**。
    pub async fn reconcile(&self) -> Result<ReconcileReport, Error> {
        // 先把规则库/软链对齐，再决定谁要起谁要停。
        for message in self.reconcile_rules()? {
            self.logger.log(LogLevel::Info, &message);
        }

        let state = self.state.lock().unwrap().clone();
        let external = self.capabilities().external_processes;

        let mut records = Vec::new();
        for raw in &state.environments {
            let environment = Environment::from_json(raw)?;
            let name = environment.name().to_string();
            let record = state.records.get(&name).cloned();
            let handle = self.handles.lock().unwrap().get(&name).cloned();
            let live = if let Some(handle) = handle {
                match self.core.probe(&handle).await {
                    InstanceHealth::Running => handle.process.clone(),
                    _ => None,
                }
            } else if external {
                record
                    .as_ref()
                    .and_then(|record| self.processes.identity(record.pid))
            } else {
                // 进程内 core：管理器进程重启后，那些实例确实不存在了。
                None
            };
            records.push(InstanceRecord {
                env: name,
                desired: state.desired_of(environment.name()),
                record,
                live,
                listen_port: Some(environment.listen().port),
                legacy: self.instance_is_legacy(&environment),
            });
        }

        // 谁是"被别的程序占着"：期望 running、但我们没在跑，而端口不通。
        let mut occupied: BTreeSet<u16> = BTreeSet::new();
        for record in &records {
            if record.desired != Desired::Running || record.live.is_some() {
                continue;
            }
            if let Some(port) = record.listen_port {
                let raw = state.find(&record.env).expect("record came from state");
                let environment = Environment::from_json(raw)?;
                if !self.probe.is_free(environment.listen().host, port) {
                    occupied.insert(port);
                }
            }
        }

        let plan: ReconcilePlan = plan_reconcile(&records, &occupied);
        let report = ReconcileReport {
            actions: plan
                .actions
                .iter()
                .map(|(env, action)| (env.clone(), action.as_str().into()))
                .collect(),
            warnings: plan
                .warnings
                .iter()
                .map(|warning| warning.as_str().to_string())
                .collect(),
        };

        for (env, action) in &plan.actions {
            match action {
                Action::Stop => {
                    self.stop(env).await?;
                }
                Action::Start => {
                    // 启动失败已经记在 marks 里；reconcile 不因为单个环境失败而中断。
                    if let Err(error) = self.start(env).await {
                        self.logger.log(
                            LogLevel::Warn,
                            &format!("reconcile: cannot start {env}: {error}"),
                        );
                    }
                }
                Action::Restart => {
                    // 上一代实例：停掉再按当前配置拉起。停失败不阻止后续动作。
                    if let Err(error) = self.stop(env).await {
                        self.logger.log(
                            LogLevel::Warn,
                            &format!(
                                "reconcile: cannot stop the previous-generation {env}: {error}"
                            ),
                        );
                    }
                    if let Err(error) = self.start(env).await {
                        self.logger.log(
                            LogLevel::Warn,
                            &format!("reconcile: cannot restart {env}: {error}"),
                        );
                    }
                }
                Action::MarkConflict => {
                    let raw = state
                        .find(env)
                        .cloned()
                        .ok_or_else(|| self.not_found(env))?;
                    let environment = Environment::from_json(&raw)?;
                    let port = environment.listen().port;
                    self.mark(
                        env,
                        &Error::new(
                            ErrorCode::PortConflict,
                            format!(
                                "port {port} is in use by another program; release it or \
                                 re-allocate explicitly (the manager never reassigns silently)"
                            ),
                        ),
                    );
                }
                Action::Keep => {}
            }
        }

        Ok(report)
    }

    /// 健康判定：**状态文件为主、TCP 探活为辅**。
    ///
    /// 与同步版 [`Self::verdict_blocking`] 的差别只有一处：这里经 `core.probe` 探活，
    /// 因此还能拿到 core 侧的配置回执比对；视图层是同步的、`await` 不了，
    /// 所以它用 `/proc` 身份判活，靠的是同一份状态文件判定（`status_health`）。
    pub async fn health(&self, name: &str) -> Result<InstanceHealth, Error> {
        let raw = self.require(name)?;
        let environment = Environment::from_json(&raw)?;
        let state = self.state.lock().unwrap().clone();

        if let Some(health) = self.stored_mark(name, &environment, &state) {
            return Ok(health);
        }

        if state.desired_of(name) != Desired::Running {
            return Ok(InstanceHealth::Stopped);
        }

        let handle = self.handles.lock().unwrap().get(name).cloned();
        if let Some(handle) = handle {
            let health = self.core.probe(&handle).await;
            if !health.is_running() {
                return Ok(health);
            }
        } else if !self.capabilities().external_processes {
            return Ok(InstanceHealth::Stopped);
        } else if let Some(record) = state.records.get(name) {
            // 进程在不在？身份不匹配也算不在（PID 复用不是我们的实例）。
            if !self.process_is_alive(record) {
                return Ok(InstanceHealth::Stopped);
            }
        } else {
            return Ok(InstanceHealth::Stopped);
        }

        self.status_health(&environment, &state)
    }

    /// PATCH 语义的更新（工作台与 CLI 共用）。
    ///
    /// 热/停机矩阵（契约的一部分，不是实现细节）：
    ///
    /// * **热**：`description`（纯展示）、`insecure_hosts`（注入器重读 `config.json`）、
    ///   `rules` 绑定（原子换链 + 重写 `config.json`）、规则**内容**（目标文件 mtime 变）；
    /// * **停机**：`name`、`listen`、`proxy_user`、`proxy_password` ——
    ///   改名/换端口会让客户端代理配置、状态文件、进程记录同时失效；凭据只在启动时
    ///   经 `--set proxyauth=…` 下发，运行中换它实例根本看不见。于是"配置说换了、实例还在按
    ///   旧的干活"正是那种"看起来成功了但到处都对不上"的故障。
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
        if (identity_changed || credentials_changed) && self.is_live(name) {
            return Err(Error::new(
                ErrorCode::Conflict,
                format!(
                    "environment {name:?} is running; stop it before changing its name, listen \
                     address, proxy_user or proxy_password (the instance reads its listen \
                     address and credentials at launch, so new values cannot take effect). \
                     Description, insecure_hosts and the rules binding are hot and can be \
                     changed while running."
                ),
            ));
        }

        // 改了端口/绑定/放行清单/鉴权，**旧的失败标记就不再成立**：`port_conflict` 记的是
        // "这个端口被占"，`config_mismatch` 记的是"回执与这份配置对不上"（很可能正是某个
        // 选项或凭据造成的）。不清理的话，用户刚把端口换到空位上，界面仍会按旧标记报冲突
        // —— 而且用的是**新**端口号，等于在说谎。
        let mark_is_stale = merged.listen() != environment.listen()
            || binding_changed
            || insecure_changed
            || credentials_changed;

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
            // 配置换了，收敛基准也得跟着换：新的期望哈希一写下去，窗口就重新开始计。
            state.config_seals.remove(environment.name());
        }
        if merged.name() != environment.name() {
            // 改名：期望状态、自动分配标记、异常标记、进程记录与配置基准一起搬过去
            // （五种 map 的 value 类型不同，所以分开搬而不是放进一个循环）
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
            if let Some(value) = state.records.remove(name) {
                state.records.insert(new_name.clone(), value);
            }
            state.config_seals.remove(name);
        }
        // agent 目录跟着名字走：旧目录整份丢掉，新目录在下一次物化时重建
        // （改名只在停机状态被允许，所以没有实例还在用它）。
        if merged.name() != environment.name() {
            let old_dir = self.config.env_agent_dir(environment.name());
            if old_dir.exists() {
                let _ = std::fs::remove_dir_all(&old_dir);
            }
        }
        // 热生效的落点：重写 `config.json` + 对齐软链。运行中的注入器按轮询间隔重读。
        self.materialize_env(&mut state, &merged)?;
        self.repo.save(&state)?;
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
        self.logger.log(
            LogLevel::Info,
            &format!("updated {name}: changed {}", changed.join(", ")),
        );
        let raw = state.find(merged.name()).cloned().expect("just replaced");
        drop(state);
        self.view_of(&self.state.lock().unwrap().clone(), &raw)
    }

    /// 显式重分配端口（`port_conflict` 的解法之一，**必须是用户点出来的动作**）。
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
        state.config_seals.remove(name);
        // 端口换了就不再是"随机分配的"了吗？仍然是 —— 它同样由管理器分配，因此将来
        // 再撞端口仍可自动换一次。
        state.auto_port.insert(name.into(), true);
        self.materialize_env(&mut state, &updated)?;
        self.repo.save(&state)?;
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

    /// 实例日志尾部。
    ///
    /// 两条来源，按"谁是那份日志的主人"分工：内存环形缓冲归 core（只有 `--no-log-file`
    /// 才有），文件归管理器（`<log_dir>/<env>.log`，默认就在这里）。实例崩溃后缓冲/文件
    /// 都必须还能读到 —— 崩溃现场正是最需要日志的时刻。
    pub fn logs_tail(&self, name: &str, lines: usize) -> Result<Vec<String>, Error> {
        self.require(name)?;
        if let Some(handle) = self.handles.lock().unwrap().get(name).cloned() {
            let from_memory = self.core.logs_tail(&handle, lines);
            if !from_memory.is_empty() {
                return Ok(from_memory);
            }
        }
        let Some(path) = self.config.log_file(name) else {
            return Ok(Vec::new());
        };
        // 有界读（只读末尾一个窗口），并且跨轮转往前拼 —— 旧实现是整个文件
        // `read_to_string`，日志一大就会把管理器拖住。
        Ok(crate::logs::tail_with_rotated(&path, lines))
    }

    /// 照看日志体积：超过 `max_log_bytes` 的环境就地轮转（copytruncate）。
    ///
    /// 幂等且便宜（每个环境一次 `stat`），所以由常驻循环按自己的节奏调用即可。
    /// 返回被轮转的环境名，便于测试与日志。
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

    /// 常驻循环的周期性维护：日志轮转 + 规则库/软链对账。
    ///
    /// 幂等且便宜（每项都是几次 `stat`），所以由常驻循环按自己的节奏调用即可。
    /// 返回"这一轮确实动了什么"，便于测试与日志。
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

    /// 跨环境静态对比：某域名在各环境被覆盖成什么。
    ///
    /// 承接 v1 `all_envs` 的用例意图规则是确定性的，所以这个问题
    /// **不需要发任何请求**就能回答，也不会失败。
    pub fn compare(&self, host: &str) -> Result<Value, Error> {
        let wanted = envboard_rules::normalize_host(host);
        let state = self.state.lock().unwrap().clone();
        let mut rows = Vec::new();
        for raw in &state.environments {
            let environment = Environment::from_json(raw)?;
            // 规则正文以账本为准：物化文件被删了也照样答得出来。
            let ip = match environment.rules() {
                None => None,
                Some(name) => state.rules_entry(name).and_then(|entry| {
                    envboard_rules::parse_hosts_text(&entry.rendered)
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

    /// 实例是否"活着"（同步判定：只看内存里的 handle 与外部进程表）。
    fn is_live(&self, name: &str) -> bool {
        if self.handles.lock().unwrap().contains_key(name) {
            return true;
        }
        if !self.capabilities().external_processes {
            return false;
        }
        let state = self.state.lock().unwrap();
        state
            .records
            .get(name)
            .and_then(|record| self.processes.identity(record.pid))
            .is_some_and(|identity| identity.same_process(&state.records[name]))
    }

    // ---------------------------------------------------------------- 内部

    /// 状态文件检查 —— 顺序即优先级（先存在、再新鲜、再比对生效配置）。
    ///
    /// **同步**：它只用同步端口（[`FilePort`] / [`ClockPort`]），所以视图层能直接复用
    /// 同一份判定，不必"列表用简化版、单查用权威版"（那正是"崩掉的实例一直显示 running"
    /// 的来源）。
    fn status_health(
        &self,
        environment: &Environment,
        state: &PersistedState,
    ) -> Result<InstanceHealth, Error> {
        // 注意：调用方可能**正持着状态锁**（`view_of` 的路径），所以这里只用传进来的
        // 快照，绝不自己再 `lock()` —— 那会自锁死。
        let seal = state.config_seals.get(environment.name());
        let path = self.config.status_file(environment.name());
        if !self.files.exists(&path) {
            return Ok(InstanceHealth::Unhealthy {
                reason: format!("status file {} is missing", path.display()),
            });
        }
        let raw = self.files.read_to_string(&path)?;
        let status = match StatusReport::from_json_slice(raw.as_bytes()) {
            Ok(status) => status,
            Err(error) => {
                return Ok(InstanceHealth::Unhealthy {
                    reason: format!("status file is unreadable: {}", error.message),
                });
            }
        };

        let now = self.clock.now_unix();
        if now.saturating_sub(status.updated_at) > self.config.status_ttl_secs {
            return Ok(InstanceHealth::Unhealthy {
                reason: format!(
                    "status file is stale: updated_at={} now={now} ttl={}s",
                    status.updated_at, self.config.status_ttl_secs
                ),
            });
        }
        if let Some(last_error) = &status.last_error {
            return Ok(InstanceHealth::Unhealthy {
                reason: last_error.clone(),
            });
        }
        // 热重载失败：实例**保留旧的快照**继续代理（不中断流量），但这份配置没生效 ——
        // 必须报出来，否则用户会以为改完了。
        if let Some(config_error) = &status.config_error {
            return Ok(InstanceHealth::Unhealthy {
                reason: format!("the instance kept its previous configuration: {config_error}"),
            });
        }

        // 收敛窗口：刚写完配置的那几秒里，实例还没轮到轮询，回执必然还是旧的。
        let window = self.config.reload_interval_secs + CONVERGENCE_MARGIN_SECS;
        let within_window = seal.is_some_and(|seal| now.saturating_sub(seal.written_at) <= window);

        // 1) 配置指纹：这是"账本里那份配置到底有没有生效"的权威判据。
        if let Some(seal) = seal
            && status.config_hash != seal.hash
            && !within_window
        {
            return Ok(InstanceHealth::ConfigMismatch {
                reason: format!(
                    "the instance reports config_hash={} but the manager's configuration hashes \
                     to {} (written {}s ago, convergence window is {window}s)",
                    short_hash(&status.config_hash),
                    short_hash(&seal.hash),
                    now.saturating_sub(seal.written_at)
                ),
            });
        }

        // 2) 规则条数：补充断言（沿用旧行为），同样受收敛窗口保护。
        //
        // 链接不在效（缺失/悬空/指向别的名字）时**跳过**这条比对：那种情况已经由
        // `rules_missing` 明确报出来了，再叠一个 `config_mismatch` 只会让用户看到两个
        // 症状、一个原因。
        let capabilities = self.capabilities();
        if capabilities.reports_rules_count && self.bound_rules_in_effect(environment) {
            let expected = self.expected_rules_count(environment, state)?;
            if status.rules_count != expected && !within_window {
                return Ok(InstanceHealth::ConfigMismatch {
                    reason: format!(
                        "instance reports rules_count={} but the effective rules file has \
                         {expected} entries (a host may have silently ignored a --set option)",
                        status.rules_count
                    ),
                });
            }
        }
        Ok(InstanceHealth::Running)
    }

    /// 跨重启保留的标记优先于推导出的状态（两个判定路径共用）。
    fn stored_mark(
        &self,
        name: &str,
        environment: &Environment,
        state: &PersistedState,
    ) -> Option<InstanceHealth> {
        let mark = state.marks.get(name)?;
        match mark.code {
            ErrorCode::PortConflict => Some(InstanceHealth::PortConflict {
                port: environment.listen().port,
            }),
            ErrorCode::ConfigMismatch => Some(InstanceHealth::ConfigMismatch {
                reason: mark.message.clone(),
            }),
            _ => None,
        }
    }

    /// **同步**的完整判定：标记 → 期望状态 → 进程存活 → 状态文件。
    ///
    /// 视图层（列表/详情）必须给出与权威判定一致的结论，否则会出现"崩掉的实例一直被
    /// 报成 running"（实测：`kill -9` 掉 mitmproxy 后 60 秒仍报 running）。所以这里
    /// 做真检查，而不是"handle 表里有这个 key 就算在跑"。
    ///
    /// 与异步 [`Self::health`] 的唯一差别：进程死亡原因来自 [`ProxyCore::last_exit`]
    /// 而不是 `probe`（后者顺带做了配置回执比对，见 `status_health`）。
    fn verdict_blocking(
        &self,
        name: &str,
        environment: &Environment,
        state: &PersistedState,
    ) -> Result<InstanceHealth, Error> {
        if let Some(health) = self.stored_mark(name, environment, state) {
            return Ok(health);
        }
        if state.desired_of(name) != Desired::Running {
            return Ok(InstanceHealth::Stopped);
        }

        let handle = self.handles.lock().unwrap().get(name).cloned();
        let had_handle = handle.is_some();
        let alive = if let Some(handle) = handle.as_ref() {
            if !self.capabilities().external_processes {
                // 进程内的 core：实例活在管理器进程里，"在表里"就等于"活着"。
                true
            } else {
                handle
                    .process
                    .as_ref()
                    .is_some_and(|expected| self.process_is_alive(expected))
            }
        } else if !self.capabilities().external_processes {
            return Ok(InstanceHealth::Stopped);
        } else if let Some(record) = state.records.get(name) {
            self.process_is_alive(record)
        } else {
            return Ok(InstanceHealth::Stopped);
        };

        if !alive {
            // 进程没了：说清是"它自己死的"（带上退出原因）还是"就是不在了"。
            // core 只按 env@port 记账，所以没有 handle 时用状态记录里的身份造一个。
            let from_record;
            let probe_handle = match handle.as_ref() {
                Some(handle) => Some(handle),
                None => match state.records.get(name) {
                    Some(record) => {
                        from_record = InstanceHandle {
                            env: name.to_string(),
                            listen: environment.listen(),
                            process: Some(record.clone()),
                        };
                        Some(&from_record)
                    }
                    None => None,
                },
            };
            let reported = probe_handle.and_then(|handle| self.core.last_exit(handle));
            return Ok(match reported {
                Some(reason) => InstanceHealth::Failed { reason },
                // 我们手里有它的 handle（= 它由本进程拉起、也没人叫它停），进程却没了 ——
                // 那是它自己死的。回收任务写下"遗言"之前也要这么报，否则工作台会出现
                // 一瞬间的 `stopped`，把"崩溃"说成"我停的"。
                None if had_handle => InstanceHealth::Failed {
                    reason: "the proxy core process is gone (nobody asked it to stop)".to_string(),
                },
                None => InstanceHealth::Stopped,
            });
        }

        self.status_health(environment, state)
    }

    /// 进程是否还活着（身份三级匹配；僵尸**不算**活着，见 `infra::ProcfsProcessTable`）。
    fn process_is_alive(&self, expected: &ProcessIdentity) -> bool {
        self.processes
            .identity(expected.pid)
            .is_some_and(|identity| identity.same_process(expected))
    }

    /// 在跑的实例是不是上一代二进制拉起来的（状态文件里没有配置哈希回执）。
    fn instance_is_legacy(&self, environment: &Environment) -> bool {
        let path = self.config.status_file(environment.name());
        let Ok(raw) = self.files.read_to_string(&path) else {
            return false;
        };
        StatusReport::from_json_slice(raw.as_bytes()).is_ok_and(|status| status.is_legacy())
    }

    async fn launch(&self, name: &str, environment: &Environment) -> Result<InstanceHandle, Error> {
        let spec = self.instance_spec(name, environment);
        std::fs::create_dir_all(&self.config.runtime_dir)?;
        std::fs::create_dir_all(&self.config.confdir)?;
        // agent 目录与配置在 `start` 里已经写好了；这里只兜底建目录。
        std::fs::create_dir_all(&spec.agent_dir)?;
        // 启动前照看一次体积：新的一段日志不该写进一个"下一行就触发轮转"的文件。
        if let Some(path) = self.config.log_file(name) {
            let _ = crate::logs::rotate_if_needed(&path, self.config.max_log_bytes);
        }
        self.core.start(spec).await
    }

    fn instance_spec(&self, name: &str, environment: &Environment) -> InstanceSpec {
        InstanceSpec {
            env: name.to_string(),
            listen: environment.listen(),
            agent_dir: self.config.env_agent_dir(name),
            status_file: self.config.status_file(name),
            shared_state_dir: self.config.confdir.clone(),
            runtime_dir: self.config.runtime_dir.clone(),
            log_dir: self.config.log_dir.clone(),
            insecure_hosts: environment.insecure_hosts().to_vec(),
            proxy_user: environment.proxy_user().map(str::to_string),
            proxy_password: environment.proxy_password().map(str::to_string),
        }
    }

    async fn reallocate_and_retry(
        &self,
        name: &str,
        environment: &Environment,
    ) -> Result<InstanceHandle, Error> {
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
            state.config_seals.remove(name);
            self.materialize_env(&mut state, &updated)?;
            self.repo.save(&state)?;
        }
        self.logger
            .log(LogLevel::Info, &format!("{name}: re-allocated port {port}"));
        self.launch(name, &updated).await
    }

    fn record_started(&self, name: &str, handle: &InstanceHandle, retried: Option<&Error>) {
        let mut state = self.state.lock().unwrap();
        state.desired.insert(name.into(), Desired::Running);
        state.marks.remove(name);
        match &handle.process {
            Some(identity) => {
                state.records.insert(name.into(), identity.clone());
            }
            None => {
                state.records.remove(name);
            }
        }
        if let Err(error) = self.repo.save(&state) {
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
        // 端口冲突保留"期望运行"：用户要它跑，只是被占着挡住了 —— 工作台该显示
        // "受阻"而不是"被放弃"。其它失败按停止处理（改配置才能再试）。
        let desired = if error.code == ErrorCode::PortConflict {
            Desired::Running
        } else {
            Desired::Stopped
        };
        state.desired.insert(name.into(), desired);
        state.records.remove(name);
        if let Err(save_error) = self.repo.save(&state) {
            self.logger.log(
                LogLevel::Error,
                &format!("cannot persist state: {save_error}"),
            );
        }
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

    /// 绑定的规则名必须在规则库里。
    ///
    /// 为什么不放行"绑一个还不存在的名字"：在新语义下（链接缺失 = 不覆盖）那会变成一次
    /// **静默失效** —— 用户以为覆盖上了，实际一条都没生效。宁可在写入时就拒绝。
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
                "rules {name:?} is not in the rules library; import it first \
                 (a binding to a missing name would silently override nothing)"
            ),
        ))
    }

    /// 把账本里的规则物化到 `<rules_dir>/<name>.rules`（缺了或内容不一致才写）。
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

    /// 物化一个环境的 agent 目录：规则文件 + 固定名软链 + `config.json`（含收敛基准）。
    fn materialize_env(
        &self,
        state: &mut PersistedState,
        environment: &Environment,
    ) -> Result<String, Error> {
        let name = environment.name();
        if let Some(rules) = environment.rules() {
            self.ensure_rules_file(state, rules)?;
        }
        let agent_dir = self.config.env_agent_dir(name);
        crate::agent::sync_rules_link(&agent_dir, environment.rules(), &self.config.rules_dir)?;

        let spec_like = self.instance_spec(name, environment);
        let config = AgentConfig {
            version: AGENT_CONFIG_VERSION,
            env: name.to_string(),
            status_file: self.config.status_file(name).display().to_string(),
            rules: environment.rules().map(str::to_string),
            insecure_hosts: environment.insecure_hosts().to_vec(),
            launch_expected: spec_like.launch_expected(),
            reload_interval_secs: self.config.reload_interval_secs,
            annotate: self.config.annotate,
        };
        let (hash, _wrote) = crate::agent::write_config(&agent_dir, &config)?;
        // 收敛基准**只在期望内容变化时**更新：每轮都刷新的话，一个永远读不进新配置的
        // 实例会被无限期当成"收敛中"，`config_mismatch` 就再也报不出来了。
        let unchanged = state
            .config_seals
            .get(name)
            .is_some_and(|seal| seal.hash == hash);
        if !unchanged {
            state.config_seals.insert(
                name.to_string(),
                ConfigSeal {
                    hash: hash.clone(),
                    written_at: self.clock.now_unix(),
                },
            );
        }
        Ok(hash)
    }

    /// 规则库对账（调用方持锁）。
    fn reconcile_rules_locked(&self, state: &mut PersistedState) -> Result<Vec<String>, Error> {
        let mut messages = Vec::new();

        // 1) 回填：物化目录里有、账本里没有的规则（升级迁移）。
        if let Ok(entries) = std::fs::read_dir(&self.config.rules_dir) {
            let mut found: Vec<(String, std::path::PathBuf)> = Vec::new();
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "rules")
                    && let Some(name) = path.file_stem().and_then(|stem| stem.to_str())
                    && envboard_domain::validate_rules_name(name).is_ok()
                    && state.rules_entry(name).is_none()
                {
                    found.push((name.to_string(), path));
                }
            }
            found.sort();
            for (name, path) in found {
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let import = envboard_rules::parse_hosts_text(&text);
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

        // 3) 每个环境的固定名软链与它的绑定对齐。
        for raw in &state.environments {
            let environment = Environment::from_json(raw)?;
            let agent_dir = self.config.env_agent_dir(environment.name());
            if crate::agent::sync_rules_link(
                &agent_dir,
                environment.rules(),
                &self.config.rules_dir,
            )? {
                messages.push(format!(
                    "{}: rules link aligned with the binding",
                    environment.name()
                ));
            }
        }

        Ok(messages)
    }

    fn view_of(&self, state: &PersistedState, raw: &Value) -> Result<EnvView, Error> {
        let environment = Environment::from_json(raw)?;
        let name = environment.name().to_string();
        // 列表/详情**不能**因为某个环境的规则文件缺失就整体失败：那是"一个环境坏了"
        // 而不是"状态坏了"。缺失在视图里明确显示（`rules_missing`），条数为 0。
        let rules_missing = !self.bound_rules_in_effect(&environment);
        // 链接不在效时条数**显示 0**：这个字段的含义是"实例实际会应用多少条规则"，
        // 而不是"账本里存了多少条"（后者由 rules 库的接口回答）。
        let rules_count = if rules_missing {
            0
        } else {
            self.expected_rules_count_lenient(&environment, state)
        };
        let listen = environment.listen();
        // 视图**不回显凭据**（契约：凭据的唯一出口是实例启动参数；视图与日志
        // 只暴露"是否启用"布尔）。所以代理命令不带凭据 —— 启用了鉴权的环境，
        // 用户复制后按 `-x http://<user>:<password>@<host>:<port>` 自行补上。
        let proxy_command =
            format!("export https_proxy=http://{listen} http_proxy=http://{listen}");
        Ok(EnvView {
            name: name.clone(),
            listen,
            rules: environment.rules().map(str::to_string),
            insecure_hosts: environment.insecure_hosts().to_vec(),
            description: environment.description().to_string(),
            desired: state.desired_of(&name),
            health: self.health_blocking(&name, &environment, state),
            rules_count,
            rules_missing,
            proxy_command,
            proxy_auth_enabled: environment.proxy_auth_enabled(),
        })
    }

    /// 规则条数：规则不可用时返回 0，**不抛错**（给列表与健康展示用）。
    fn expected_rules_count_lenient(
        &self,
        environment: &Environment,
        state: &PersistedState,
    ) -> usize {
        self.expected_rules_count(environment, state).unwrap_or(0)
    }

    /// `view_of` 里的同步健康判定：只用内存状态与同步端口（不 await core）。
    ///
    /// 需要一个不 await 的版本是因为列表操作要一次算很多环境。但它**必须做真检查**：
    /// 早先的实现只要 `handles` 表里有这个 key 就返回 `Running`，于是 `kill -9` 掉实例
    /// 进程后工作台会一直报 running（实测 60 秒后仍然如此）——
    /// 这正是"编排层说在跑、实际早就死了"的经典错觉。现在它走
    /// [`Self::verdict_blocking`]：进程存活（`/proc` 状态位，僵尸不算活）→ 状态文件时效
    /// → 配置回执比对（哈希 + 规则条数，都在收敛窗口内）。
    ///
    /// **环境定义必须由调用方传进来**（它已经解析好了）：`view_of` 有几个调用点是在
    /// 持有 `state` 锁的情况下调它的，而 `std::sync::Mutex` 不可重入 —— 这里再去
    /// `require()` 拿一次状态就会自锁死（踩过：`env add` 直接挂住）。
    fn health_blocking(
        &self,
        name: &str,
        environment: &Environment,
        state: &PersistedState,
    ) -> InstanceHealth {
        // 判定要读文件与 `/proc`，失败不该把整个列表打挂：退回"标记里的结论"或 Stopped。
        match self.verdict_blocking(name, environment, state) {
            Ok(health) => health,
            Err(_) => self
                .stored_mark(name, environment, state)
                .unwrap_or(InstanceHealth::Stopped),
        }
    }

    /// 期望的规则条数：优先用**注入器真正在用的那个文件**（软链指向的物化文件），
    /// 文件不在时退回账本里的正文 —— 两者在正常情况下逐字节相同。
    fn expected_rules_count(
        &self,
        environment: &Environment,
        state: &PersistedState,
    ) -> Result<usize, Error> {
        let Some(name) = environment.rules() else {
            return Ok(0);
        };
        // **账本是唯一真相**：期望条数就是"账本里那条规则被接受多少条"。
        // 不去读物化文件 —— 它可能正好在自愈窗口里，而"期望什么"不该依赖那个瞬间。
        Ok(state
            .rules_entry(name)
            .map(|entry| envboard_rules::parse_hosts_text(&entry.rendered).accepted())
            .unwrap_or(0))
    }

    /// 该环境绑定的规则是不是**真的在生效**（固定名软链解析到绑定的那个名字）。
    fn bound_rules_in_effect(&self, environment: &Environment) -> bool {
        let Some(bound) = environment.rules() else {
            return true; // 没绑定：期望条数 0，注入器也报 0，这条断言仍然成立
        };
        crate::agent::linked_rules_name(
            &self
                .config
                .env_agent_dir(environment.name())
                .join(RULES_LINK_NAME),
        )
        .as_deref()
            == Some(bound)
    }

    /// 在区间内随机挑一个空闲端口。
    ///
    /// **探测是惰性的**：每次判定都是一次 `bind`，在 WSL2 mirrored 模式下实测单次
    /// 约 21ms，所以绝不能"先把整个区间探一遍"（1000 个候选就是约 21 秒）。
    /// 常见情形只探 1 次，最坏 `max_attempts` 次。
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

/// 日志里只印前 12 位 —— 哈希是给人对照的，不是给人读全文的。
fn short_hash(hash: &str) -> String {
    if hash.is_empty() {
        return "<none>".to_string();
    }
    hash.chars().take(12).collect()
}

fn replace_environment(state: &mut PersistedState, environment: &Environment) -> Result<(), Error> {
    replace_environment_by(state, environment.name(), environment)
}

/// 按**旧名字**定位并替换 —— 改名时新名字当然还不在状态里。
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

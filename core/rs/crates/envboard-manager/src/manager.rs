//! 环境管理器 —— v2 的产品本体。
//!
//! 职责：环境 CRUD、端口分配与持久化、进程编排、状态持久化、健康检查、reconcile。
//! 它只认识 [`ProxyCore`]，不认识 mitmproxy。
//!
//! 这里把契约里几条"容易实现错"的规则集中落地，每条都有测试钉住：
//!
//! * 端口：自动分配 / 显式指定 / 启动失败重试一次 / 已持久化端口**禁止静默重分配**；
//! * 生命周期：期望状态与实际状态解耦，reconcile 顺序固定（先清理、后启动）；
//! * 健康：**状态文件为主、探活为辅**，并且要比对"生效配置回显"。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use envboard_core_api::{
    ClockPort, CoreCapabilities, CoreInfo, Error, ErrorCode, InstanceHandle, InstanceHealth,
    InstanceSpec, Listen, LogLevel, LoggerPort, ProcessIdentity, ProxyCore, StatusReport,
};
use envboard_domain::{
    Action, Desired, Environment, InstanceRecord, PortRequest, ReconcilePlan, candidate_ports,
    plan_reconcile, seed_from, select_port,
};
use serde_json::{Map, Value, json};

use crate::ports::{FilePort, PortProbe, ProcessTable, StateRepo};
use crate::state::{InstanceLock, ManagerConfig, PersistedState, StoredError};

/// 一个环境对外的视图（工作台与 CLI 都渲染它）。
#[derive(Debug, Clone)]
pub struct EnvView {
    pub name: String,
    pub listen: Listen,
    pub rules: Option<String>,
    /// 透传给 core 的每实例选项（**已持久化**在环境定义里，启动与 reconcile 都用它）。
    pub options: BTreeMap<String, String>,
    pub description: String,
    pub desired: Desired,
    pub health: InstanceHealth,
    /// 生效后的规则条数（读文件解析得出）。
    pub rules_count: usize,
    pub proxy_command: String,
    /// 代理鉴权是否启用。只给布尔，**不回显凭据** —— 视图会被列表/详情/SSE 广播，
    /// 凭据只存在状态存储里。
    pub proxy_auth_enabled: bool,
}

impl EnvView {
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "listen": {"host": self.listen.host.to_string(), "port": self.listen.port},
            "rules": self.rules,
            "options": self.options,
            "description": self.description,
            "desired": match self.desired { Desired::Running => "running", Desired::Stopped => "stopped" },
            "health": self.health.as_str(),
            "health_reason": self.health.reason(),
            "rules_count": self.rules_count,
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
        Ok(Self {
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
        })
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
        state.environments.push(environment.to_json());
        state.desired.insert(name.clone(), Desired::Stopped);
        state.auto_port.insert(name.clone(), !explicit_port);
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
    /// 选项**只来自环境上持久化的 `options`**（没有"本次启动临时覆盖"的通道）：
    /// 否则手动启动的实例与账本里的配置会分叉，reconcile 之后又按账本把它拉回另一套值。
    pub async fn start(&self, name: &str) -> Result<EnvView, Error> {
        let raw = self.require(name)?;
        let environment = Environment::from_json(&raw)?;
        let (rules_path, _) = self.resolve_rules(&environment)?;

        match self.launch(name, &environment, rules_path).await {
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
        self.repo.save(&state)?;
        Ok(())
    }

    /// 导入规则文件：解析 → **规范化渲染** → 原子落盘（0600）。
    ///
    /// 输入由人手写、不保证整洁；输出是确定性的（`core/spec/rules.md` §6）。
    /// 注意"非法内容不让整次导入失败"是**有意的**：一个笔误不该让其余几百条一起报废。
    pub fn import_rules(&self, name: &str, source_text: &str) -> Result<PathBuf, Error> {
        envboard_domain::validate_rules_name(name)?;
        let import = envboard_rules::parse_hosts_text(source_text);
        let rendered = envboard_rules::render_rules(&import.entries, name)?;

        let path = self.config.rules_path(name);
        std::fs::create_dir_all(&self.config.rules_dir)?;
        let tmp = path.with_extension("rules.tmp");
        std::fs::write(&tmp, rendered.as_bytes())?;
        restrict_mode(&tmp)?;
        std::fs::rename(&tmp, &path)?;

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

    /// 期望状态与实际状态对齐。顺序固定：**先清理孤儿，再拉起**。
    pub async fn reconcile(&self) -> Result<ReconcileReport, Error> {
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
    /// 因此还能拿到 core 侧的选项回显比对；视图层是同步的、`await` 不了，
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

        self.status_health(&environment)
    }

    /// PATCH 语义的更新（工作台与 CLI 共用）。
    ///
    /// 三条规则值得说清楚：
    ///
    /// * **改描述随时可以**（纯展示字段）；
    /// * **规则文件的内容是热更新** —— 注入器按 mtime 重载**已绑定的那个路径**，不必重启；
    /// * **改身份（改名 / 换端口）、换绑定、换选项与改 `proxy_auth` 必须先是停止状态**。
    ///   这不是保守，是事实：实例在启动时通过 `--set envboard_rules=<path>` /
    ///   `--set <key>=<value>` / `--set proxyauth=…` 拿到规则路径、选项与凭据，运行中换
    ///   它们根本看不见 —— 于是"配置说绑了 A、实例还在按 B 干活"，正是那种"看起来成功了
    ///   但到处都对不上"的故障（状态文件的 `rules_count` / `options_echo` 回显会把这种
    ///   不一致如实报成 `config_mismatch`）。改名/换端口同理：客户端代理配置、状态文件、
    ///   进程记录会同时失效。
    pub fn update(&self, name: &str, patch: &Value) -> Result<EnvView, Error> {
        let raw = self.require(name)?;
        let environment = Environment::from_json(&raw)?;
        let merged = environment.merged(patch)?;

        let identity_changed =
            merged.name() != environment.name() || merged.listen() != environment.listen();
        let binding_changed = merged.rules() != environment.rules();
        let options_changed = merged.options() != environment.options();
        let auth_changed = merged.proxy_auth() != environment.proxy_auth();
        if (identity_changed || binding_changed || options_changed || auth_changed)
            && self.is_live(name)
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                format!(
                    "environment {name:?} is running; stop it before changing its name, listen \
                     address, rules binding, instance options or proxy_auth (an instance reads \
                     the rules path, the options and the credentials at launch, so new values \
                     cannot take effect; the bound file's contents do reload live). Description \
                     changes are allowed while running."
                ),
            ));
        }

        // 改了端口/绑定/选项/鉴权，**旧的失败标记就不再成立**：`port_conflict` 记的是"这个端口被占"，
        // `config_mismatch` 记的是"回显与这份配置对不上"（很可能正是某个选项或凭据造成的）。
        // 不清理的话，用户刚把端口换到空位上，界面仍会按旧标记报冲突 —— 而且用的是**新**端口号，
        // 等于在说谎。
        let mark_is_stale = merged.listen() != environment.listen()
            || binding_changed
            || options_changed
            || auth_changed;

        let mut state = self.state.lock().unwrap();
        if merged.name() != environment.name() && state.find(merged.name()).is_some() {
            return Err(Error::new(
                ErrorCode::Conflict,
                format!("environment {:?} already exists", merged.name()),
            ));
        }
        replace_environment_by(&mut state, environment.name(), &merged)?;
        if mark_is_stale {
            state.marks.remove(environment.name());
        }
        if merged.name() != environment.name() {
            // 改名：期望状态、自动分配标记、异常标记与进程记录一起搬过去
            // （四种 map 的 value 类型不同，所以分开搬而不是放进一个循环）
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
        }
        self.repo.save(&state)?;
        let mut changed: Vec<&str> = Vec::new();
        if merged.name() != environment.name() {
            changed.push("name");
        }
        if merged.listen() != environment.listen() {
            changed.push("listen");
        }
        if merged.rules() != environment.rules() {
            changed.push("rules");
        }
        if options_changed {
            changed.push("options");
        }
        if auth_changed {
            changed.push("proxy_auth");
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
        // 端口换了就不再是"随机分配的"了吗？仍然是 —— 它同样由管理器分配，因此将来
        // 再撞端口仍可自动换一次。
        state.auto_port.insert(name.into(), true);
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

    /// 规则库清单（只列名字，名字即白名单）。
    pub fn rules_list(&self) -> Result<Vec<String>, Error> {
        let mut names = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.config.rules_dir) else {
            return Ok(names);
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "rules")
                && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
            {
                names.push(stem.to_string());
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn rules_read(&self, name: &str) -> Result<String, Error> {
        envboard_domain::validate_rules_name(name)?;
        let path = self.config.rules_path(name);
        if !self.files.exists(&path) {
            return Err(Error::at(
                ErrorCode::NotFound,
                "rules",
                format!("rules file {} does not exist", path.display()),
            ));
        }
        self.files.read_to_string(&path)
    }

    /// 删除规则文件。**被任何环境绑定时拒绝** —— 否则那个环境会静默失去覆盖。
    pub fn rules_delete(&self, name: &str) -> Result<(), Error> {
        envboard_domain::validate_rules_name(name)?;
        {
            let state = self.state.lock().unwrap();
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
        }
        let path = self.config.rules_path(name);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
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
            let ip = match environment.rules() {
                None => None,
                Some(name) => {
                    let path = self.config.rules_path(name);
                    if self.files.exists(&path) {
                        let text = self.files.read_to_string(&path)?;
                        envboard_rules::parse_hosts_text(&text)
                            .entries
                            .get(&wanted)
                            .cloned()
                    } else {
                        None
                    }
                }
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
    fn status_health(&self, environment: &Environment) -> Result<InstanceHealth, Error> {
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

        // 契约回显比对宿主对未知/拼错的 `--set` 是**静默忽略**的，
        // 所以"命令没报错"不能当作配置生效。
        let capabilities = self.capabilities();
        if capabilities.reports_rules_count {
            let expected = self.expected_rules_count(environment)?;
            if status.rules_count != expected {
                return Ok(InstanceHealth::ConfigMismatch {
                    reason: format!(
                        "instance reports rules_count={} but the bound rules file has {expected} \
                         entries (a host may have silently ignored a --set option)",
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
    /// 而不是 `probe`（后者顺带做了选项回显比对，见下面 `health` 的注释）。
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

        self.status_health(environment)
    }

    /// 进程是否还活着（身份三级匹配；僵尸**不算**活着，见 `infra::ProcfsProcessTable`）。
    fn process_is_alive(&self, expected: &ProcessIdentity) -> bool {
        self.processes
            .identity(expected.pid)
            .is_some_and(|identity| identity.same_process(expected))
    }

    async fn launch(
        &self,
        name: &str,
        environment: &Environment,
        rules_path: Option<std::path::PathBuf>,
    ) -> Result<InstanceHandle, Error> {
        let options = environment.options();
        let spec = InstanceSpec {
            env: name.to_string(),
            listen: environment.listen(),
            rules: rules_path,
            status_file: self.config.status_file(name),
            shared_state_dir: self.config.confdir.clone(),
            runtime_dir: self.config.runtime_dir.clone(),
            log_dir: self.config.log_dir.clone(),
            options: options.clone(),
            proxy_auth: environment.proxy_auth().map(str::to_string),
        };
        // denylist 在 core 侧也会跑一次；这里先跑一遍，好在错误里带上正确的字段路径。
        envboard_core_api::validate_options(options)?;
        std::fs::create_dir_all(&self.config.runtime_dir)?;
        std::fs::create_dir_all(&self.config.confdir)?;
        // 启动前照看一次体积：新的一段日志不该写进一个"下一行就触发轮转"的文件。
        if let Some(path) = self.config.log_file(name) {
            let _ = crate::logs::rotate_if_needed(&path, self.config.max_log_bytes);
        }
        self.core.start(spec).await
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
            self.repo.save(&state)?;
        }
        self.logger
            .log(LogLevel::Info, &format!("{name}: re-allocated port {port}"));
        let (rules_path, _) = self.resolve_rules(&updated)?;
        self.launch(name, &updated, rules_path).await
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

    fn view_of(&self, state: &PersistedState, raw: &Value) -> Result<EnvView, Error> {
        let environment = Environment::from_json(raw)?;
        let name = environment.name().to_string();
        // 列表/详情**不能**因为某个环境的规则文件缺失就整体失败：那是"一个环境坏了"
        // 而不是"状态坏了"。缺失由 `start` 响亮失败、并在视图里体现为条数 0。
        let rules_count = self.expected_rules_count_lenient(&environment);
        let listen = environment.listen();
        // 视图**不回显凭据**（契约：proxy_auth 的唯一出口是实例启动参数；视图与日志
        // 只暴露"是否启用"布尔）。所以代理命令不带凭据 —— 启用了鉴权的环境，
        // 用户复制后按 `-x http://<proxy_auth>@<host>:<port>` 自行补上。
        let proxy_command =
            format!("export https_proxy=http://{listen} http_proxy=http://{listen}");
        Ok(EnvView {
            name: name.clone(),
            listen,
            rules: environment.rules().map(str::to_string),
            options: environment.options().clone(),
            description: environment.description().to_string(),
            desired: state.desired_of(&name),
            health: self.health_blocking(&name, &environment, state),
            rules_count,
            proxy_command,
            proxy_auth_enabled: environment.proxy_auth().is_some(),
        })
    }

    /// 规则条数：文件缺失/读不动时返回 0，**不抛错**（给列表与健康展示用）。
    fn expected_rules_count_lenient(&self, environment: &Environment) -> usize {
        let Some(name) = environment.rules() else {
            return 0;
        };
        let path = self.config.rules_path(name);
        if !self.files.exists(&path) {
            return 0;
        }
        match self.files.read_to_string(&path) {
            Ok(text) => envboard_rules::parse_hosts_text(&text).accepted(),
            Err(_) => 0,
        }
    }

    /// `view_of` 里的同步健康判定：只用内存状态与同步端口（不 await core）。
    ///
    /// 需要一个不 await 的版本是因为列表操作要一次算很多环境。但它**必须做真检查**：
    /// 早先的实现只要 `handles` 表里有这个 key 就返回 `Running`，于是 `kill -9` 掉实例
    /// 进程后工作台会一直报 running（实测 60 秒后仍然如此）——
    /// 这正是"编排层说在跑、实际早就死了"的经典错觉。现在它走
    /// [`Self::verdict_blocking`]：进程存活（`/proc` 状态位，僵尸不算活）→ 状态文件时效
    /// → 契约回显比对的规则条数。
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

    fn resolve_rules(
        &self,
        environment: &Environment,
    ) -> Result<(Option<std::path::PathBuf>, usize), Error> {
        let Some(name) = environment.rules() else {
            return Ok((None, 0));
        };
        let path = self.config.rules_path(name);
        if !self.files.exists(&path) {
            return Err(Error::at(
                ErrorCode::NotFound,
                "environment.rules",
                format!("rules file {} does not exist", path.display()),
            ));
        }
        let expected = self.expected_rules_count(environment)?;
        Ok((Some(path), expected))
    }

    /// 期望的规则条数 = 规则文件里被接受的条目数（解析器说了算）。
    fn expected_rules_count(&self, environment: &Environment) -> Result<usize, Error> {
        let Some(name) = environment.rules() else {
            return Ok(0);
        };
        let path = self.config.rules_path(name);
        if !self.files.exists(&path) {
            return Ok(0);
        }
        let text = self.files.read_to_string(&path)?;
        Ok(envboard_rules::parse_hosts_text(&text).accepted())
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

fn restrict_mode(path: &std::path::Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

use std::path::PathBuf;

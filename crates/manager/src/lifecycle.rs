//! 域：环境实例生命周期 —— 环境的创建、启停、删除与更新。
//!
//! 收什么：`create` / `start` / `stop` / `restart` / `remove` / `update` /
//! `reallocate_port` / `is_live`，以及只服务于本域的私有机制：启动并等待
//! 绑定定态（launch）、启动成功后的记账（record_started）、端口冲突的
//! 一次性重分配重试（reallocate_and_retry）、规则/上游绑定的存在性校验
//! （require_known_rules / require_known_upstream）与账本内环境替换
//! （replace_environment*）。
//!
//! 跨域共享的私有机制（commit、mark、hot_update_engine、require、view_of、
//! engine_spec_locked、allocate_port）不在这里，见 manager.rs 与各所属域。

use std::collections::BTreeSet;
use std::time::Duration;

use envboard_engine::domain::{Desired, Environment};
use envboard_engine::{EngineHandle, Error, ErrorCode, InstanceState, LogLevel};
use envboard_protocol::EnvView;
use serde_json::{Map, Value};

use crate::manager::Manager;
use crate::state::PersistedState;
use envboard_events::ControlEvent;

/// 启动后等待绑定定态的预算（绑定是本地 bind，毫秒级；给足宽容只防极端调度）。
const SETTLE_BUDGET: Duration = Duration::from_secs(5);
const SETTLE_POLL: Duration = Duration::from_millis(10);

impl Manager {
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
        self.require_known_upstream(&state, &environment)?;
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

    /// 热/停机矩阵（契约的一部分）：
    ///
    /// * 热：description、insecure_hosts、rules 绑定、规则内容、upstream 绑定 ——
    ///   全部经同步 apply 装配即生效（上游连接每请求新建，换绑定没有残留）；
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
        let upstream_changed = merged.upstream() != environment.upstream();
        let insecure_changed = merged.insecure_hosts() != environment.insecure_hosts();
        let credentials_changed = merged.proxy_user() != environment.proxy_user()
            || merged.proxy_password() != environment.proxy_password();
        // capture 也是热字段：调试页「开启 / 切换」驱动的就是它。曾经它不在触发集合里，
        // 开关只落盘不热应用 —— 引擎手里的快照永远 capture=false，抓包计数恒 0，
        // 而页面忠实地把 0 渲染出来（坏的是数据源，不是渲染）。
        let capture_changed = merged.capture() != environment.capture();
        let hot_changed =
            binding_changed || upstream_changed || insecure_changed || capture_changed;
        if (identity_changed || credentials_changed) && self.is_live(name) {
            return Err(Error::new(
                ErrorCode::Conflict,
                format!(
                    "environment {name:?} is running; stop it before changing its name, listen \
                     address, proxy_user or proxy_password. Description, insecure_hosts, the \
                     rules binding and the upstream proxy binding are hot and can be changed \
                     while running."
                ),
            ));
        }

        // 改了端口/绑定/放行清单/鉴权，旧的失败标记就不再成立：留着它，界面会拿
        // 新端口号去报旧冲突 —— 等于在说谎。
        let mark_is_stale = identity_changed
            || binding_changed
            || upstream_changed
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
        self.require_known_upstream(&state, &merged)?;
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
        if upstream_changed {
            changed.push("upstream");
        }
        if insecure_changed {
            changed.push("insecure_hosts");
        }
        if credentials_changed {
            changed.push("proxy_user/proxy_password");
        }
        if capture_changed {
            changed.push("capture");
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

    /// 环境绑定的上游代理名必须在账本里（与 require_known_rules 同构）：
    /// 不放行"绑一个还不存在的名字"，否则那是静默直连。
    fn require_known_upstream(
        &self,
        state: &PersistedState,
        environment: &Environment,
    ) -> Result<(), Error> {
        let Some(name) = environment.upstream() else {
            return Ok(());
        };
        if state.proxy_entry(name).is_some() {
            return Ok(());
        }
        Err(Error::invalid_config(
            "environment.upstream",
            format!(
                "upstream proxy {name:?} is not in the proxy ledger; create it first \
                 (a binding to a missing proxy would silently connect direct)"
            ),
        ))
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

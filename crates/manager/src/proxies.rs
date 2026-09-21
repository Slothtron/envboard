//! 域：上游代理账本（proxy library）—— 清单、读取、保存与删除。
//!
//! 收什么：`proxy_list` / `proxy_get` / `proxy_put` / `proxy_delete`，以及
//! 本域私有的视图构造与引用查询（proxy_view_of、proxy_not_found、
//! environments_bound_upstream_locked、retitle_field）。
//!
//! 与规则账本同构：`state.json` 是唯一真相；同名即覆盖；引用它的运行中
//! 环境经 `hot_update_engine` 当场换快照；被引用时删除一律拒绝并点名引用方。

use envboard_engine::domain::{Environment, UpstreamProxy, validate_upstream_name};
use envboard_engine::{Error, ErrorCode, LogLevel};
use envboard_protocol::ProxyView;
use serde_json::{Value, json};

use crate::manager::Manager;
use crate::state::PersistedState;
use envboard_events::ControlEvent;

impl Manager {
    /// 上游代理清单（按名字序，与账本存储顺序一致）。
    pub fn proxy_list(&self) -> Result<Vec<ProxyView>, Error> {
        let state = self.state.lock().unwrap();
        Ok(state
            .proxies
            .iter()
            .filter_map(|raw| self.proxy_view_of(&state, raw).ok())
            .collect())
    }

    pub fn proxy_get(&self, name: &str) -> Result<ProxyView, Error> {
        validate_upstream_name(name).map_err(|error| retitle_field(error, "proxy.name"))?;
        let state = self.state.lock().unwrap();
        let raw = state
            .proxy_entry(name)
            .cloned()
            .ok_or_else(|| self.proxy_not_found(name))?;
        self.proxy_view_of(&state, &raw)
    }

    /// 保存（创建/整体替换）一条上游代理：校验 → 账本 → 持久化 → 热应用引用环境。
    ///
    /// 账本是唯一真相；同名即覆盖（与规则导入同构）。引用它的运行中环境当场
    /// 换快照 —— 上游连接每请求新建，改 host/port/凭据下一请求即生效。
    pub fn proxy_put(&self, body: &Value) -> Result<ProxyView, Error> {
        let proxy = UpstreamProxy::from_json(body)?;
        let json = proxy.to_json();
        let name = proxy.name().to_string();

        let mut state = self.state.lock().unwrap();
        match state
            .proxies
            .iter_mut()
            .find(|raw| raw.get("name").and_then(Value::as_str) == Some(&name))
        {
            Some(existing) => *existing = json.clone(),
            None => state.proxies.push(json.clone()),
        }
        state.proxies.sort_by_key(|raw| {
            raw.get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        });
        // 控制面写入必须走 commit（先 save 后 emit）：代理账本变更同样是
        // 可审计的动作，不许旁路事件日志。
        self.commit(
            &state,
            ControlEvent::Custom {
                kind: "proxy/saved".to_string(),
                payload: json!({ "name": name }),
            },
        )?;
        let affected = self.environments_bound_upstream_locked(&state, &name);
        drop(state);

        self.logger.log(
            LogLevel::Info,
            &format!(
                "saved upstream proxy {name} → {}:{} (auth: {}); {} referencing environment(s) \
                 hot-applied",
                proxy.host(),
                proxy.port(),
                if proxy.has_auth() { "yes" } else { "no" },
                affected.len(),
            ),
        );
        for env in affected {
            self.hot_update_engine(&env);
        }
        self.proxy_get(&name)
    }

    /// 删除上游代理。被任何环境引用时拒绝并**点名全部引用方**（ADR-6）：
    /// 静默降级成直连会让用户以为流量还在走代理。
    pub fn proxy_delete(&self, name: &str) -> Result<(), Error> {
        validate_upstream_name(name).map_err(|error| retitle_field(error, "proxy.name"))?;
        let mut state = self.state.lock().unwrap();
        let referencing = self.environments_bound_upstream_locked(&state, name);
        if !referencing.is_empty() {
            return Err(Error::new(
                ErrorCode::Conflict,
                format!(
                    "upstream proxy {name:?} is referenced by environment(s) {referencing:?}; \
                     unbind them first (a dangling reference would silently connect direct)"
                ),
            ));
        }
        if state.proxy_entry(name).is_none() {
            return Err(self.proxy_not_found(name));
        }
        state
            .proxies
            .retain(|raw| raw.get("name").and_then(Value::as_str) != Some(name));
        self.commit(
            &state,
            ControlEvent::Custom {
                kind: "proxy/deleted".to_string(),
                payload: json!({ "name": name }),
            },
        )?;
        drop(state);
        self.logger
            .log(LogLevel::Info, &format!("deleted upstream proxy {name}"));
        Ok(())
    }

    fn proxy_view_of(&self, state: &PersistedState, raw: &Value) -> Result<ProxyView, Error> {
        let proxy = UpstreamProxy::from_json(raw)?;
        Ok(ProxyView {
            name: proxy.name().to_string(),
            host: proxy.host().to_string(),
            port: proxy.port(),
            has_auth: proxy.has_auth(),
            references: self.environments_bound_upstream_locked(state, proxy.name()),
        })
    }

    fn proxy_not_found(&self, name: &str) -> Error {
        Error::at(
            ErrorCode::NotFound,
            "proxy",
            format!("upstream proxy {name:?} does not exist"),
        )
    }

    /// 引用某条上游代理的全部环境名（热应用的影响面）。
    fn environments_bound_upstream_locked(
        &self,
        state: &PersistedState,
        name: &str,
    ) -> Vec<String> {
        state
            .environments
            .iter()
            .filter_map(|raw| Environment::from_json(raw).ok())
            .filter(|environment| environment.upstream() == Some(name))
            .map(|environment| environment.name().to_string())
            .collect()
    }
}

/// domain 层的名字白名单报错带着 `environment.upstream` 字段路径；代理实体的
/// 校验语义相同，只把路径改到 `proxy.name`。
fn retitle_field(mut error: Error, field: &str) -> Error {
    error.field = Some(field.to_string());
    error
}

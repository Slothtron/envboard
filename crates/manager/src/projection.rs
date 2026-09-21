//! 域：投影视图 —— 账本 + 健康裁决 → UI 词汇（DTO）的构造与跨环境对比。
//!
//! 收什么：`view_of`（EnvView 的唯一构造点，list/get/create/update 都经它）、
//! 期望规则数（expected_rules_count）与跨环境静态对比（`compare`）。
//!
//! 视图构造只读账本与裁决，从不写入；健康裁决本身是 `Manager::verdict`
//! （manager.rs 的跨域共享项）—— 视图与权威判定共用同一实现，不可能分叉。

use envboard_engine::Error;
use envboard_engine::domain::{Desired, Environment};
use envboard_protocol::{Desired as WireDesired, Endpoint, EnvView};
use serde_json::{Value, json};

use crate::manager::Manager;
use crate::state::PersistedState;

impl Manager {
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

    pub(crate) fn view_of(&self, state: &PersistedState, raw: &Value) -> Result<EnvView, Error> {
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
        let health = self.verdict(&name, &environment, state);
        Ok(EnvView {
            name: name.clone(),
            listen: Endpoint {
                host: listen.host.to_string(),
                port: listen.port,
            },
            rules: environment.rules().map(str::to_string),
            upstream: environment.upstream().map(str::to_string),
            insecure_hosts: environment.insecure_hosts().to_vec(),
            description: environment.description().to_string(),
            capture: environment.capture(),
            desired: match state.desired_of(&name) {
                Desired::Running => WireDesired::Running,
                Desired::Stopped => WireDesired::Stopped,
            },
            health: health.as_str().to_string(),
            health_reason: health.reason().map(str::to_string),
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
}

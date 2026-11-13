//! 域：期望状态对齐与健康判定。
//!
//! 收什么：`reconcile`（先清理、后启动的固定顺序；占用探测只服务于规划）
//! 与 `health`（v3：内存报告，视图与权威判定共用 `Manager::verdict`）。
//!
//! 起停动作本身不在这里 —— 它复用生命周期域的 `start`/`stop` 公开方法。

use std::collections::BTreeSet;

use envboard_engine::domain::{
    Action, Desired, Environment, InstanceRecord, ReconcilePlan, plan_reconcile,
};
use envboard_engine::{Error, ErrorCode, InstanceState, LogLevel};
use envboard_protocol::ReconcileReport;

use crate::manager::Manager;

impl Manager {
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

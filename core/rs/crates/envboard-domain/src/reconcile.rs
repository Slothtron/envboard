//! 期望状态与实际状态的对齐（`instance.reconcile`）—— 纯决策，不做 I/O。
//!
//! 契约见 `core/spec/capabilities.md`（§实例生命周期）。两条容易写错、
//! 且都有 golden fixture 钉住的规则：
//!
//! 1. **孤儿清理必须比对 PID + 启动时刻 + cmdline**。只比 PID 存活会误杀 PID 复用后的
//!    无关进程；只比 PID + cmdline 也不够 —— 同一份配置的实例 cmdline **完全相同**，
//!    PID 复用后照样撞上。任一项不匹配就**不动那个进程**，只记告警。
//! 2. **顺序固定**：先清理孤儿，再拉起期望 running 的环境。
//! 3. **上一代实例要重启**：在跑但状态文件里没有配置哈希回执的实例，跑的是旧通道下的
//!    配置，`Keep` 会让分叉一直留着 —— 它走 `Restart`，不参与"没坏就不动"。

use std::collections::BTreeSet;

use envboard_core_api::ProcessIdentity;

/// 持久化的期望状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Desired {
    Running,
    Stopped,
}

/// 一个环境在 reconcile 时的输入快照。
#[derive(Debug, Clone)]
pub struct InstanceRecord {
    pub env: String,
    pub desired: Desired,
    /// 上次启动时记录下来的进程身份（`None` = 从没启动过）。
    pub record: Option<ProcessIdentity>,
    /// 现在实际在跑的那个进程（`None` = 没有活着）。
    pub live: Option<ProcessIdentity>,
    /// 该环境的持久化端口。只有 `desired = running` 且没在跑时才用得上。
    pub listen_port: Option<u16>,
    /// 在跑的实例是**上一代二进制**拉起来的（状态文件里没有配置哈希回执）。
    ///
    /// 那一代实例拿不到"配置文件 + 固定名软链"这套通道，因此它跑的配置与账本必然
    /// 分叉；`Keep` 会让分叉一直留着，所以这里要**重启一次**把它拉齐。
    pub legacy: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    /// 什么都不做（进程还活着 / 不是我们的进程）。
    Keep,
    /// 端口被别的程序占走：标记 `port_conflict`，**不换端口**。
    MarkConflict,
    /// 在跑，但那是上一代二进制拉起来的实例：停掉再按当前配置拉起一次。
    Restart,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Start => "start",
            Action::Stop => "stop",
            Action::Keep => "keep",
            Action::MarkConflict => "mark_conflict",
            Action::Restart => "restart",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileWarning {
    /// PID 相同但启动时刻不同（或 PID 不同）：PID 复用，别动它。
    PidReuseSuspected,
    /// PID + 启动时刻相同但 cmdline 不同：也不是我们的进程。
    CmdlineMismatch,
    /// 在跑，但没有记录可对——无法归属，因此不动它。
    UntrackedProcess,
}

impl ReconcileWarning {
    pub fn as_str(self) -> &'static str {
        match self {
            ReconcileWarning::PidReuseSuspected => "pid_reuse_suspected",
            ReconcileWarning::CmdlineMismatch => "cmdline_mismatch",
            ReconcileWarning::UntrackedProcess => "untracked_process",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcilePlan {
    /// 有序动作列表：**先清理、后启动**。
    pub actions: Vec<(String, Action)>,
    pub warnings: Vec<ReconcileWarning>,
}

impl ReconcilePlan {
    pub fn actions_of(&self, action: Action) -> impl Iterator<Item = &str> {
        self.actions
            .iter()
            .filter(move |(_, candidate)| *candidate == action)
            .map(|(env, _)| env.as_str())
    }
}

/// 算出这一轮要做什么（调用方按顺序执行，见契约的"顺序固定"）。
pub fn plan_reconcile(
    instances: &[InstanceRecord],
    occupied_by_others: &BTreeSet<u16>,
) -> ReconcilePlan {
    let mut plan = ReconcilePlan::default();

    // 第一遍：清理。只处理"确实在跑"的实例。
    for instance in instances {
        let Some(live) = instance.live.as_ref() else {
            continue;
        };
        let Some(record) = instance.record.as_ref() else {
            plan.warnings.push(ReconcileWarning::UntrackedProcess);
            plan.actions.push((instance.env.clone(), Action::Keep));
            continue;
        };

        if record.same_process(live) {
            if instance.desired == Desired::Stopped {
                plan.actions.push((instance.env.clone(), Action::Stop));
            } else if instance.legacy {
                // 上一代实例配置必然与账本分叉：重启一次把它拉齐。
                plan.actions.push((instance.env.clone(), Action::Restart));
            } else {
                plan.actions.push((instance.env.clone(), Action::Keep));
            }
        } else if record.same_process_different_cmdline(live) {
            plan.warnings.push(ReconcileWarning::CmdlineMismatch);
            plan.actions.push((instance.env.clone(), Action::Keep));
        } else {
            plan.warnings.push(ReconcileWarning::PidReuseSuspected);
            plan.actions.push((instance.env.clone(), Action::Keep));
        }
    }

    // 第二遍：启动。顺序固定 —— 清理做完才拉起。
    for instance in instances {
        if instance.desired != Desired::Running || instance.live.is_some() {
            continue;
        }
        let conflicts = instance
            .listen_port
            .is_some_and(|port| occupied_by_others.contains(&port));
        let action = if conflicts {
            Action::MarkConflict
        } else {
            Action::Start
        };
        plan.actions.push((instance.env.clone(), action));
    }

    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(pid: i32, starttime: u64, port: u16) -> ProcessIdentity {
        ProcessIdentity {
            pid,
            starttime,
            cmdline: vec![
                "mitmdump".into(),
                "-s".into(),
                "/home/u/.envboard/agent/envboard_mitmproxy.py".into(),
                "--set".into(),
                format!("listen_port={port}"),
            ],
        }
    }

    #[test]
    fn pid_reuse_is_never_killed() {
        let plan = plan_reconcile(
            &[InstanceRecord {
                env: "alpha".into(),
                desired: Desired::Stopped,
                record: Some(identity(111, 900, 16_301)),
                live: Some(identity(111, 1200, 16_301)),
                listen_port: Some(16_301),
                legacy: false,
            }],
            &BTreeSet::new(),
        );
        assert_eq!(plan.actions, vec![("alpha".to_string(), Action::Keep)]);
        assert_eq!(plan.warnings, vec![ReconcileWarning::PidReuseSuspected]);
    }

    #[test]
    fn cleanup_runs_before_starts() {
        let plan = plan_reconcile(
            &[
                InstanceRecord {
                    env: "beta".into(),
                    desired: Desired::Running,
                    record: None,
                    live: None,
                    listen_port: Some(16_302),
                    legacy: false,
                },
                InstanceRecord {
                    env: "alpha".into(),
                    desired: Desired::Stopped,
                    record: Some(identity(111, 900, 16_301)),
                    live: Some(identity(111, 900, 16_301)),
                    listen_port: Some(16_301),
                    legacy: false,
                },
            ],
            &BTreeSet::new(),
        );
        assert_eq!(
            plan.actions,
            vec![
                ("alpha".to_string(), Action::Stop),
                ("beta".to_string(), Action::Start)
            ]
        );
    }

    #[test]
    fn occupied_port_marks_conflict_instead_of_reallocating() {
        let occupied: BTreeSet<u16> = [16_301u16].into_iter().collect();
        let plan = plan_reconcile(
            &[InstanceRecord {
                env: "beta".into(),
                desired: Desired::Running,
                record: None,
                live: None,
                listen_port: Some(16_301),
                legacy: false,
            }],
            &occupied,
        );
        assert_eq!(
            plan.actions,
            vec![("beta".to_string(), Action::MarkConflict)]
        );
    }

    #[test]
    fn runs_from_the_previous_generation_are_restarted_not_kept() {
        // 上一代实例拿不到"配置文件 + 固定名软链"通道，配置必然与账本分叉：
        // 期望 running 时它不是 Keep 而是 Restart。
        let plan = plan_reconcile(
            &[InstanceRecord {
                env: "beta".into(),
                desired: Desired::Running,
                record: Some(identity(111, 900, 16_301)),
                live: Some(identity(111, 900, 16_301)),
                listen_port: Some(16_301),
                legacy: true,
            }],
            &BTreeSet::new(),
        );
        assert_eq!(plan.actions, vec![("beta".to_string(), Action::Restart)]);
        assert!(plan.warnings.is_empty());
    }
}

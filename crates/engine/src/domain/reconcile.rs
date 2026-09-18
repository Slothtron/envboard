//! 期望状态与实际状态的对齐（instance.reconcile）—— 纯决策，不做 I/O。
//!
//! v3 的"实际在跑"是**引擎实例的存活性**（同进程的线程/运行时状态，由管理面
//! 读内存报告得出）—— 不再是 OS 进程身份。v2 在这里的三条判据随子进程模型退场：
//! PID+starttime+cmdline 三重比对、PID 复用告警、上一代实例重启。它们防的是
//! "跨进程边界的身份错认"；实例不出进程边界，错认就没有发生的介质。
//!
//! 保留并有 golden fixture 钉住的规则：
//!
//! 1. **顺序固定**：先清理（stop），后启动（start/mark_conflict）。
//! 2. **desired=running 且没在跑、而端口被别的程序占着 → 标记冲突，绝不自动换端口**
//!    （已持久化端口是环境的对外身份）。
//! 3. **failed 自愈走 Start**：live=false 统一按期望重新拉起；"该不该重启"由
//!    desired 裁决，不在这里做退避（节奏归管理面的 reconcile 循环）。

use std::collections::BTreeSet;

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
    /// 引擎实例当前是否在跑（内存报告的 starting/running 即 true）。
    pub live: bool,
    /// 该环境的持久化端口。只有 desired = running 且没在跑时才用得上。
    pub listen_port: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    /// 什么都不做（已在跑且期望 running，或本就没在跑且期望 stopped）。
    Keep,
    /// 端口被别的程序占走：标记 port_conflict，**不换端口**。
    MarkConflict,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Start => "start",
            Action::Stop => "stop",
            Action::Keep => "keep",
            Action::MarkConflict => "mark_conflict",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcilePlan {
    /// 有序动作列表：**先清理、后启动**。
    pub actions: Vec<(String, Action)>,
    /// v3 没有"不可归属的进程"这类需要告警的形态；字段保留是给未来的
    /// 告警面（如自动重启退避）留的落点，恒空由 fixture 断言钉住。
    pub warnings: Vec<String>,
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

    // 第一遍：清理与在册确认。live 的实例要么被停，要么被记 keep（"没坏就不动"是
    // 可见的决策，不是静默缺省）。
    for instance in instances {
        if !instance.live {
            continue;
        }
        let action = if instance.desired == Desired::Stopped {
            Action::Stop
        } else {
            Action::Keep
        };
        plan.actions.push((instance.env.clone(), action));
    }

    // 第二遍：启动。已在跑的（含 failed 前被摘除登记的场合由调用方处理）不重复拉起。
    for instance in instances {
        if instance.desired != Desired::Running || instance.live {
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

    fn record(env: &str, desired: Desired, live: bool, port: u16) -> InstanceRecord {
        InstanceRecord {
            env: env.into(),
            desired,
            live,
            listen_port: Some(port),
        }
    }

    #[test]
    fn cleanup_runs_before_starts() {
        let plan = plan_reconcile(
            &[
                record("beta", Desired::Running, false, 16_302),
                record("alpha", Desired::Stopped, true, 16_301),
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
        assert!(plan.warnings.is_empty());
    }

    #[test]
    fn running_and_desired_running_is_kept() {
        let plan = plan_reconcile(
            &[record("beta", Desired::Running, true, 16_301)],
            &BTreeSet::new(),
        );
        assert_eq!(plan.actions, vec![("beta".to_string(), Action::Keep)]);
    }

    #[test]
    fn occupied_port_marks_conflict_instead_of_reallocating() {
        let occupied: BTreeSet<u16> = [16_301u16].into_iter().collect();
        let plan = plan_reconcile(
            &[record("beta", Desired::Running, false, 16_301)],
            &occupied,
        );
        assert_eq!(
            plan.actions,
            vec![("beta".to_string(), Action::MarkConflict)]
        );
    }

    #[test]
    fn stopped_and_not_live_gets_no_action() {
        let plan = plan_reconcile(
            &[record("beta", Desired::Stopped, false, 16_301)],
            &BTreeSet::new(),
        );
        assert!(plan.actions.is_empty());
    }
}

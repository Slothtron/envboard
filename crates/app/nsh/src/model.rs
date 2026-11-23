//! 业务语义：环境是什么、状态有哪几种、每种状态在界面上长什么样。
//!
//! 这一层**不画东西**。它只做三件事：把 `Health` 翻成 `(Tone, Shape, 文案)`，
//! 把规则集合算成状态条上的计数，把 fixture 数据摆好给页面挑。
//! 颜色与像素在 `envboard_ui`，布局在 `pages`。

use nsh_core::prelude::*;

use envboard_ui::primitives::{self as ui, Shape, Tone};

// ---- 状态 ------------------------------------------------------------------

/// 一个环境的五种状态。`Unhealthy` 能服务、但配置没装配成功 —— 所以它降级成
/// 提醒（黄），只有启动失败与端口冲突才够红。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Health {
    Running,
    Stopped,
    Unhealthy,
    PortConflict,
    Failed,
}

impl Health {
    /// 状态文案。`status_cell` 也用它，所以两处（列表与抽屉头）永远不会漂。
    pub fn label(self) -> &'static str {
        match self {
            Health::Running => "运行中",
            Health::Stopped => "已停止",
            Health::Unhealthy => "不健康",
            Health::PortConflict => "端口冲突",
            Health::Failed => "启动失败",
        }
    }

    /// 颜色语气与形状。红/橙从不单独承担语义，形状与文字必须同时在场。
    pub fn tone_shape(self) -> (Tone, Shape) {
        match self {
            Health::Running => (Tone::Success, Shape::Dot),
            Health::Stopped => (Tone::Neutral, Shape::Ring),
            Health::Unhealthy => (Tone::Caution, Shape::DotBar),
            Health::PortConflict => (Tone::Critical, Shape::DotVBar),
            Health::Failed => (Tone::Critical, Shape::DotBang),
        }
    }
}

/// 列表列与抽屉头都用这一个入口，`width` 由调用方给（列表给列宽，抽屉头给自然宽）。
pub fn status_cell_for(health: Health, width: f32) -> impl IntoElement {
    let (tone, shape) = health.tone_shape();
    ui::status_cell(tone, shape, health.label(), width)
}

// ---- 计数 ------------------------------------------------------------------

/// 状态条与筛选片要报的计数。它们由**同一份数据**算出，所以前后永远一致。
pub struct Counts {
    pub total: usize,
    pub running: usize,
    pub stopped: usize,
    /// 与「需处理」筛选片同一判据：非运行/停止，或规则集缺失。
    pub attention: usize,
    pub port_conflict: usize,
    pub rules_missing: usize,
}

/// 「需处理」的判据只有这一处：筛选片的第一行、状态条的告警句都从这里来。
pub fn needs_attention(e: &Env) -> bool {
    !matches!(e.health, Health::Running | Health::Stopped) || e.rules_missing
}

pub fn counts(envs: &[Env]) -> Counts {
    Counts {
        total: envs.len(),
        running: envs.iter().filter(|e| e.health == Health::Running).count(),
        stopped: envs.iter().filter(|e| e.health == Health::Stopped).count(),
        attention: envs.iter().filter(|e| needs_attention(e)).count(),
        port_conflict: envs
            .iter()
            .filter(|e| e.health == Health::PortConflict)
            .count(),
        rules_missing: envs.iter().filter(|e| e.rules_missing).count(),
    }
}

/// 中段那句话：没有要人处理的事就明说“无待处理告警”，不留空。
pub fn attention_line(envs: &[Env]) -> String {
    let c = counts(envs);
    if c.port_conflict == 0 && c.rules_missing == 0 {
        return "无待处理告警".to_owned();
    }
    format!(
        "{} 端口冲突 · {} 规则集缺失",
        c.port_conflict, c.rules_missing
    )
}

// ---- fixture ---------------------------------------------------------------

pub struct Env {
    pub id: usize,
    pub name: &'static str,
    pub health: Health,
    /// 自动分配的端口。列表的「监听」列用 `listen`（带 IP），这个数留给
    /// 抽屉的「复制端口」与 M2 的数据面。
    #[allow(dead_code)]
    pub port: u16,
    pub listen: &'static str,
    pub rules: &'static str,
    pub rules_missing: bool,
    pub last: &'static str,
}

/// 八个环境：允许的五种状态全部在场，外加一个“在跑但规则集缺失”的环境 ——
/// 状态条要报的两件事（不健康 / 规则缺失）因此可以同屏看。
pub static ENVS: [Env; 8] = [
    Env { id: 0, name: "proxy-a", health: Health::Running, port: 16301, listen: "127.0.0.1:16301", rules: "3 条", rules_missing: false, last: "刚刚" },
    Env { id: 1, name: "proxy-b", health: Health::Running, port: 16302, listen: "127.0.0.1:16302", rules: "2 条", rules_missing: false, last: "2 分钟前" },
    Env { id: 2, name: "proxy-c", health: Health::Unhealthy, port: 16303, listen: "127.0.0.1:16303", rules: "2 条", rules_missing: false, last: "1 分钟前" },
    Env { id: 3, name: "dev-local", health: Health::Stopped, port: 16305, listen: "127.0.0.1:16305", rules: "—", rules_missing: false, last: "12 分钟前" },
    Env { id: 4, name: "staging", health: Health::Running, port: 16307, listen: "127.0.0.1:16307", rules: "缺失", rules_missing: true, last: "刚刚" },
    Env { id: 5, name: "api-mock", health: Health::PortConflict, port: 16309, listen: "127.0.0.1:16309", rules: "1 条", rules_missing: false, last: "3 分钟前" },
    Env { id: 6, name: "legacy-x", health: Health::Failed, port: 16311, listen: "127.0.0.1:16311", rules: "1 条", rules_missing: false, last: "5 分钟前" },
    Env { id: 7, name: "scratch", health: Health::Stopped, port: 16313, listen: "127.0.0.1:16313", rules: "—", rules_missing: false, last: "—" },
];

pub fn env(id: usize) -> &'static Env {
    &ENVS[id.min(ENVS.len() - 1)]
}

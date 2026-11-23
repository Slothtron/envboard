//! 业务语义：环境是什么、状态有哪几种、每种状态在界面上长什么样。
//!
//! 这一层**不画东西**。它只做三件事：把 `EnvView.health` 翻成 `(Tone, Shape, 文案)`，
//! 把一组环境算成状态条上的计数，把 `EnvView` 收成界面用的 `Env`。
//! 颜色与像素在 `envboard_ui`，布局在 `pages`。
//!
//! M3 起数据来自 `AdminService::snapshot()`；`fixture()` 只在 golden 用
//! （`NG_GOLDEN=1`），把八种状态摆齐、逐像素基线才稳定。

use envboard_protocol::EnvView;
use nsh_core::prelude::*;

use envboard_ui::primitives::{self as ui, Shape, Tone};

// ---- 状态 ------------------------------------------------------------------

/// 界面上的六种状态（引擎的六种 `InstanceState` 一一对应）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Health {
    Running,
    Stopped,
    Starting,
    Unhealthy,
    PortConflict,
    Failed,
}

impl Health {
    /// 从 `EnvView.health` 的字面量来（`starting` / `running` / `stopped` /
    /// `unhealthy` / `port_conflict` / `failed`）；未知值按 `Failed` 处理，
    /// 宁可红一下，也不把不认识的状态画成绿。
    pub fn parse(literal: &str) -> Self {
        match literal {
            "running" => Health::Running,
            "stopped" => Health::Stopped,
            "starting" => Health::Starting,
            "unhealthy" => Health::Unhealthy,
            "port_conflict" => Health::PortConflict,
            _ => Health::Failed,
        }
    }

    /// 状态文案。`status_cell` 也用它，所以两处（列表与抽屉头）永远不会漂。
    pub fn label(self) -> &'static str {
        match self {
            Health::Running => "运行中",
            Health::Stopped => "已停止",
            Health::Starting => "启动中",
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
            Health::Starting => (Tone::Neutral, Shape::Ring),
            Health::Unhealthy => (Tone::Caution, Shape::DotBar),
            Health::PortConflict => (Tone::Critical, Shape::DotVBar),
            Health::Failed => (Tone::Critical, Shape::DotBang),
        }
    }

    /// 行尾动作的动词：在跑的给「停止」，停着的给「启动」，其余给「查看」。
    pub fn action(self) -> &'static str {
        match self {
            Health::Running => "停止",
            Health::Stopped => "启动",
            _ => "查看",
        }
    }
}

/// 列表列与抽屉头都用这一个入口，`width` 由调用方给（列表给列宽，抽屉头给自然宽）。
pub fn status_cell_for(health: Health, width: f32) -> impl IntoElement {
    let (tone, shape) = health.tone_shape();
    ui::status_cell(tone, shape, health.label(), width)
}

// ---- 环境 ------------------------------------------------------------------

/// 界面上的一行。字符串是**拥有**的：真数据每秒都可能换一批，不能靠 `'static`。
#[derive(Clone, PartialEq, Debug)]
pub struct Env {
    pub name: String,
    pub health: Health,
    /// 健康判定的原因（健康态为 `None`）。
    pub reason: Option<String>,
    pub port: u16,
    pub listen: String,
    /// 「3 条」/「缺失」/「—」，由 `rules_count` / `rules_missing` 算。
    pub rules: String,
    pub rules_missing: bool,
    pub upstream: Option<String>,
    pub description: String,
    /// 可直接粘进终端的一整行（`export https_proxy=…` 或只是一条 URL），来自 `EnvView`。
    pub proxy_command: String,
}

impl Env {
    pub fn from_view(view: EnvView) -> Self {
        let health = Health::parse(&view.health);
        let rules = if view.rules_missing {
            "缺失".to_owned()
        } else if view.rules.is_some() {
            format!("{} 条", view.rules_count)
        } else {
            "—".to_owned()
        };
        let listen = format!("{}:{}", view.listen.host, view.listen.port);
        Self {
            name: view.name,
            health,
            reason: view.health_reason,
            port: view.listen.port,
            listen,
            rules,
            rules_missing: view.rules_missing,
            upstream: view.upstream,
            description: view.description,
            proxy_command: view.proxy_command,
        }
    }

    /// 搜索的判据（名称、端口、规则、上游都算）。
    pub fn matches(&self, needle: &str) -> bool {
        if needle.is_empty() {
            return true;
        }
        let lower = needle.to_ascii_lowercase();
        self.name.to_ascii_lowercase().contains(&lower)
            || self.listen.contains(needle)
            || self.rules.contains(needle)
            || self.upstream.as_deref().is_some_and(|u| u.contains(needle))
    }
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

/// 按筛选片与搜索框过滤：判据与 `counts` 共用 `needs_attention`。
pub fn filtered(envs: &[Env], filter: usize, needle: &str) -> Vec<Env> {
    envs.iter()
        .filter(|e| match filter {
            1 => e.health == Health::Running,
            2 => e.health == Health::Stopped,
            3 => needs_attention(e),
            _ => true,
        })
        .filter(|e| e.matches(needle.trim()))
        .cloned()
        .collect()
}

// ---- fixture（只在 golden） -------------------------------------------------

/// 八个环境：允许的状态全部在场，外加一个“在跑但规则集缺失”的环境 ——
/// 状态条要报的两件事（不健康 / 规则缺失）因此可以同屏看。
pub fn fixture() -> Vec<Env> {
    fn env(name: &str, health: Health, port: u16, rules: &str, rules_missing: bool) -> Env {
        Env {
            name: name.to_owned(),
            health,
            reason: match health {
                Health::Unhealthy => Some("新配置尚未装配成功".to_owned()),
                Health::PortConflict => Some("端口被其它进程占用".to_owned()),
                Health::Failed => Some("实例线程已退出".to_owned()),
                _ => None,
            },
            port,
            listen: format!("127.0.0.1:{port}"),
            rules: rules.to_owned(),
            rules_missing,
            upstream: None,
            description: String::new(),
            proxy_command: format!("http://127.0.0.1:{port}"),
        }
    }
    vec![
        env("proxy-a", Health::Running, 16301, "3 条", false),
        env("proxy-b", Health::Running, 16302, "2 条", false),
        env("proxy-c", Health::Unhealthy, 16303, "2 条", false),
        env("dev-local", Health::Stopped, 16305, "—", false),
        env("staging", Health::Running, 16307, "缺失", true),
        env("api-mock", Health::PortConflict, 16309, "1 条", false),
        env("legacy-x", Health::Failed, 16311, "1 条", false),
        env("scratch", Health::Stopped, 16313, "—", false),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_split_running_stopped_and_attention() {
        let envs = fixture();
        let c = counts(&envs);
        assert_eq!(c.total, 8);
        assert_eq!(c.running, 3);
        assert_eq!(c.stopped, 2);
        assert_eq!(c.attention, 4); // unhealthy + port_conflict + failed + rules_missing
        assert_eq!(c.port_conflict, 1);
        assert_eq!(c.rules_missing, 1);
    }

    #[test]
    fn attention_line_says_none_when_clean() {
        let clean = vec![Env::from_view(EnvView {
            name: "a".into(),
            listen: envboard_protocol::Endpoint { host: "127.0.0.1".into(), port: 1 },
            rules: None,
            upstream: None,
            insecure_hosts: vec![],
            mitm_hosts: vec![],
            description: String::new(),
            desired: envboard_protocol::Desired::Stopped,
            health: "stopped".into(),
            health_reason: None,
            rules_count: 0,
            rules_missing: false,
            proxy_command: String::new(),
            proxy_auth_enabled: false,
            capture: false,
        })];
        assert_eq!(attention_line(&clean), "无待处理告警");
    }

    #[test]
    fn unknown_health_is_failed_not_running() {
        assert_eq!(Health::parse("who_knows"), Health::Failed);
        assert_eq!(Health::parse("running"), Health::Running);
    }

    #[test]
    fn search_matches_name_port_and_rules() {
        let envs = fixture();
        let by_port = filtered(&envs, 0, "16309");
        assert_eq!(by_port.len(), 1);
        assert_eq!(by_port[0].name, "api-mock");
        assert!(filtered(&envs, 1, "").iter().all(|e| e.health == Health::Running));
    }
}

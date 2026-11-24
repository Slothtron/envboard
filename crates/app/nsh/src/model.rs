//! 业务语义：环境是什么、状态有哪几种、每种状态在界面上长什么样。
//!
//! 这一层**不画东西**。它只做三件事：把 `EnvView.health` 翻成 `(Tone, Shape, 文案)`，
//! 把一组环境算成状态条上的计数，把 `EnvView` 收成界面用的 `Env`。
//! 颜色与像素在 `envboard_ui`，布局在 `pages`。
//!
//! M3 起数据来自 `AdminService::snapshot()`；`fixture()` 只在 golden 用
//! （`NG_GOLDEN=1`），把八种状态摆齐、逐像素基线才稳定。

pub mod body;
pub mod capture;

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

/// 虚拟列表用的随行版：状态由闭包现取，同一个格挪给另一行时重跑绑定即可。
pub fn status_cell_live_for<H>(health: H, width: f32) -> impl IntoElement
where
    H: Fn() -> Health + Copy + 'static,
{
    ui::status_cell_live(
        move || health().tone_shape().0,
        move || health().tone_shape().1,
        move || health().label().to_owned(),
        width,
    )
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

// ---- 证书（共享 CA）------------------------------------------------------
//
// 证书页要说的只有两件事：**系统信不信它**，以及**这张证书是什么**。
// 前者是三值探测（装了 / 没装 / 探不到），后者是只读摘要；两者都不含私钥，
// 界面也没有任何“重新签发”的入口（轮换是命令行的事，M4 不做）。

/// 系统信任库里这张 CA 的所在（引擎 `StorePresence` 的界面映射）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Trust {
    Installed,
    Missing,
    /// 探测本身失败（读不到系统存储）：既不敢说装了，也不敢说没装。
    Unknown,
}

impl Trust {
    /// 展示用的一行字（证书页「计算机存储」那一行）。
    pub fn label(self) -> &'static str {
        match self {
            Trust::Installed => "已安装",
            Trust::Missing => "未安装",
            Trust::Unknown => "无法确认",
        }
    }
}

/// 证书页的一屏数据。字段全是**可选**：摘要解析不出来就显示「—」，不编内容。
#[derive(Clone, PartialEq, Debug)]
pub struct Ca {
    /// 当前用户 ROOT 存储。
    pub user: Trust,
    /// 计算机（LocalMachine）存储：本版本只探测、不代删。
    pub machine: Trust,
    /// 探测给出的解释（例如“证书文件缺失”），没有就不写。
    pub detail: Option<String>,
    pub common_name: Option<String>,
    pub organization: Option<String>,
    pub serial_hex: Option<String>,
    pub not_before: Option<String>,
    pub not_after: Option<String>,
    pub sig_alg: Option<String>,
    /// 「EC · prime256v1」这类一行话。
    pub pubkey: Option<String>,
    pub fingerprint: String,
    pub cert_path: String,
}

impl Ca {
    /// 当前用户在信任库里 = 界面不再提醒。
    pub fn trusted(&self) -> bool {
        self.user == Trust::Installed
    }

    /// 顶部那条提示的语气：信了是绿的，其余一律警。
    pub fn tone(&self) -> Tone {
        if self.trusted() {
            Tone::Success
        } else {
            Tone::Caution
        }
    }

    /// 顶部提示的（标题, 说明）。四句话覆盖四种真实情况，不写“一切正常”式占位。
    pub fn headline(&self) -> (&'static str, &'static str) {
        match (self.user, self.machine) {
            (Trust::Installed, _) => (
                "根证书已就绪且已受信任",
                "被代理的 HTTPS 流量可以正常解密与改写。",
            ),
            (Trust::Missing, Trust::Installed) => (
                "计算机存储里有这张证书，当前用户还没有",
                "当前用户启动的程序仍会报不受信任：点「安装证书」写入当前用户存储（免管理员）。",
            ),
            (Trust::Missing, _) => (
                "根证书尚未导入系统信任库",
                "本机客户端会报证书不受信任：点「安装证书」直接写入当前用户信任库（免管理员）。",
            ),
            (Trust::Unknown, _) => (
                "无法确认系统信任库状态",
                "探测失败：点「重新读取」再试一次；「安装证书」是幂等的，也可以直接点。",
            ),
        }
    }

    /// 「主体」一行：证书里有什么就说什么，都没有给组织名，再没有给「—」。
    pub fn subject(&self) -> String {
        self.common_name
            .clone()
            .or_else(|| self.organization.clone())
            .unwrap_or_else(|| "—".to_owned())
    }

    /// 按钮可用性：已安装就别再“安装”，没装就别“卸载”（判断只有这一处）。
    pub fn can_install(&self) -> bool {
        self.user != Trust::Installed
    }

    pub fn can_uninstall(&self) -> bool {
        self.user == Trust::Installed
    }
}

/// 卸载证书的结果。界面文案的分支依据只有一个：**到底删掉了没有**，
/// 以及有没有“计算机存储里还剩一份”这句必须说出来的实话。
#[derive(Clone, PartialEq, Debug)]
pub struct Uninstall {
    pub removed: bool,
    /// 计算机存储里仍有这张证书时的说明（本版本不代删，只报告）。
    pub warning: Option<String>,
}

/// golden 与截图口径用的固定样本（`NG_GOLDEN=1` / `NG_PLAIN=1` 没有宿主）。
/// 值全是**样张**，不是任何一台机器上的真证书；与 `fixture()` 同一用途。
pub fn fixture_ca() -> Ca {
    Ca {
        user: Trust::Missing,
        machine: Trust::Missing,
        detail: None,
        common_name: Some("envboard local CA".to_owned()),
        organization: Some("envboard".to_owned()),
        serial_hex: Some("7A1F440C9E2233B8".to_owned()),
        not_before: Some("2026-11-23 08:00:00Z".to_owned()),
        not_after: Some("2036-11-23 08:00:00Z".to_owned()),
        sig_alg: Some("ECDSA with SHA-256".to_owned()),
        pubkey: Some("EC · P-256".to_owned()),
        fingerprint: "3F:9C:71:2D:88:04:BE:57:C1:6A:0D:9F:33:10:5E:A2".to_owned(),
        cert_path: r"C:\Users\<user>\AppData\Local\envboard\ca\envboard-ca-cert.pem".to_owned(),
    }
}

// ---- fixture（golden 与压力口径） -----------------------------------------

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

/// 八个环境：允许的状态全部在场，外加一个“在跑但规则集缺失”的环境 ——
/// 状态条要报的两件事（不健康 / 规则缺失）因此可以同屏看。
/// 8 条的固定账本（golden 与单测的口径）；界面走 [`fixture_n`]。
#[allow(dead_code)]
pub fn fixture() -> Vec<Env> {
    fixture_n(8)
}

/// 压力口径（G3）：前 8 条就是 `fixture()` 那八条（golden 因此不动），
/// 之后按 `bulk-00000…` 顺序补到 `n` 条，四种状态轮转。
///
/// 为什么用 fixture 而不是往真账本灌 10k：G3 量的是**列表渲染**（`each` 建行、
/// 行高、滚动），与账本无关；灌 10k 真环境要上万次 create + 落库，测出来的
/// 是管理器的写吞吐，不是界面。真数据的读写正确性已由 `tests/data_path.rs` 覆盖。
pub fn fixture_n(n: usize) -> Vec<Env> {
    let mut envs = vec![
        env("proxy-a", Health::Running, 16301, "3 条", false),
        env("proxy-b", Health::Running, 16302, "2 条", false),
        env("proxy-c", Health::Unhealthy, 16303, "2 条", false),
        env("dev-local", Health::Stopped, 16305, "—", false),
        env("staging", Health::Running, 16307, "缺失", true),
        env("api-mock", Health::PortConflict, 16309, "1 条", false),
        env("legacy-x", Health::Failed, 16311, "1 条", false),
        env("scratch", Health::Stopped, 16313, "—", false),
    ];
    while envs.len() < n {
        let i = envs.len() - 8;
        let port = 17000u16 + (i % 4000) as u16;
        let health = match i % 4 {
            0 => Health::Running,
            1 => Health::Unhealthy,
            2 => Health::Stopped,
            _ => Health::PortConflict,
        };
        envs.push(env(&format!("bulk-{i:05}"), health, port, "2 条", false));
    }
    envs
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
            listen: envboard_protocol::Endpoint {
                host: "127.0.0.1".into(),
                port: 1,
            },
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
        assert!(
            filtered(&envs, 1, "")
                .iter()
                .all(|e| e.health == Health::Running)
        );
    }
}

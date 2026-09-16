//! proxy core 抽象 —— 编排层只认识这里的类型，不认识 mitmproxy。
//!
//! 契约见 `core/spec/capabilities.md` 的「ProxyCore 能力矩阵」与
//! core 抽象：编排层只认识它，不认识具体代理实现。设计要点：
//!
//! * 接口粒度收在**启动参数 + 状态回传**——再细就会泄漏某个 core 的形状；
//! * [`InstanceSpec::options`] 是**唯一**允许 core 特有配置进入实例的通道，
//!   但它有 denylist（见 [`validate_options`]），否则逃生门会从另一头漏掉抽象；
//! * 实例**不知道自己是哪个环境**——环境名只在注解与状态文件里作为展示数据出现。

use std::collections::BTreeMap;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Error;

/// 默认监听地址：只在 core 定义一次，适配器禁止再来一份。
pub const DEFAULT_LISTEN_HOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// 环境的监听地址 —— 环境的**外部身份**（一个环境 = 一个实例 = 一个端口）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Listen {
    pub host: IpAddr,
    pub port: u16,
}

impl Listen {
    pub fn new(host: IpAddr, port: u16) -> Self {
        Self { host, port }
    }

    pub fn localhost(port: u16) -> Self {
        Self {
            host: DEFAULT_LISTEN_HOST,
            port,
        }
    }
}

impl fmt::Display for Listen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.host {
            // IPv6 要加方括号才是合法的 host:port 写法
            IpAddr::V6(addr) => write!(f, "[{addr}]:{}", self.port),
            IpAddr::V4(addr) => write!(f, "{addr}:{}", self.port),
        }
    }
}

/// 进程身份 —— 孤儿清理的判据。
///
/// **三者都要比**：只看 PID 存活会误杀 PID 复用后的无关进程；只看 PID + cmdline
/// 仍然不够，因为同一份配置的实例 cmdline **完全相同**，所以必须带上启动时刻。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: i32,
    /// 进程启动时刻（`/proc/<pid>/stat` 的 starttime，或等价单调量）。
    pub starttime: u64,
    pub cmdline: Vec<String>,
}

impl ProcessIdentity {
    pub fn same_process(&self, other: &Self) -> bool {
        self.pid == other.pid && self.starttime == other.starttime && self.cmdline == other.cmdline
    }

    /// 是不是"同一个进程但内容变了"（PID + 启动时刻相同，cmdline 不同）。
    pub fn same_process_different_cmdline(&self, other: &Self) -> bool {
        self.pid == other.pid && self.starttime == other.starttime && self.cmdline != other.cmdline
    }
}

/// 启动一个实例所需的全部输入。
#[derive(Debug, Clone)]
pub struct InstanceSpec {
    pub env: String,
    pub listen: Listen,
    /// 绑定的规则文件（已解析成绝对路径）。`None` = 不覆盖任何域名。
    pub rules: Option<PathBuf>,
    /// 状态文件路径 —— **由管理器指定绝对路径**，注入器不拼环境名。
    pub status_file: PathBuf,
    /// 跨实例共享物（如共享 CA）所在目录。
    pub shared_state_dir: PathBuf,
    /// 本实例的运行时目录（状态文件、PID 记录）。
    pub runtime_dir: PathBuf,
    /// 实例输出的**落盘目录**（默认由管理器指到 `<state_dir>/logs`）。
    ///
    /// `Some(dir)`：让子进程的 stdout/stderr **直接写 `dir/<env>.log`** —— 没有管道、
    /// 没有读线程，我们进程不在日志链路上。这样"没人读管道 → 管道写满 → 子进程阻塞在
    /// 写日志上 → 整个代理挂住"这条路径从设计上不存在（实测：默认详细度下 213 个请求
    /// 就写满 64 KiB 管道，之后所有客户端一起挂）。
    ///
    /// `None`：不落盘，只保留有界的内存环形缓冲（`--no-log-file`）。这条路径下
    /// **必须**持续把管道读走，否则会退化成上面那种卡死。
    ///
    /// 落在 `dir` 里的文件由管理器读尾部并轮转（`envboard-manager/src/logs.rs`），
    /// core 不重复实现一份文件读取。
    pub log_dir: Option<PathBuf>,
    /// core 特有选项，处理器不理解也不解释，只做 denylist 后原样透传。
    pub options: BTreeMap<String, String>,
}

impl InstanceSpec {
    pub fn validate(&self) -> Result<(), Error> {
        validate_options(&self.options)
    }
}

/// 管理器自有键：这些值由启动契约下发，用户透过 `options` 覆盖会让
/// "实例的形态"出现两个真相。
pub const RESERVED_OPTION_KEYS: &[&str] = &[
    "listen_host",
    "listen_port",
    "confdir",
    "status_file",
    "rules",
];

/// 会改变进程拓扑或加载第三方代码的键：它们不是"core 特有配置"，
/// 而是"把 core 换成别的东西"（`scripts` 能再挂一个 addon 进来，绕过启动契约）。
pub const TOPOLOGY_OPTION_KEYS: &[&str] = &["mode", "upstream", "scripts"];

/// 同上，但按前缀匹配（某个 core 的整族选项）。
///
/// `web` 覆盖 mitmweb 的全部选项（`web_port` / `web_host` / `web_open_browser` …）：
/// 它们只对 mitmweb 生效，配到 headless 实例上会被**静默忽略**，
/// 与其让用户以为生效了，不如直接拒绝。
pub const TOPOLOGY_OPTION_PREFIXES: &[&str] = &["web"];

/// 环境变量式的保留前缀：`envboard_*` 是管理器与注入器之间的约定，不允许从
/// `options` 覆盖。
pub const RESERVED_OPTION_PREFIX: &str = "envboard_";

/// `InstanceSpec.options` 的 denylist 校验（契约见 `core/spec/capabilities.md`）。
///
/// 只列"绝不允许"，其余键原样透传；某个 core 实现可以再收紧（例如 mitmproxy
/// 实现额外禁掉自己特有的危险选项），但**不能放宽**这张表。
pub fn validate_options(options: &BTreeMap<String, String>) -> Result<(), Error> {
    for key in options.keys() {
        let field = format!("instance.options.{key}");
        if key.starts_with(RESERVED_OPTION_PREFIX) {
            return Err(Error::invalid_config(
                field,
                format!("option {key:?} is reserved for the manager/injector contract"),
            ));
        }
        if RESERVED_OPTION_KEYS.contains(&key.as_str())
            || TOPOLOGY_OPTION_KEYS.contains(&key.as_str())
            || TOPOLOGY_OPTION_PREFIXES
                .iter()
                .any(|prefix| key.starts_with(prefix))
        {
            return Err(Error::invalid_config(
                field,
                format!("option {key:?} is owned by the manager and cannot be overridden"),
            ));
        }
    }
    Ok(())
}

/// 一个已启动（或正在启动）的实例。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceHandle {
    pub env: String,
    pub listen: Listen,
    /// 受管外部进程的身份；进程内实现（如 FakeCore）为 `None`。
    pub process: Option<ProcessIdentity>,
}

/// 实例健康状态 —— 与"期望状态"解耦，判定次序见 `core/spec/errors.md`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceHealth {
    Running,
    Stopped,
    /// 进程活着，但状态文件缺失/过期，或探活失败。
    Unhealthy {
        reason: String,
    },
    /// 起来了，但生效配置与期望不一致（宿主静默忽略了某个 `--set`）。
    ConfigMismatch {
        reason: String,
    },
    /// 端口被别的程序占走，起不来。**不换端口**，等一次显式动作。
    PortConflict {
        port: u16,
    },
    Failed {
        reason: String,
    },
}

impl InstanceHealth {
    pub fn is_running(&self) -> bool {
        matches!(self, InstanceHealth::Running)
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            InstanceHealth::Unhealthy { reason }
            | InstanceHealth::ConfigMismatch { reason }
            | InstanceHealth::Failed { reason } => Some(reason),
            _ => None,
        }
    }

    /// 契约里的健康状态字面量（`core/spec/errors.md` 的健康状态表）。
    pub fn as_str(&self) -> &'static str {
        match self {
            InstanceHealth::Running => "running",
            InstanceHealth::Stopped => "stopped",
            InstanceHealth::Unhealthy { .. } => "unhealthy",
            InstanceHealth::ConfigMismatch { .. } => "config_mismatch",
            InstanceHealth::PortConflict { .. } => "port_conflict",
            InstanceHealth::Failed { .. } => "failed",
        }
    }
}

/// core 的名字与版本，用于展示与兼容性判断。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreInfo {
    pub name: String,
    pub version: String,
}

/// 这个 core 支持哪些能力（能力矩阵见 `core/spec/capabilities.md`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CoreCapabilities {
    pub listen: bool,
    pub dynamic_certs: bool,
    pub rewrite_upstream: bool,
    pub per_instance_options: bool,
    pub shared_ca: bool,
    pub flow_annotation: bool,
    /// 状态文件里是否如实回显 `rules_count`。
    ///
    /// 决定管理器能不能做"规则条数"这一项比对：做不到的 core（如 FakeCore）
    /// 必须显式声明 `false`，否则管理器会去比一个永远为 0 的字段而误报
    /// [`crate::error::ErrorCode::ConfigMismatch`]。
    pub reports_rules_count: bool,
    /// 实例是不是**独立的外部进程**。
    ///
    /// 管理器据此决定"孤儿清理"要不要读 `/proc`：进程内的 core（FakeCore、
    /// 将来的嵌入式核心）没有可清理的进程，硬去比对 PID 只会制造误报。
    /// 这条是能力声明，不是 `if core == "..."` 判断 —— 后者是抽象漏了的信号。
    pub external_processes: bool,
}

/// 实例回传的状态——**判定以它为主、TCP 探活为辅**。
///
/// 这是管理器与注入器之间的线上契约：字段改名即 breaking。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusReport {
    /// 仅展示：环境名不参与实例自身的逻辑。
    pub env_name: String,
    pub pid: i32,
    pub listen: Listen,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules_path: Option<String>,
    #[serde(default)]
    pub rules_count: usize,
    #[serde(default)]
    pub reload_interval_secs: u64,
    pub core_version: String,
    pub agent_version: String,
    /// Unix 秒。管理器靠它判断"新鲜度"。
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// **契约回显**：注入器对"管理器下发的每个选项"逐项核对的结论。
    ///
    /// 键是选项名，值是 `"ok"` 或一段说明（未注册 / 取值不同）。为什么需要它：
    /// mitmproxy 对**未知或拼错的 `--set` 是静默忽略**的（实测），
    /// 所以"命令没报错"不能当作配置生效 —— 注入器是唯一能回答"这些键到底有没有
    /// 被宿主接受"的地方，它把答案写在这里，管理器再比对。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options_echo: BTreeMap<String, String>,
}

impl StatusReport {
    /// `options_echo` 里有没有非 `"ok"` 的项 —— 有就是配置没落地。
    pub fn options_mismatch(&self) -> Option<String> {
        let bad: Vec<String> = self
            .options_echo
            .iter()
            .filter(|(_, verdict)| verdict.as_str() != "ok")
            .map(|(key, verdict)| format!("{key}: {verdict}"))
            .collect();
        if bad.is_empty() {
            None
        } else {
            Some(bad.join("; "))
        }
    }

    pub fn from_json_slice(raw: &[u8]) -> Result<Self, Error> {
        serde_json::from_slice(raw).map_err(|error| {
            Error::internal_error(format!("status file is not a valid StatusReport: {error}"))
        })
    }

    pub fn to_json_vec(&self) -> Result<Vec<u8>, Error> {
        serde_json::to_vec_pretty(self).map_err(Error::from)
    }
}

/// 一个可被管理器编排的代理核心。
///
/// 抽象只为"将来能换核心"而存在，但抽取本身就值一次架构校验：
/// 若管理器里出现了 `if core == "mitmproxy"`，说明抽象漏了。
#[async_trait]
pub trait ProxyCore: Send + Sync {
    fn describe(&self) -> CoreInfo;

    fn capabilities(&self) -> CoreCapabilities;

    /// 启动一个实例。实现方负责校验 [`InstanceSpec::options`]（至少跑
    /// [`validate_options`]），并在失败时返回**响亮**的错误。
    async fn start(&self, spec: InstanceSpec) -> Result<InstanceHandle, Error>;

    /// 停止一个实例：先优雅，超时再强杀。
    async fn stop(&self, handle: &InstanceHandle) -> Result<(), Error>;

    /// 实例是否还活着（生死判据，细节由状态文件补）。
    async fn probe(&self, handle: &InstanceHandle) -> InstanceHealth;

    /// 实例输出的尾部若干行（工作台 `GET /api/environments/:name/logs` 用它）。
    ///
    /// 默认返回空：进程内的 core（FakeCore）没有子进程输出可给。
    /// 由"拥有那个进程"的实现提供，因此读的是它自己的缓冲，而不是猜某个文件路径。
    fn logs_tail(&self, _handle: &InstanceHandle, _lines: usize) -> Vec<String> {
        Vec::new()
    }

    /// 进程**已经自己退出**时，说明它为什么退出（同步）。
    ///
    /// 存在的理由：视图层（列表/详情）是同步的、不能 `await` core，但又必须给出准确
    /// 结论 —— "进程没了"与"我让它停的"是两件事。返回 `None` 表示没有这样的记录
    /// （实例没起过、还活着，或者是被显式停掉的）。
    ///
    /// 默认 `None`：进程内 core（FakeCore）的实例不会自己消失。
    fn last_exit(&self, _handle: &InstanceHandle) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn options_escape_hatch_allows_core_specific_keys() {
        // ssl_insecure 是"core 特有配置"的典型：管理器不理解它，原样透传。
        let allowed = options(&[("ssl_insecure", "true")]);
        assert!(validate_options(&allowed).is_ok());
    }

    #[test]
    fn options_denylist_blocks_manager_owned_keys() {
        for key in [
            "listen_port",
            "listen_host",
            "confdir",
            "rules",
            "status_file",
        ] {
            let denied = options(&[(key, "x")]);
            let error = validate_options(&denied).unwrap_err();
            assert_eq!(error.code, crate::error::ErrorCode::InvalidConfig);
            assert_eq!(
                error.field.as_deref(),
                Some(format!("instance.options.{key}").as_str())
            );
        }
    }

    #[test]
    fn options_denylist_blocks_topology_and_envboard_prefix() {
        for key in ["mode", "upstream", "scripts", "web_open_browser"] {
            assert!(
                validate_options(&options(&[(key, "x")])).is_err(),
                "{key} must be denied"
            );
        }
        // 前缀保留：注入器/管理器的约定不允许被 options 覆盖
        assert!(validate_options(&options(&[("envboard_rules", "/tmp/x")])).is_err());
    }

    #[test]
    fn process_identity_separates_pid_reuse() {
        let recorded = ProcessIdentity {
            pid: 111,
            starttime: 900,
            cmdline: vec!["mitmdump".into()],
        };
        let reused = ProcessIdentity {
            pid: 111,
            starttime: 1200,
            cmdline: vec!["mitmdump".into()],
        };
        assert!(!recorded.same_process(&reused));
        assert!(!recorded.same_process_different_cmdline(&reused));
    }

    #[test]
    fn status_report_round_trips() {
        let report = StatusReport {
            env_name: "beta".into(),
            pid: 42,
            listen: Listen::localhost(16301),
            rules_path: Some("/tmp/beta.rules".into()),
            rules_count: 3,
            reload_interval_secs: 5,
            core_version: "12.2.3".into(),
            agent_version: "0.1.0".into(),
            updated_at: 1_700_000_000,
            last_error: None,
            options_echo: BTreeMap::from([("ssl_insecure".to_string(), "ok".to_string())]),
        };
        let raw = report.to_json_vec().unwrap();
        assert_eq!(StatusReport::from_json_slice(&raw).unwrap(), report);
    }
}

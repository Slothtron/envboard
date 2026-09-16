//! proxy core 抽象 —— 编排层只认识这里的类型，不认识 mitmproxy。
//!
//! 契约见 `core/spec/capabilities.md` 的「ProxyCore 能力矩阵」与
//! core 抽象：编排层只认识它，不认识具体代理实现。设计要点：
//!
//! * 接口粒度收在**启动参数 + 状态回传**——再细就会泄漏某个 core 的形状；
//! * 实例的配置**只有一等字段**这一条路（监听地址、注入器目录、放行域名、代理凭据）；
//!   曾经存在过一个任意 `options` 透传通道，它让"放宽上游证书校验"这种安全控制
//!   可以绕过契约，已删除；
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

/// 每个环境目录里注入器读的配置文件名（管理器唯一写者）。
pub const CONFIG_FILE_NAME: &str = "config.json";

/// 每个环境目录里规则文件的**固定名**软链。
///
/// 环境绑定哪份规则由这条链指向谁决定，`config.json` 里的 `rules` 只用来做
/// **链接完整性校验**（链指向的名字与配置不一致 = 响亮报错，不猜用哪一个）。
/// 这样"换绑定"就是换一条链，注入器不必知道名字、也不必重启。
pub const RULES_LINK_NAME: &str = "envboard.rules";

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
///
/// cmdline 里可能带凭据（`--set proxyauth=user:password`），所以它**记录前必须脱敏**
/// （见 [`redact_cmdline`]）：账本是长期留存的产物，没有理由把凭据复制进去。
/// 比对时两侧都脱敏，所以"启用鉴权的实例"仍然能被正确认出来。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: i32,
    /// 进程启动时刻（`/proc/<pid>/stat` 的 starttime，或等价单调量）。
    pub starttime: u64,
    pub cmdline: Vec<String>,
}

impl ProcessIdentity {
    pub fn same_process(&self, other: &Self) -> bool {
        self.pid == other.pid
            && self.starttime == other.starttime
            && redact_cmdline(&self.cmdline) == redact_cmdline(&other.cmdline)
    }

    /// 是不是"同一个进程但内容变了"（PID + 启动时刻相同，cmdline 不同）。
    pub fn same_process_different_cmdline(&self, other: &Self) -> bool {
        self.pid == other.pid
            && self.starttime == other.starttime
            && redact_cmdline(&self.cmdline) != redact_cmdline(&other.cmdline)
    }

    /// 记录前调用：把凭据形状的参数值换成 `***`。
    pub fn redacted(mut self) -> Self {
        self.cmdline = redact_cmdline(&self.cmdline);
        self
    }
}

/// 把 `--set proxyauth=<凭据>` 的值替换成 `***`。
///
/// 认两种形态：`["--set", "proxyauth=…"]` 与 `["--set=proxyauth=…"]`。
/// 其余参数原样保留 —— 脱敏只针对**凭据键**，改别的参数会让身份判定失去意义。
pub fn redact_cmdline(args: &[String]) -> Vec<String> {
    args.iter().map(|arg| redact_arg(arg)).collect()
}

fn redact_arg(arg: &str) -> String {
    if arg.starts_with("proxyauth=") {
        return "proxyauth=***".to_string();
    }
    if let Some(rest) = arg.strip_prefix("--set=")
        && rest.starts_with("proxyauth=")
    {
        return "--set=proxyauth=***".to_string();
    }
    arg.to_string()
}

/// 启动一个实例所需的全部输入。
#[derive(Debug, Clone)]
pub struct InstanceSpec {
    pub env: String,
    pub listen: Listen,
    /// 本实例的 agent 目录（`<state_dir>/agent/<env>`）。
    ///
    /// 注入器、[`CONFIG_FILE_NAME`] 与 [`RULES_LINK_NAME`] 都在这里。规则路径**不在**
    /// 启动参数里：注入器只认自己目录旁边的固定名，管理器负责把那条链指对 —— 于是
    /// "换绑定"变成一次原子换链，运行中的实例下一次轮询就能跟上。
    pub agent_dir: PathBuf,
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
    /// 按域名放宽上游证书校验的完整域名清单（已归一化 + 去重 + 排序）。
    ///
    /// 空 = 全部严格校验。放宽**只**能经这条一等字段，没有全局开关。
    pub insecure_hosts: Vec<String>,
    /// 代理访问鉴权用户名；`None` = 不启用。与 [`InstanceSpec::proxy_password`] 同生共死。
    pub proxy_user: Option<String>,
    /// 代理访问鉴权密码。凭据的唯一来源是环境字段，且只经实例启动参数下发。
    pub proxy_password: Option<String>,
}

impl InstanceSpec {
    /// 本实例的配置文件路径。
    pub fn config_file(&self) -> PathBuf {
        self.agent_dir.join(CONFIG_FILE_NAME)
    }

    /// 本实例的规则软链路径（**固定名**）。
    pub fn rules_link(&self) -> PathBuf {
        self.agent_dir.join(RULES_LINK_NAME)
    }

    /// `proxyauth` 的期望取值（`user:password`）；未启用鉴权时为 `None`。
    ///
    /// 这是**唯一**把两个字段拼回 mitmproxy 形状的地方 —— 拼装只发生在边界上。
    pub fn proxyauth(&self) -> Option<String> {
        match (&self.proxy_user, &self.proxy_password) {
            (Some(user), Some(password)) => Some(format!("{user}:{password}")),
            _ => None,
        }
    }

    /// 实例启动时由管理器下发、且**必须**被宿主接受的 `--set` 键值。
    ///
    /// 用途是让注入器回显自检（`StatusReport.options_echo`）：mitmproxy 对未知或拼错的
    /// `--set` 是静默忽略的，而 `proxyauth` 是安全控制 —— 被忽略就等于代理在"以为开了
    /// 鉴权"的状态下裸奔。契约形状由本方法一处给出，写配置的一方与拼参数的一方都用它，
    /// 不会出现两个真相。
    pub fn launch_expected(&self) -> BTreeMap<String, String> {
        let mut expected = BTreeMap::new();
        if let Some(proxyauth) = self.proxyauth() {
            expected.insert("proxyauth".to_string(), proxyauth);
        }
        expected
    }
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
    /// 是否支持**按域名放宽上游证书校验**（`insecure_hosts`）。
    ///
    /// 不支持时必须显式声明 `false`：管理器据此决定要不要把这份配置算进"生效回显"，
    /// 而不是去比一个永远为 0 的字段。
    pub per_domain_insecure: bool,
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
    /// 生效配置的条目数（放行域名）。
    #[serde(default)]
    pub insecure_hosts_count: usize,
    /// 实例当前生效的那份 `config.json` 的 sha256。
    ///
    /// 管理器拿它与自己写下的字节对比：相等 = 已生效；不等但在收敛窗口内 = 还在收敛；
    /// 超窗仍不等 = `config_mismatch`。**空串 = 上一代实例**（那份实现还没有这个字段），
    /// 说明它跑的根本不是这套通道，reconcile 会把它重启一次。
    #[serde(default)]
    pub config_hash: String,
    /// 最近一次热重载失败的原因（校验不过时保留旧快照）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_error: Option<String>,
    #[serde(default)]
    pub reload_interval_secs: u64,
    pub core_version: String,
    pub agent_version: String,
    /// Unix 秒。管理器靠它判断"新鲜度"。
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// **契约回显**：注入器对"管理器下发的每个 `--set`"逐项核对的结论。
    ///
    /// 键是选项名，值是 `"ok"` 或一段说明（未注册 / 取值不同）。为什么需要它：
    /// mitmproxy 对**未知或拼错的 `--set` 是静默忽略**的（实测），
    /// 所以"命令没报错"不能当作配置生效 —— 注入器是唯一能回答"这些键到底有没有
    /// 被宿主接受"的地方，它把答案写在这里，管理器再比对。
    ///
    /// 现在只剩管理器自己下发的键（`proxyauth`）：它是安全控制，
    /// 被静默忽略会让代理在"以为开了鉴权"的状态下裸奔。
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

    /// 这个实例是不是上一代二进制拉起来的（没有配置哈希回执）。
    pub fn is_legacy(&self) -> bool {
        self.config_hash.is_empty()
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

    /// 启动一个实例。实现方负责在失败时返回**响亮**的错误。
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

    fn spec(port: u16) -> InstanceSpec {
        InstanceSpec {
            env: "beta".into(),
            listen: Listen::localhost(port),
            agent_dir: std::env::temp_dir().join("envboard-agent-beta"),
            status_file: std::env::temp_dir().join("beta.status.json"),
            shared_state_dir: std::env::temp_dir(),
            runtime_dir: std::env::temp_dir(),
            log_dir: None,
            insecure_hosts: Vec::new(),
            proxy_user: None,
            proxy_password: None,
        }
    }

    #[test]
    fn proxyauth_is_assembled_only_at_the_boundary() {
        let mut spec = spec(16_301);
        assert_eq!(spec.proxyauth(), None);
        spec.proxy_user = Some("alice".into());
        spec.proxy_password = Some("s3cret".into());
        assert_eq!(spec.proxyauth().as_deref(), Some("alice:s3cret"));
    }

    #[test]
    fn instance_paths_are_fixed_names_beside_the_injector() {
        let spec = spec(16_301);
        assert_eq!(
            spec.config_file(),
            std::env::temp_dir().join("envboard-agent-beta/config.json")
        );
        assert_eq!(
            spec.rules_link(),
            std::env::temp_dir().join("envboard-agent-beta/envboard.rules")
        );
    }

    #[test]
    fn credentials_are_redacted_before_they_reach_a_record() {
        let identity = ProcessIdentity {
            pid: 111,
            starttime: 900,
            cmdline: vec![
                "mitmdump".into(),
                "--set".into(),
                "proxyauth=alice:s3cret".into(),
                "--set".into(),
                "listen_port=16301".into(),
            ],
        }
        .redacted();
        assert_eq!(identity.cmdline[2], "proxyauth=***");
        assert_eq!(identity.cmdline[4], "listen_port=16301");
        assert!(!identity.cmdline.iter().any(|arg| arg.contains("s3cret")));

        // `--set=key=value` 形态同样要脱敏
        let inline = redact_cmdline(&["--set=proxyauth=alice:s3cret".to_string()]);
        assert_eq!(inline, vec!["--set=proxyauth=***".to_string()]);
    }

    #[test]
    fn identity_comparison_ignores_the_rotated_credential_value() {
        let recorded = ProcessIdentity {
            pid: 111,
            starttime: 900,
            cmdline: vec!["--set".into(), "proxyauth=old:pw".into()],
        };
        let live = ProcessIdentity {
            pid: 111,
            starttime: 900,
            cmdline: vec!["--set".into(), "proxyauth=new:pw".into()],
        };
        // 轮换密码不该让"这是同一个进程"变成假
        assert!(recorded.same_process(&live));
        assert!(!recorded.same_process_different_cmdline(&live));
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
            rules_path: Some("/tmp/agent/beta/envboard.rules".into()),
            rules_count: 3,
            insecure_hosts_count: 1,
            config_hash: "abc123".into(),
            config_error: None,
            reload_interval_secs: 5,
            core_version: "12.2.3".into(),
            agent_version: "0.1.0".into(),
            updated_at: 1_700_000_000,
            last_error: None,
            options_echo: BTreeMap::from([("proxyauth".to_string(), "ok".to_string())]),
        };
        let raw = report.to_json_vec().unwrap();
        assert_eq!(StatusReport::from_json_slice(&raw).unwrap(), report);
        assert!(!report.is_legacy());
    }

    #[test]
    fn a_status_file_without_a_config_hash_is_from_the_previous_generation() {
        // 上一代注入器写的状态文件没有 config_hash / insecure_hosts_count 两个键。
        let raw = br#"{
            "env_name": "beta", "pid": 1,
            "listen": {"host": "127.0.0.1", "port": 16301},
            "rules_count": 2, "reload_interval_secs": 5,
            "core_version": "12.2.3", "agent_version": "0.0.9", "updated_at": 1700000000
        }"#;
        let report = StatusReport::from_json_slice(raw).unwrap();
        assert!(report.is_legacy());
        assert_eq!(report.insecure_hosts_count, 0);
    }
}

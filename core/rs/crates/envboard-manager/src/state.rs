//! 状态持久化 + 单实例锁 + 管理器自身配置。
//!
//! 三条契约要求在这里落地：
//! 1. **状态只有一个写者**：管理器对 `<state_dir>/lock` 取 flock；
//! 2. **原子写 + 0600**：先写临时文件再 rename，权限收紧；
//! 3. **唯一归一化配置**：默认值只在这里定义一次，CLI 只做映射。

use std::collections::BTreeMap;
use std::fmt;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use envboard_core_api::{DEFAULT_LISTEN_HOST, Error, ErrorCode};
use envboard_domain::{DEFAULT_MAX_ATTEMPTS, DEFAULT_PORT_RANGE, Desired};
use serde::{Deserialize, Serialize};

/// 状态文件格式版本 —— 结构变化时递增，加载时拒绝不认识的高版本。
pub const STATE_VERSION: u32 = 1;

/// 单个环境日志文件的默认轮转上限。
///
/// 默认详细度约 307 字节/请求，8 MiB 约合 2.7 万个请求 —— 够放下崩溃现场，
/// 又不至于"总是落盘 + 不轮转"把磁盘写满。
pub const DEFAULT_MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;

/// 轮转上限的下限：64 KiB（约 210 个请求的日志），再小就没有排障价值了。
pub const MIN_MAX_LOG_BYTES: u64 = 64 * 1024;

/// 状态文件里一份环境的形状（就是 `Environment::to_json` 的结果）。
pub type EnvironmentJson = serde_json::Value;

/// 状态文件里记下的一条"标记"（不是健康状态本身，而是需要跨重启保留的异常）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredError {
    pub code: ErrorCode,
    pub message: String,
}

/// 规则库账本里的一条记录。
///
/// **账本是唯一真相**：`<rules_dir>/<name>.rules` 只是它的一份**可再生物化产物**
/// （文件被删/被改坏都能按 `rendered` 逐字节重建）。把规则正文一起存进来，是为了
/// 备份/恢复与"自愈"能够成立；代价是状态文件会变大，用体积换确定性是这里的取舍。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredRules {
    /// 规则名（白名单 `^[a-z][a-z0-9_-]{0,31}$`，与环境 `rules` 字段同源校验）。
    pub name: String,
    /// 导入时给的来源标签（仅展示，可为空）。
    #[serde(default)]
    pub source: Option<String>,
    /// **规范化渲染后的完整文本**（与物化文件的字节一致）。
    pub rendered: String,
    /// 被接受的条目数。
    #[serde(default)]
    pub entries: usize,
    /// 被跳过的行数。
    #[serde(default)]
    pub skipped: usize,
    /// 冲突（同名多次映射）次数。
    #[serde(default)]
    pub conflicts: usize,
    pub imported_at: u64,
}

/// 持久化状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedState {
    pub version: u32,
    /// 环境定义（归一化后的记录）。用 `Vec` 而不是 map，是为了让文件里的顺序稳定、
    /// 便于 diff；查找走下面的辅助方法。
    pub environments: Vec<EnvironmentJson>,
    /// 规则库账本。
    ///
    /// `#[serde(default)]` 是有意的：升级前写的状态文件没有这一段，加载时必须成立
    /// （否则管理器起不来），随后由物化目录**回填**（见 `Manager::reconcile_rules`）。
    #[serde(default)]
    pub rules: Vec<StoredRules>,
    /// 上游代理账本（环境 `upstream` 字段按名引用这里的条目）。
    ///
    /// `#[serde(default)]` 是有意的：升级前写的状态文件没有这一段，加载时必须成立。
    /// 条目形状 = `UpstreamProxy::to_json` 的结果，加载时由管理器重新校验。
    #[serde(default)]
    pub proxies: Vec<serde_json::Value>,
    #[serde(default)]
    pub desired: BTreeMap<String, Desired>,
    /// 端口是否由管理器**自动分配**。
    ///
    /// 这条决定了启动失败时的行为差异（两条规则不得混用）：自动分配的端口
    /// 冲突时可以换一个重试一次；用户显式指定的端口冲突时必须报 `port_conflict`，
    /// **禁止**静默重分配。
    #[serde(default)]
    pub auto_port: BTreeMap<String, bool>,
    /// 需要跨重启保留的异常标记（`port_conflict` / `invalid_config` 等：上一次动作留下的事实）。
    #[serde(default)]
    pub marks: BTreeMap<String, StoredError>,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            environments: Vec::new(),
            rules: Vec::new(),
            proxies: Vec::new(),
            desired: BTreeMap::new(),
            auto_port: BTreeMap::new(),
            marks: BTreeMap::new(),
        }
    }
}

impl PersistedState {
    pub fn desired_of(&self, name: &str) -> Desired {
        self.desired.get(name).copied().unwrap_or(Desired::Stopped)
    }

    pub fn find(&self, name: &str) -> Option<&EnvironmentJson> {
        self.environments
            .iter()
            .find(|env| env.get("name").and_then(|name| name.as_str()) == Some(name))
    }

    /// 账本里找一条规则。
    pub fn rules_entry(&self, name: &str) -> Option<&StoredRules> {
        self.rules.iter().find(|entry| entry.name == name)
    }

    /// 账本里找一条上游代理。
    pub fn proxy_entry(&self, name: &str) -> Option<&serde_json::Value> {
        self.proxies
            .iter()
            .find(|proxy| proxy.get("name").and_then(|name| name.as_str()) == Some(name))
    }
}

/// 管理器自身配置。CLI 参数只做映射，**不新增默认值**。
#[derive(Debug, Clone)]
pub struct ManagerConfig {
    pub state_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub rules_dir: PathBuf,
    pub agent_dir: PathBuf,
    pub confdir: PathBuf,
    /// 实例日志落盘目录。`None` = **不落盘**，只在内存里保留有界环形缓冲（`--no-log-file`）。
    ///
    /// 默认**落盘**：子进程的 stdout/stderr 直接接文件（没有管道、我们进程不在链路上），
    /// 于是"没人读管道 → 管道写满 → 子进程阻塞在写日志上 → 整个代理挂住"这条路径
    /// 从设计上不存在（实测：默认详细度下 213 个请求就写满 64 KiB 管道）。
    pub log_dir: Option<PathBuf>,
    /// 单个环境日志文件的轮转上限（字节）；`0` = 不轮转。
    pub max_log_bytes: u64,
    pub listen_host: IpAddr,
    pub port_range: (u16, u16),
    pub max_attempts: usize,
}

impl ManagerConfig {
    /// 从一个 state_dir 推出全部默认路径。
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        let state_dir = state_dir.into();
        Self {
            runtime_dir: state_dir.join("runtime"),
            rules_dir: state_dir.join("rules"),
            agent_dir: state_dir.join("agent"),
            confdir: state_dir.join("shared").join("confdir"),
            log_dir: Some(state_dir.join("logs")),
            max_log_bytes: DEFAULT_MAX_LOG_BYTES,
            state_dir,
            listen_host: DEFAULT_LISTEN_HOST,
            port_range: DEFAULT_PORT_RANGE,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        }
    }

    /// 某个环境的 agent 目录（v2 遗留工件的去处；remove 时整目录清扫）。
    ///
    /// 环境名已在 domain 层过白名单校验，所以这里的拼接不会逃出 `agent_dir`。
    pub fn env_agent_dir(&self, env: &str) -> PathBuf {
        self.agent_dir.join(env)
    }

    pub fn state_file(&self) -> PathBuf {
        self.state_dir.join("state.json")
    }

    pub fn lock_file(&self) -> PathBuf {
        self.state_dir.join("lock")
    }

    /// 某个环境的日志文件绝对路径（白名单名字 + 固定父目录）。
    pub fn log_file(&self, env: &str) -> Option<PathBuf> {
        self.log_dir
            .as_ref()
            .map(|dir| dir.join(format!("{env}.log")))
    }

    /// 轮转后保留的那一份。
    pub fn rotated_log_file(&self, env: &str) -> Option<PathBuf> {
        self.log_file(env).map(|path| path.with_extension("log.1"))
    }

    /// 某个环境的规则文件绝对路径 —— **名字白名单 + 固定父目录 + 固定后缀**，
    /// 绝不接受调用方给的路径片段（见 `core/spec/capabilities.md` 的「规则文件（rules）语义」）。
    pub fn rules_path(&self, name: &str) -> PathBuf {
        self.rules_dir.join(envboard_rules::file_name_for(name))
    }

    /// 非法配置**加载即失败**，附字段路径。
    pub fn validate(&self) -> Result<(), Error> {
        let (min, max) = self.port_range;
        if min == 0 || min > max {
            return Err(Error::invalid_config(
                "port_range",
                format!("port_range {min}-{max} is invalid"),
            ));
        }
        if self.max_attempts == 0 {
            return Err(Error::invalid_config(
                "max_attempts",
                "max_attempts must be >= 1",
            ));
        }
        // 轮转上限要么关掉（0），要么大到能放下一次崩溃现场；太小的值会让日志
        // 还没读到就轮转掉，等于静默丢日志。
        if self.max_log_bytes != 0 && self.max_log_bytes < MIN_MAX_LOG_BYTES {
            return Err(Error::invalid_config(
                "max_log_bytes",
                format!(
                    "max_log_bytes must be 0 (no rotation) or >= {MIN_MAX_LOG_BYTES}, got {}",
                    self.max_log_bytes
                ),
            ));
        }
        // confdir 归引擎独占（CA 材料）。v2 的 config.yaml 防线随注入器退场
        // —— 引擎不读任何 confdir 里的用户配置文件。
        Ok(())
    }
}

/// flock 保护的目录锁。
///
/// 拿不到锁**要响亮失败**，而不是"先跑起来再说"：两个管理器同时写状态会让编排分叉，
/// 而且症状（环境莫名重启、端口被抢）极难归因。
#[derive(Debug)]
pub struct InstanceLock {
    _file: std::fs::File,
}

impl InstanceLock {
    pub fn acquire(path: &Path) -> Result<Self, Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)?;
        // 状态目录下的东西一律收紧（同款纪律）。
        restrict_permissions(path)?;

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            // SAFETY: `fd` 来自仍然活着的 `file`；flock 只在同一 fd 上操作。
            let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if rc != 0 {
                let error = std::io::Error::last_os_error();
                return Err(Error::new(
                    ErrorCode::Conflict,
                    format!(
                        "another envboard process holds {} ({error}); \
                         state must have exactly one writer",
                        path.display()
                    ),
                ));
            }
        }

        Ok(Self { _file: file })
    }
}

/// 文件状态存储：原子写 + 0600。
#[derive(Debug)]
pub struct JsonFileStateRepo {
    path: PathBuf,
}

impl JsonFileStateRepo {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl crate::ports::StateRepo for JsonFileStateRepo {
    fn load(&self) -> Result<PersistedState, Error> {
        if !self.path.exists() {
            return Ok(PersistedState::default());
        }
        let raw = std::fs::read_to_string(&self.path)?;
        let state: PersistedState = serde_json::from_str(&raw).map_err(|error| {
            Error::new(
                ErrorCode::StoreFailure,
                format!("{} is not a valid state file: {error}", self.path.display()),
            )
        })?;
        if state.version > STATE_VERSION {
            return Err(Error::new(
                ErrorCode::StoreFailure,
                format!(
                    "state file version {} is newer than supported version {STATE_VERSION}",
                    state.version
                ),
            ));
        }
        Ok(state)
    }

    fn save(&self, state: &PersistedState) -> Result<(), Error> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_vec_pretty(state)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &raw)?;
        restrict_permissions(&tmp)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// 凭据/状态文件必须收紧权限。
fn restrict_permissions(path: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

impl fmt::Display for ManagerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "state_dir={} port_range={}-{}",
            self.state_dir.display(),
            self.port_range.0,
            self.port_range.1
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::StateRepo;

    #[test]
    fn config_derives_paths_from_state_dir() {
        let config = ManagerConfig::new("/tmp/envboard-test");
        assert_eq!(
            config.rules_path("beta"),
            PathBuf::from("/tmp/envboard-test/rules/beta.rules")
        );
        assert_eq!(config.lock_file(), PathBuf::from("/tmp/envboard-test/lock"));
    }

    #[test]
    fn a_v2_state_file_loads_and_rewrites_without_process_channels() {
        // v2 的 state.json 里有 records/config_seals（进程身份与配置封印）这些键。
        // v3 结构不认识它们：加载必须成立（serde 忽略未知键），再保存时旧键整体
        // 消失、环境数据原样搬过来 —— 这就是"直接读旧状态文件升级"的契约测试。
        let sample = r#"{
  "version": 1,
  "environments": [
    {"name": "beta", "description": "", "listen": {"host": "127.0.0.1", "port": 16440},
     "insecure_hosts": ["api.example.com"], "proxy_user": null, "proxy_password": null,
     "rules": "beta"}
  ],
  "rules": [],
  "desired": {"beta": "running"},
  "records": {"beta": {"pid": 4242, "starttime": 999, "cmdline": ["mitmdump"]}},
  "auto_port": {"beta": false},
  "marks": {},
  "config_seals": {"beta": {"hash": "deadbeef", "written_at": 1000}}
}"#;
        let dir = std::env::temp_dir().join(format!("envboard-v2state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        std::fs::write(&path, sample).unwrap();
        let repo = JsonFileStateRepo::new(&path);
        let state = crate::ports::StateRepo::load(&repo).unwrap();
        assert_eq!(state.environments.len(), 1);
        assert_eq!(state.desired_of("beta"), Desired::Running);
        crate::ports::StateRepo::save(&repo, &state).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("records"), "进程身份通道不得回写");
        assert!(!text.contains("config_seals"), "配置封印不得回写");
        assert!(text.contains("\"beta\""), "环境数据原样搬过来");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_port_range_fails_loudly() {
        let mut config = ManagerConfig::new("/tmp/envboard-test");
        config.port_range = (17_000, 16_000);
        let error = config.validate().unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidConfig);
        assert_eq!(error.field.as_deref(), Some("port_range"));
    }

    #[test]
    fn lock_is_exclusive_even_within_one_process() {
        let dir = std::env::temp_dir().join(format!("envboard-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lock");

        let first = InstanceLock::acquire(&path).unwrap();
        // flock 是按**打开文件描述**加的，所以同一进程里第二次 open + LOCK_EX 也会失败 ——
        // 这正是我们要的：第二个管理器进程（或进程内的第二个 Manager）必须被挡住，
        // 而不是"先跑起来再说"。
        let second = InstanceLock::acquire(&path);
        assert!(second.is_err(), "a second lock holder must be rejected");
        assert_eq!(second.unwrap_err().code, ErrorCode::Conflict);

        drop(first);
        assert!(
            InstanceLock::acquire(&path).is_ok(),
            "lock must be released on drop"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn state_round_trips_and_defaults_to_stopped() {
        let dir = std::env::temp_dir().join(format!("envboard-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let repo = JsonFileStateRepo::new(dir.join("state.json"));
        assert_eq!(repo.load().unwrap().desired_of("beta"), Desired::Stopped);

        let mut state = PersistedState::default();
        state.desired.insert("beta".into(), Desired::Running);
        repo.save(&state).unwrap();
        assert_eq!(repo.load().unwrap().desired_of("beta"), Desired::Running);
        std::fs::remove_dir_all(&dir).ok();
    }
}

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

use envboard_engine::domain::{DEFAULT_MAX_ATTEMPTS, DEFAULT_PORT_RANGE, Desired};
use envboard_engine::{DEFAULT_LISTEN_HOST, Error, ErrorCode};
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

/// 状态目录的**布局表** —— 全仓唯一的一份。
///
/// 为什么单独成表，而不是散落在各处的 `join("...")`：这份表回答的是"状态目录
/// 里能出现什么"，而**路径规则一旦有两份实现就必然漂移** —— 改了一处忘了另一处，
/// 症状是"界面读的是 A、管理器写的是 B"，这种分叉从外部几乎无法归因。
///
/// 界面偏好（`ui.json`）也登记在这里，尽管写它的是界面而不是管理器：
/// 它同样住在状态目录里，规则就不该有第二份。
///
/// 纪律：这些字面量**只在本文件出现**，其余代码一律经 `layout::*` 常量拼路径。
pub mod layout {
    /// 账本（环境、规则账本、代理账本、期望状态）—— **v3 起的权威存储是 SQLite**
    /// （[`super::super::state_sqlite`]，ADR-007）；`state.json` 仅作迁移源保留。
    pub const STATE_DB: &str = "state.db";
    /// 旧 JSON 账本（迁移源；迁移后改名 `state.json.migrated` 留备份）。
    pub const STATE_FILE: &str = "state.json";
    /// 界面偏好（主题；界面写，不进账本）。
    pub const UI_PREFS_FILE: &str = "ui.json";
    /// 控制面审计事件（append-only；权威仍是账本）。
    pub const EVENTS_FILE: &str = "events.jsonl";
    /// 单实例锁（状态只有一个写者的凭据）。
    pub const LOCK_FILE: &str = "lock";
    /// 规则物化目录（账本 rendered 的**可再生**投影）。
    pub const RULES_DIR: &str = "rules";
    /// 实例日志目录（可被 `--log-dir` 改址；`--no-log-file` 关闭）。
    pub const LOGS_DIR: &str = "logs";
    /// 请求轨迹目录（数据面事件流；字节偏移游标）。
    ///
    /// 它**不跟 `--log-dir` 走**：轨迹是数据面事件流（有自己的游标与生命周期），
    /// 不是实例日志；把它挂在日志目录下会让"改日志目录"顺带搬走轨迹 —— 一条
    /// 与调用方意图无关的副作用。它跟的是"要不要落盘"（`--no-log-file`）。
    pub const TRAJECTORIES_DIR: &str = "trajectories";
    /// 共享材料目录（目前只有 CA）。
    pub const SHARED_DIR: &str = "shared";
    /// CA 材料目录（在共享目录下；文件名规则归引擎，见 `engine::ca`）。
    pub const CONFDIR_DIR: &str = "confdir";
    /// **v2 遗留**的 agent 工件目录（注入器时代）。
    ///
    /// v3 的产品路径**永不创建**它；这里登记它只为了一件事：升级时把旧状态目录里
    /// 残留的 `<state_dir>/agent/<env>/` 清掉（见 [`super::ManagerConfig::legacy_agent_dir`]）。
    /// 登记在表里而不是随手拼字面量，是为了"这个名字在本仓只出现一次"这条纪律。
    pub const LEGACY_AGENT_DIR: &str = "agent";
}

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
    pub rules_dir: PathBuf,
    pub confdir: PathBuf,
    /// 请求轨迹目录（数据面事件流）。
    ///
    /// `None` = 轨迹不落盘（`--no-log-file`）。
    ///
    /// **位置不跟 `--log-dir` 走**：轨迹是数据面事件流（有字节偏移游标、独立生命周期），
    /// 不是实例日志。原来它挂在 `log_dir` 下面，于是"改日志目录"会顺带搬走轨迹
    /// —— 一条与调用方意图无关的副作用。现在两件事各归各：**落到哪**由状态目录定，
    /// **要不要落盘**由 `--no-log-file` 定。
    pub trajectories_dir: Option<PathBuf>,
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
    /// 抓包缓冲字节预算（每环境；0 = 引擎默认 256 MiB）。
    pub capture_budget: usize,
}

impl ManagerConfig {
    /// 从一个 state_dir 推出全部默认路径。
    ///
    /// **所有路径都从 `state_dir` 派生**（`--log-dir` 是唯一的例外，它是显式覆盖）。
    /// 字面量只在 [`layout`] 里出现一次；这里不再重复任何目录名。
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        let state_dir = state_dir.into();
        Self {
            rules_dir: state_dir.join(layout::RULES_DIR),
            confdir: state_dir.join(layout::SHARED_DIR).join(layout::CONFDIR_DIR),
            trajectories_dir: Some(state_dir.join(layout::TRAJECTORIES_DIR)),
            log_dir: Some(state_dir.join(layout::LOGS_DIR)),
            max_log_bytes: DEFAULT_MAX_LOG_BYTES,
            state_dir,
            listen_host: DEFAULT_LISTEN_HOST,
            port_range: DEFAULT_PORT_RANGE,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            capture_budget: 0,
        }
    }

    /// 账本数据库（环境、规则、代理、期望状态的唯一真相；SQLite + WAL）。
    ///
    /// `-wal`/`-shm` 伴生文件由 SQLite 自管，不登记进布局表（不是本仓写的）。
    pub fn state_db(&self) -> PathBuf {
        self.state_dir.join(layout::STATE_DB)
    }

    /// 旧 JSON 账本路径（一次性迁移源；见 `state_sqlite::SqliteStateRepo::open`）。
    pub fn state_file(&self) -> PathBuf {
        self.state_dir.join(layout::STATE_FILE)
    }

    /// 界面偏好文件（桌面形态写；坏文件回落到默认，永不拦启动）。
    ///
    /// 路径规则登记在 [`layout`] 里，但**写它的是界面形态** —— 所以这个函数
    /// 归界面那一侧用，管理器自己不碰这个文件。
    pub fn ui_prefs_file(&self) -> PathBuf {
        self.state_dir.join(layout::UI_PREFS_FILE)
    }

    /// 控制面审计事件日志（E-B：权威在 state.json，事件是观察）。
    pub fn events_file(&self) -> PathBuf {
        self.state_dir.join(layout::EVENTS_FILE)
    }

    /// 某个环境的请求轨迹文件（`<state_dir>/trajectories/<env>.jsonl`）。
    ///
    /// `None` = 轨迹不落盘（`--no-log-file`）。
    pub fn trajectory_file(&self, env: &str) -> Option<PathBuf> {
        self.trajectories_dir
            .as_ref()
            .map(|dir| dir.join(format!("{env}.jsonl")))
    }

    pub fn lock_file(&self) -> PathBuf {
        self.state_dir.join(layout::LOCK_FILE)
    }

    /// **v2 遗留**的 agent 工件目录（注入器时代）：`<state_dir>/agent/<env>/`。
    ///
    /// v3 的产品路径永不创建它 —— 这个方法只为**清扫**而存在：删环境 / 改环境名时
    /// 把旧状态目录里的残留一并带走（残留不清就永远留在那儿，占着地方还误导人）。
    /// 环境名已在 domain 层过白名单校验，所以这里的拼接不会逃出 agent 目录。
    pub fn legacy_agent_dir(&self, env: &str) -> PathBuf {
        self.state_dir.join(layout::LEGACY_AGENT_DIR).join(env)
    }

    /// 某个环境的日志文件绝对路径（白名单名字 + 固定父目录）。
    pub fn log_file(&self, env: &str) -> Option<PathBuf> {
        self.log_dir
            .as_ref()
            .map(|dir| dir.join(format!("{env}.log")))
    }

    /// 某个环境的规则文件绝对路径 —— **名字白名单 + 固定父目录 + 固定后缀**，
    /// 绝不接受调用方给的路径片段（见 envboard-engine::rules 模块文档）。
    pub fn rules_path(&self, name: &str) -> PathBuf {
        self.rules_dir
            .join(envboard_engine::rules::file_name_for(name))
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
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::io_at("create the state directory", parent, e))?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .map_err(|e| Error::io_at("open the instance lock", path, e))?;
        // 状态目录下的东西一律收紧（同款纪律）。
        restrict_permissions(path)?;

        // 独占建议锁：std 跨平台实现（unix = flock，Windows = LockFileEx）。
        // 单写者契约必须在两个平台都成立 —— 旧实现只在 unix 加锁，Windows 上
        // 第二个实例会静默启动（状态双写者，且 smoke 互斥断言永远挂死）。
        if let Err(error) = file.try_lock() {
            return Err(Error::new(
                ErrorCode::Conflict,
                format!(
                    "another envboard process holds {} ({error}); \
                     state must have exactly one writer",
                    path.display()
                ),
            ));
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
        let raw = std::fs::read_to_string(&self.path)
            .map_err(|e| Error::io_at("read the state file", &self.path, e))?;
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
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::io_at("create the state directory", parent, e))?;
        }
        let raw = serde_json::to_vec_pretty(state)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &raw).map_err(|e| Error::io_at("write the state file", &tmp, e))?;
        restrict_permissions(&tmp)?;
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| Error::io_at("replace the state file", &self.path, e))?;
        Ok(())
    }
}

/// 凭据/状态文件必须收紧权限。
pub(crate) fn restrict_permissions(path: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
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
    fn the_trajectory_sits_beside_the_logs_not_inside_them() {
        // 轨迹是数据面事件流，不是实例日志：它与 `logs/` **平级**，
        // 不挂在日志目录下 —— 否则 `--log-dir` 改址会顺带搬走轨迹。
        let config = ManagerConfig::new("/tmp/envboard-test");
        let trajectory = config.trajectory_file("beta").expect("默认要落盘");
        assert_eq!(
            trajectory,
            PathBuf::from("/tmp/envboard-test/trajectories/beta.jsonl"),
            "轨迹必须在状态目录的 trajectories 下"
        );
        let log_dir = config.log_dir.clone().expect("默认要落盘");
        assert!(
            !trajectory.starts_with(&log_dir),
            "轨迹不该落在日志目录里面：{trajectory:?}"
        );
    }

    #[test]
    fn the_layout_table_covers_everything_the_state_dir_holds() {
        // 布局表是"规范"：少登记一项就等于给布局开了个没锁的口子。
        assert_eq!(layout::STATE_DB, "state.db");
        assert_eq!(layout::STATE_FILE, "state.json");
        assert_eq!(layout::UI_PREFS_FILE, "ui.json");
        assert_eq!(layout::EVENTS_FILE, "events.jsonl");
        assert_eq!(layout::LOCK_FILE, "lock");
        assert_eq!(layout::RULES_DIR, "rules");
        assert_eq!(layout::LOGS_DIR, "logs");
        assert_eq!(layout::TRAJECTORIES_DIR, "trajectories");
        assert_eq!(layout::SHARED_DIR, "shared");
        assert_eq!(layout::CONFDIR_DIR, "confdir");
        assert_eq!(layout::LEGACY_AGENT_DIR, "agent");
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
        // 锁按**打开文件描述/句柄**生效，所以同一进程里第二次 open + try_lock 也会失败 ——
        // 这正是我们要的：第二个管理器进程（或进程内的第二个 Manager）必须被挡住，
        // 而不是"先跑起来再说"。（std try_lock 跨平台：unix flock / Windows LockFileEx。）
        {
            let second = InstanceLock::acquire(&path);
            assert!(second.is_err(), "a second lock holder must be rejected");
            assert_eq!(second.unwrap_err().code, ErrorCode::Conflict);
        }

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

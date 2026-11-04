//! 装配：引擎 + 共享 CA + 管理器 + Admin 门面，一次构造交出去。
//!
//! 本文件是全仓**唯一**允许出现引擎装配符号（`EngineBackend` / `SharedCa`）的地方 ——
//! 判据由 `envboard-policy-tests` 的 `deps` 门禁在源文件面强制。

use std::path::PathBuf;
use std::sync::Arc;

use envboard_admin::AdminService;
use envboard_engine::ca::CaStore;
use envboard_engine::{CaOutcome, EngineBackend, Error, ErrorCode, ProxyEngine, SharedCa};
use envboard_manager::infra::{RealFiles, SocketPortProbe, StderrLogger, SystemClock};
use envboard_manager::{JsonFileStateRepo, Manager, ManagerConfig, StateRepo};

use crate::ca_control::CaControl;
use crate::options::HostOptions;

/// 装配完成的宿主：界面形态拿到它之后只需要 [`Prepared::admin`] 与 [`Prepared::startup`]。
///
/// 内部持有的 [`Manager`] 是 **crate 内私有**的：编排要用它，但界面形态拿不到
/// 管理器类型（依赖方向上界面 crate 也不允许直依 manager）。
pub struct Prepared {
    pub(crate) manager: Arc<Manager>,
    /// 交给界面形态的管理门面（唯一的使用面）。
    pub admin: Arc<AdminService>,
    /// 共享 CA 的操作面（设置页：摘要、重新签发、按域名签发、信任库探测）。
    ///
    /// `None` = 本宿主没有物化 CA（测试注入的假引擎）。
    pub ca: Option<CaControl>,
    /// 装配期产生的、**不允许静默**的警告（例如共享 CA 被重新物化：
    /// 已装证书的客户端要重装）。由调用方决定是打印还是渲染成界面提示。
    pub warnings: Vec<String>,
    /// 状态目录（宿主参数与界面展示都要用）。
    pub state_dir: PathBuf,
    /// 界面偏好文件（主题）的路径。
    ///
    /// 它住在状态目录里，但**写它的是界面形态**（桌面形态的 `UiPrefs`）——
    /// 管理器自己不碰这个文件。可路径规则只有一份（`envboard_manager::layout`），
    /// 所以这里把算好的路径交出去：界面形态不必自己拼 `state_dir.join("ui.json")`，
    /// 也就不会有第二份路径实现（两份必然漂移，漂移的症状从外部几乎无法归因）。
    pub ui_prefs_file: PathBuf,
    /// CA 材料所在目录（设置屏据此指出"证书文件在哪"）。
    ///
    /// 只给目录不给路径：文件名规则属于引擎（`envboard_engine::ca::cert_path`），
    /// 这里复制一遍就等于埋了一次静默漂移。
    pub ca_dir: PathBuf,
    /// 随机分配端口的区间（生效值）。
    pub port_range: (u16, u16),
}

impl Prepared {
    /// 当前账本代次（状态栏与"界面是否落后于事实"的参考量，不是渲染开关：
    /// 快照是全量的，见 spec/admin-api.md 的游标语义）。
    pub fn state_generation(&self) -> u64 {
        self.manager.state_generation()
    }
}

/// 用真实进程内引擎装配一个宿主。
pub fn prepare(options: HostOptions) -> Result<Prepared, Error> {
    let config = options.into_config()?;
    let (engine, ca, warnings) = build_engine(&config)?;
    build_prepared(config, engine, ca, warnings)
}

/// **测试注入口**：用给定的引擎实现装配一个宿主（共享 CA 不物化）。
///
/// 存在的理由：界面形态的测试需要一个真管理器 + 假引擎（不装任何东西、不占端口），
/// 而它们要走的正是生产的那条装配路径 —— 复制一遍构造序列就等于测试与实现脱钩。
pub fn prepare_with(options: HostOptions, engine: Arc<dyn ProxyEngine>) -> Result<Prepared, Error> {
    let config = options.into_config()?;
    build_prepared(config, engine, None, Vec::new())
}

/// 引擎装配的产物：引擎句柄 + 共享 CA 公开面 + 装配期警告。
type Assembled = (Arc<dyn ProxyEngine>, Option<CaControl>, Vec<String>);

/// v3 引擎构造：进程内纯库引擎，共享 CA 在 confdir 就绪（兼容既有 mitmproxy
/// 生成的 CA 文件，已装证书的客户端零感知）。
///
/// CA 被重新物化时**必须**带回警告 —— 已装证书的客户端要重装，这条不能静默。
/// 原来是 `eprintln!` 直接打到 stderr；现在提升为数据，宿主打印、界面渲染都来得及。
fn build_engine(config: &ManagerConfig) -> Result<Assembled, Error> {
    let (ca, outcome) = SharedCa::load_or_create(&config.confdir)?;
    let mut warnings = Vec::new();
    if let CaOutcome::Regenerated {
        fingerprint,
        reason,
    } = &outcome
    {
        warnings.push(format!(
            "shared CA was regenerated ({reason}); fingerprint {fingerprint}"
        ));
        warnings.push(format!(
            "clients must reinstall the CA from {}",
            config.confdir.display()
        ));
    }
    // 引擎与界面共用同一份「当前 CA」：重新签发时两边看到的是同一张。
    let store = CaStore::new(ca);
    let control = CaControl::new(store.clone(), config.confdir.clone());
    Ok((Arc::new(EngineBackend::new(store)), Some(control), warnings))
}

/// 管理器构造（含单实例锁）+ Admin 门面。引擎由此处注入，所以真装配与测试注入
/// 共用同一段构造序列。
fn build_prepared(
    config: ManagerConfig,
    engine: Arc<dyn ProxyEngine>,
    ca: Option<CaControl>,
    warnings: Vec<String>,
) -> Result<Prepared, Error> {
    let state_dir = config.state_dir.clone();
    let ui_prefs_file = config.ui_prefs_file();
    let ca_dir = config.confdir.clone();
    let port_range = config.port_range;
    let state_file = config.state_file();
    let manager = Manager::new(
        config,
        engine,
        Arc::new(SocketPortProbe),
        Arc::new(RealFiles),
        Arc::new(SystemClock),
        Arc::new(StderrLogger),
        Arc::new(JsonFileStateRepo::new(state_file)) as Arc<dyn StateRepo>,
        // 单实例锁：拿不到就响亮失败（状态只有一个写者）。
        true,
    )
    .map_err(ownership_conflict)?;
    let manager = Arc::new(manager);
    Ok(Prepared {
        state_dir,
        ui_prefs_file,
        ca_dir,
        port_range,
        manager: Arc::clone(&manager),
        admin: Arc::new(AdminService::new(manager)),
        ca,
        warnings,
    })
}

/// 把「拿不到单实例锁」翻译成人话：原样保留错误码与字段，只补一句可操作的说明。
fn ownership_conflict(error: Error) -> Error {
    if error.code == ErrorCode::Conflict {
        return Error {
            message: format!(
                "{} — another envboard workbench already owns this state dir; stop it first",
                error.message
            ),
            ..error
        };
    }
    error
}

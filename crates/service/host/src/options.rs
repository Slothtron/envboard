//! [`HostOptions`]：组合根的入参 —— 用户意图到 [`ManagerConfig`] 的翻译落点。
//!
//! 纪律：**读环境变量是适配器的职责**（core 禁止直接读环境变量），所以
//! `ENVBOARD_STATE_DIR` / 用户主目录的回退链落在本文件，而不是渗进 manager 或
//! domain。命令行翻译是宿主二进制（`crates/app/desktop/src/lib.rs`）的事：它把
//! `--state-dir` 等旗标翻成这里的字段；本文件自己只认环境变量、平台约定，
//! 以及调用方（测试、示例）直接填的字段。
//!
//! ## 状态目录怎么定下来（这里是唯一的裁决点）
//!
//! 回退链有四级，顺序不可换：
//!
//! 1. [`HostOptions::state_dir`] —— 调用方直接给目录（web 宿主经 `--state-dir`
//!    暴露它；不给则走下面三级）；
//! 2. `$ENVBOARD_STATE_DIR` —— 机器级环境变量（CI、容器、临时实验用它指到别处；
//!    空值等同未设置）；
//! 3. **便携模式** —— exe 同目录存在 `<exe 名>.portable` 标记时，数据落到同目录的
//!    `<exe 名>-data`（绿色免安装分发靠它，见 [`PORTABLE_MARKER_SUFFIX`]）；
//! 4. **平台默认状态目录** —— 见 [`platform_default_state_dir`]。
//!
//! 第 4 级以前写死 `$HOME/.envboard`，在 Windows 上是**错的**：`HOME` 不是
//! Windows 的约定（原生进程通常根本没有这个变量），于是回退链在 Windows 上
//! 直接断掉，报「cannot determine a state dir」—— 而用户明明在 Windows 上。
//! 现在第 4 级按平台各取各的默认位置（Windows `%LOCALAPPDATA%`、macOS
//! `~/Library/Application Support`、Linux `$XDG_STATE_HOME`），且**取不到就取不到**：
//! 宁可响亮失败让用户设 `$ENVBOARD_STATE_DIR`，也不要猜一个猜错的目录（猜错的
//! 后果是账本写到别处，症状是"环境列表空了"）。
//!
//! **三家的共同点**：`<平台基础目录>/envboard` 这样"套一层自己的目录"是规范要求，
//! 而不是风格偏好；**主目录根下直接放 `.envboard`** 在三个平台都违反约定
//! （Windows 有明文禁止，macOS 要求自动生成的文件进 `Library`，Linux 要求进
//! `.config`/`.local/*`/`.cache`）。

use std::path::{Path, PathBuf};

use envboard_engine::Error;
use envboard_manager::ManagerConfig;

/// 状态目录的**环境变量名**（第 2 级）。
pub const STATE_DIR_ENV: &str = "ENVBOARD_STATE_DIR";

/// 便携模式标记的后缀（第 3 级）：exe 同目录存在 `<应用名>.portable` 即启用。
///
/// 用"标记文件"而不是"目录已存在"当判据：目录可以不存在（首次运行才建），而
/// 标记是发行包里**明确放进去**的意图表达 —— 判据不能是该由应用自己创建的路径，
/// 否则第一次运行之前它永远不成立。
pub const PORTABLE_MARKER_SUFFIX: &str = ".portable";

/// 便携模式的数据目录后缀：`<exe 目录>/<应用名>-data`。
///
/// 名字由应用名推出来（不写死 `envboard`），与本文件的纪律一致：组合根不知道
/// 产品叫什么，名字由宿主二进制传进来。
pub const PORTABLE_DATA_SUFFIX: &str = "-data";

/// 起一个宿主（任意界面形态）所需的全部选择。
///
/// 字段全是 `Option`：默认值由 [`ManagerConfig::new`] 从 `state_dir` 推出，
/// 这里不重复任何一条默认（重复就会漂移）。
#[derive(Debug, Clone, Default)]
pub struct HostOptions {
    /// 调用方**直接指定**的状态目录（回退链第 1 级）。
    ///
    /// 桌面宿主（`envboard-desktop`）经 `--state-dir` 暴露它；组合根本身不读
    /// 命令行 —— 翻译是宿主二进制的事。`None` 时走回退链：
    /// `$ENVBOARD_STATE_DIR` → 便携标记 → 平台默认目录。
    pub state_dir: Option<PathBuf>,
    /// **应用自己的目录名**（平台默认目录里 `<基础目录>/<这个名字>` 的那一层）。
    ///
    /// 它是**产品身份**，不是路径规则，所以组合根不猜也不硬编码：由宿主二进制从
    /// `Cargo.toml` 取（`env!("CARGO_BIN_NAME")`，即 `[[bin]] name`）再传进来。
    /// `envboard-host` 是个通用组合根，它没有资格知道产品叫什么。
    ///
    /// `None` = 不提供平台默认目录与便携默认目录：回退链第 3、4 级直接不存在
    /// （调用方要么自己给 `state_dir`，要么设 `$ENVBOARD_STATE_DIR`）。这也是
    /// `Default` 的值 —— 宁可让 `HostOptions::default()` 缺少默认目录，也不要
    /// 凭空编一个名字把账本写进 `%LOCALAPPDATA%\unknown`。
    pub app_dir_name: Option<String>,
    /// 随机分配端口的区间（`--port-range 16000-16999` 的翻译结果）。
    pub port_range: Option<(u16, u16)>,
    /// 抓包缓冲字节预算（每环境；`0` = 引擎默认）。
    pub capture_budget: usize,
    /// 实例日志落盘目录；`None` = 用默认（`<state_dir>/logs`）。
    pub log_dir: Option<PathBuf>,
    /// 显式要求**不落盘**（日志只留在内存的有界环形缓冲）。与 `log_dir` 同时给出时
    /// 以本标志为准 —— 它表达的是"我就是不要文件"。
    pub no_log_file: bool,
    /// 单个环境日志文件的轮转上限（字节；`0` = 不轮转）。
    pub max_log_bytes: Option<u64>,
}

impl HostOptions {
    /// 解析状态目录：调用方给的值 → `$ENVBOARD_STATE_DIR` → 便携标记 → 平台默认目录。
    ///
    /// 四级都没有才响亮失败（原来这条判据在宿主 `main.rs` 里，语义不变）。
    ///
    /// **环境变量的空值等同未设置**：`ENVBOARD_STATE_DIR=` 在 shell 里通常是
    /// "变量展开成了空"（`ENVBOARD_STATE_DIR=${SOME_UNSET}`），不是"我要一个空路径"；
    /// 照空值用会得到一个空 `PathBuf`。
    ///
    /// **环境变量压过便携标记**（第 2 级在第 3 级之前）：绿色版里想临时换位置，
    /// 设一个变量就够了，不必去删发行包里的标记文件。反过来（标记压过变量）会让
    /// 绿色包在 CI 与临时实验里"指不动"，那是最难排查的一类。
    ///
    /// **取到的值一律规范化**（见 [`normalize_state_dir`]）：`~` 展开成用户主目录、
    /// 相对路径补成绝对路径。`~` 是 **shell** 的展开规则而不是路径规则（直接传进
    /// 程序会得到一个字面目录名 `~`），相对路径的基准又取决于进程 cwd
    /// （换个启动方式就指到别处）。
    pub fn resolve_state_dir(&self) -> Result<PathBuf, Error> {
        let raw = self
            .state_dir
            .clone()
            .or_else(|| {
                std::env::var_os(STATE_DIR_ENV)
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
            })
            .or_else(|| {
                // 第 3、4 级都要有"叫什么"才能算出来；没给名字就没有这两级。
                let name = self.app_dir_name.as_deref()?;
                // 便携模式以**正在运行的 exe** 为基准：绿色包被拷到哪、从哪启动，
                // 数据就跟到哪。`current_exe()` 取不到的场合（极少）直接跳过这一级，
                // 不把"取不到自己的路径"升级成启动失败。
                std::env::current_exe()
                    .ok()
                    .and_then(|exe| portable_state_dir(&exe, name))
                    .or_else(|| platform_default_state_dir(name))
            });
        let Some(raw) = raw else {
            return Err(Error::invalid_config(
                "state_dir",
                format!("cannot determine a state dir; set {STATE_DIR_ENV}"),
            ));
        };
        normalize_state_dir(raw)
    }

    /// 翻译成经过校验的 [`ManagerConfig`]。
    ///
    /// `port_range` 的**文本**解析（`"min-max"`）在调用方的 CLI 层做完，这里只接收
    /// 已经成形的区间 —— 参数怎么拼属于界面形态，区间是否合法属于 domain。
    pub fn into_config(self) -> Result<ManagerConfig, Error> {
        let mut config = ManagerConfig::new(self.resolve_state_dir()?);
        config.capture_budget = self.capture_budget;
        if let Some(dir) = self.log_dir {
            config.log_dir = Some(dir);
        }
        if self.no_log_file {
            config.log_dir = None;
            // 轨迹跟"要不要落盘"走，不跟"落到哪"走（见 `ManagerConfig`）。
            config.trajectories_dir = None;
        }
        if let Some(bytes) = self.max_log_bytes {
            config.max_log_bytes = bytes;
        }
        if let Some(range) = self.port_range {
            config.port_range = range;
        }
        config.validate()?;
        Ok(config)
    }
}

/// 便携模式的状态目录（回退链第 3 级）：**只看 exe 同目录**，不看 cwd。
///
/// 判据是标记文件 `<exe 目录>/<应用名>.portable` 存在 —— 绿色包把它和法律意义上的
/// "这是便携副本"一起交给用户，应用自己不猜（不因为"exe 不在 Program Files 里"
/// 就自作聪明改布局）。数据目录固定为 `<exe 目录>/<应用名>-data`：
///
/// * **跟 exe 走**，所以整个目录拷到 U 盘、换一台机器都自带账本；
/// * **不与 exe 同级散落**，所以发行目录里只有一个 exe、一个标记、一个数据目录，
///   用户删数据 = 删那一个目录；
/// * **单实例锁跟着状态目录**，于是两个便携副本（各自目录）可以同时开，
///   而同一个副本开两个仍是响亮失败 —— 语义与机器级安装一致。
///
/// 以 `current_exe()` 为基准而不是 `current_dir()`：后者取决于谁拉起进程（资源管理器、
/// 快捷方式、终端），"数据在哪"不能由启动方式决定。
pub fn portable_state_dir(exe: &Path, app_dir_name: &str) -> Option<PathBuf> {
    if app_dir_name.is_empty() {
        return None;
    }
    let exe_dir = exe.parent()?;
    let marker = exe_dir.join(format!("{app_dir_name}{PORTABLE_MARKER_SUFFIX}"));
    marker
        .exists()
        .then(|| exe_dir.join(format!("{app_dir_name}{PORTABLE_DATA_SUFFIX}")))
}

/// 平台默认的状态目录（回退链第 4 级）。
///
/// 三家的共同结构是 `[平台基础目录]/<app_dir_name>`，分歧在于"机器相关的应用数据"
/// 落在哪个基础目录：
///
/// * **Windows**：`%LOCALAPPDATA%\envboard`。`LocalAppData` 的定义就是
///   「a data repository for **local (nonroaming)** applications」。不选 `%APPDATA%`：
///   状态里含端口分配与单实例锁，跟着域账号漫游只会让两台机器互相打架，CA 私钥
///   更没有漫游的理由。也不选主目录根 —— `CSIDL_PROFILE` 明文禁止在那里建目录。
/// * **macOS**：`~/Library/Application Support/envboard`。macOS **只有这一档**：
///   `FileManager.SearchPathDirectory` 里既没有 `state` 也没有 `logs`，配置与数据
///   本来就同住 `Application Support`。（沙箱应用下 `HOME` 是容器，于是自动落在
///   容器内的 `Application Support`，正是 Apple 要的结果。）
/// * **Linux**：`$XDG_STATE_HOME/envboard`，未设时 `$HOME/.local/state/envboard`。
///   XDG 的 state 语义是「跨重启留存、但不够重要到进 `$XDG_DATA_HOME`」，
///   与 Windows 的 nonroaming 是同一条轴（"这台机器上的东西"）。
///
/// **不读 `$HOME/.envboard`**：那是 Unix 习惯，且在三个平台都违反"主目录根不许
/// 放应用目录"这条。旧实现只读 `HOME` 还把它拼在根下，在 Windows 上直接断链
/// （原生进程通常没有 `HOME`）。
///
/// `app_dir_name` 由宿主二进制从 `Cargo.toml` 取（`env!("CARGO_BIN_NAME")`）传进来，
/// 不在这里硬编码 —— 见 [`HostOptions::app_dir_name`]。
pub fn platform_default_state_dir(app_dir_name: &str) -> Option<PathBuf> {
    let platform = current_platform();
    let local_app_data = std::env::var_os("LOCALAPPDATA");
    let home = std::env::var_os("HOME");
    let xdg_state_home = std::env::var_os("XDG_STATE_HOME");
    default_state_dir_for(
        platform,
        app_dir_name,
        local_app_data.as_deref(),
        home.as_deref(),
        xdg_state_home.as_deref(),
    )
}

/// 当前宿主平台。三个分支都不在编译期裁掉 —— 下面的纯函数要靠它被逐平台测试。
fn current_platform() -> Platform {
    if cfg!(windows) {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::MacOs
    } else {
        Platform::Linux
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    Windows,
    MacOs,
    Linux,
}

/// 平台默认目录的**纯决策**：不读环境，只吃"叫什么"和三个候选基础目录。
///
/// 单独拆出来是为了**在没有 Mac 构建机的仓里测 macOS 规则** —— 否则那条分支
/// 只有等真机上跑才发现写错，而它出错的表现是"起了个空工作台"（账本在另一个
/// 目录），从界面上看不出来。
///
/// 环境变量的两条规范纪律都在这里落地：
///
/// * **空串等同未设置**（XDG 对每个变量都这么定义）—— 当成 `None` 往下回退，
///   否则 `join` 出来会是一个相对路径，等于把状态写到进程 cwd；
/// * **相对路径视为无效**（XDG 原文：`should consider the path invalid and ignore
///   it`）—— 只对 `XDG_STATE_HOME` 强制，因为那是规范明文；Windows 的
///   `LOCALAPPDATA` 没有同名规定，交给调用方的规范化补成绝对即可。
///
/// `app_dir_name` 为空串时同样算"没给名字"（返回 `None`）：空名字会 `join` 出
/// 基础目录本身，等于把账本直接铺在 `%LOCALAPPDATA%` 里。
fn default_state_dir_for(
    platform: Platform,
    app_dir_name: &str,
    local_app_data: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
    xdg_state_home: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    fn non_empty(value: Option<&std::ffi::OsStr>) -> Option<&std::ffi::OsStr> {
        value.filter(|v| !v.is_empty())
    }
    if app_dir_name.is_empty() {
        return None;
    }
    match platform {
        Platform::Windows => {
            non_empty(local_app_data).map(|base| PathBuf::from(base).join(app_dir_name))
        }
        Platform::MacOs => non_empty(home).map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join(app_dir_name)
        }),
        Platform::Linux => {
            let xdg = non_empty(xdg_state_home).filter(|base| PathBuf::from(base).is_absolute());
            match xdg {
                Some(base) => Some(PathBuf::from(base).join(app_dir_name)),
                None => non_empty(home).map(|home| {
                    PathBuf::from(home)
                        .join(".local")
                        .join("state")
                        .join(app_dir_name)
                }),
            }
        }
    }
}

/// 把用户给的状态目录规范化成一个**能直接用**的绝对路径。
///
/// 处理三件事，每一件都对应一类真实报障：
///
/// 1. **空串** → 报错（空路径不是"默认"，是"没给"）；
/// 2. **`~` 开头** → 展开成用户主目录（`ENVBOARD_STATE_DIR=~/.envboard` 是很容易
///    写出来的形状，但 `~` 是 **shell** 的展开规则，不是路径规则 —— 环境变量里的
///    `~` 不会被 shell 展开，程序会拿到一个字面目录名 `~`，本仓真的踩过：
///    工作区里躺着一个 `~` 目录）；
/// 3. **相对路径** → 相对当前工作目录补全（否则"相对谁"取决于进程 cwd，
///    换个启动方式就指到别处）。
///
/// 展开 `~` 时用**用户主目录**（`USERPROFILE` / `HOME`），而不是当前目录。
pub fn normalize_state_dir(raw: PathBuf) -> Result<PathBuf, Error> {
    if raw.as_os_str().is_empty() {
        return Err(Error::invalid_config(
            "state_dir",
            "state dir must not be empty",
        ));
    }

    // `~` 与 `~/…`：按路径**成分**判定（不是整串比较），`~user/…` 不做展开
    // （那是别的用户的主目录，我们没有替他选目录的立场）。
    let text = raw.to_string_lossy();
    let expanded = if text == "~" {
        user_home().ok_or_else(|| {
            Error::invalid_config(
                "state_dir",
                "cannot expand '~': no user home directory is known",
            )
        })?
    } else if let Some(rest) = text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\")) {
        user_home()
            .ok_or_else(|| {
                Error::invalid_config(
                    "state_dir",
                    "cannot expand '~': no user home directory is known",
                )
            })?
            .join(rest)
    } else {
        raw
    };

    // 相对路径补全：绝对路径原样保留（不动 `C:\…` 与 `/…`）。
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .map_err(|error| {
                Error::invalid_config(
                    "state_dir",
                    format!("cannot resolve a relative state dir: {error}"),
                )
            })?
            .join(expanded)
    };

    // 词法归一化：去掉 `.`、把 `..` 折叠掉。
    //
    // 为什么不是 `canonicalize()`：① 它要求路径已经存在（状态目录首次创建前不存在）；
    // ② Windows 上它返回 `\\?\C:\…` 前缀，那个前缀会漏进设置屏与日志；③ 它解引用
    // 符号链接，把"用户选的位置"变成"链接指向的位置"，与用户意图不符。
    //
    // 不折叠的副作用才是重点：`C:\work\app\..\..\state` 会被原样显示成一串 `..`，
    // 读起来像"应用把状态写进了源码树"，而它其实指向别处 —— 路径要么归一，要么别显示。
    let absolute = lexically_normalize(&absolute);

    // 去掉尾部分隔符（`C:\state\` 与 `C:\state` 指的是同一个目录，但显示出来
    // 两份，且拼接时会多出一层空路径成分）。
    Ok(trim_trailing_separator(absolute))
}

/// 词法归一化：去掉 `.`、折叠 `..`，**不碰文件系统**。
///
/// 只在"上一段是普通名字"时折叠：根与盘符后面的 `..` 原样保留 —— 否则
/// `C:\..` 会被折成 `C:\`（跨盘语义），`\\server\share\..` 会被折到共享名之外。
fn lexically_normalize(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let can_pop = matches!(out.components().next_back(), Some(Component::Normal(_)));
                if can_pop {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// 用户主目录（`USERPROFILE` / `HOME`）。
///
/// Windows 上 `HOME` 可能不存在（原生进程），`USERPROFILE` 才是约定；
/// Unix 上反过来。两边都试，取到的第一个算数。
pub fn user_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

/// 去掉路径尾部的分隔符（根目录本身除外）。
///
/// **只在路径是合法 UTF-8 时才动它**：非 UTF-8 的路径字节一旦经过
/// `to_string_lossy` 往返就会**被替换成 `U+FFFD`** —— 路径变了，指向别处。
/// 那样的路径原样返回（多一个尾部分隔符不影响正确性，而指错地方影响很大）。
///
/// 根目录（`C:\` 与 `/`）不会被削成 `C:` —— `C:` 是"当前盘"的相对路径，
/// 语义与 `C:\` 完全不同。
fn trim_trailing_separator(path: PathBuf) -> PathBuf {
    let Some(text) = path.to_str() else {
        // 非 UTF-8 路径：不动它。
        return path;
    };
    let mut text = text.to_owned();
    // Windows 上 `\` 才是分隔符；Unix 上 `\` 是合法文件名成分，不能当分隔符。
    let is_separator = |tail: &str| tail.ends_with('/') || (cfg!(windows) && tail.ends_with('\\'));

    // 至少留 1 个字符（根目录 `/`），以及 Windows 上 `C:\` 的 3 个字符
    // （`C:` 是当前盘，不是根）。
    let floor = if cfg!(windows) && text.len() >= 3 && text.as_bytes()[1] == b':' {
        3
    } else {
        1
    };
    while text.len() > floor && is_separator(&text[text.len() - 1..]) {
        text.pop();
    }
    PathBuf::from(text)
}

#[cfg(test)]
mod tests {
    use super::{
        Platform, default_state_dir_for, normalize_state_dir, platform_default_state_dir, user_home,
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn state_dir_is_normalized_lexically() {
        // `..` 不该出现在任何一个会被展示的路径里：它会让"状态目录在源码树里"
        // 这种误读成立，而实际位置在别处。
        let base = std::env::temp_dir();
        let raw = base.join("demo").join("..").join("envboard-norm-test");
        let normalized = normalize_state_dir(raw).unwrap();
        assert!(
            !normalized.to_string_lossy().contains(".."),
            "归一化后不该再有 `..`：{normalized:?}"
        );
        assert_eq!(normalized, base.join("envboard-norm-test"));
    }

    #[test]
    fn normalization_never_folds_above_the_root() {
        // 根/盘符上的 `..` 折不得（`C:\..` 折成 `C:\` 会改变语义），原样保留。
        let normalized = super::lexically_normalize(Path::new(r"C:\..\x"));
        assert!(
            normalized.to_string_lossy().contains(".."),
            "盘符上的 `..` 必须保留：{normalized:?}"
        );
    }

    #[test]
    fn no_log_file_beats_an_explicit_log_dir() {
        let options = super::HostOptions {
            state_dir: Some(PathBuf::from("/tmp/envboard-options-test")),
            log_dir: Some(PathBuf::from("/tmp/envboard-options-logs")),
            no_log_file: true,
            ..Default::default()
        };
        assert!(
            options.into_config().unwrap().log_dir.is_none(),
            "`--no-log-file` 表达的就是不要文件，必须压过 --log-dir"
        );
    }

    #[test]
    fn no_log_file_also_stops_the_trajectory_from_being_written() {
        // 轨迹跟"要不要落盘"走（不跟"落到哪"走）：不落盘时它必须一起关。
        let options = super::HostOptions {
            state_dir: Some(PathBuf::from("/tmp/envboard-options-test")),
            no_log_file: true,
            ..Default::default()
        };
        let config = options.into_config().unwrap();
        assert!(
            config.trajectory_file("beta").is_none(),
            "不落盘时轨迹不该有路径"
        );
    }

    #[test]
    fn the_trajectory_lives_under_the_state_dir_not_under_the_log_dir() {
        // 轨迹是数据面事件流，不是实例日志：改日志目录不该把它搬走。
        //
        // 用 `temp_dir()` 而不是写死 `/tmp/…`：后者在 Windows 上不是绝对路径，
        // 而规范化会把相对路径补成绝对（`C:/tmp/…`），于是 `starts_with` 的
        // 基准和结果对不上 —— 断言必须建立在**规范化之后**的同一个基准上。
        let state_dir = std::env::temp_dir().join("envboard-options-test");
        let options = super::HostOptions {
            state_dir: Some(state_dir.clone()),
            log_dir: Some(std::env::temp_dir().join("envboard-options-logs")),
            ..Default::default()
        };
        let config = options.into_config().unwrap();
        let path = config.trajectory_file("beta").expect("默认要落盘");
        // 用**成分**比对，不比字符串：Windows 上分隔符是 `\`。
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("beta.jsonl"),
            "轨迹文件名"
        );
        assert_eq!(
            path.parent()
                .and_then(|dir| dir.file_name())
                .and_then(|name| name.to_str()),
            Some("trajectories"),
            "轨迹必须待在状态目录的 trajectories 下，而不是日志目录下：{path:?}"
        );
        assert!(
            path.starts_with(&state_dir),
            "轨迹必须在状态目录之内：{path:?} 不在 {state_dir:?} 里"
        );
    }

    #[test]
    fn an_empty_state_dir_is_rejected() {
        let error = normalize_state_dir(PathBuf::from("")).expect_err("空路径不是默认");
        assert_eq!(error.field.as_deref(), Some("state_dir"), "got {error:?}");
    }

    #[test]
    fn a_tilde_is_expanded_to_the_user_home() {
        // `~` 是 shell 的展开规则，不是路径规则：直接传进程序会得到一个字面
        // 目录名 `~`（本仓真的在仓库根躺过一个 `~` 目录）。
        let Some(home) = user_home() else {
            return; // 这台机器没有已知的主目录 —— 无从断言，跳过。
        };
        assert_eq!(normalize_state_dir(PathBuf::from("~")).unwrap(), home);
        assert_eq!(
            normalize_state_dir(PathBuf::from("~/.envboard")).unwrap(),
            home.join(".envboard")
        );
    }

    #[test]
    fn a_relative_state_dir_becomes_absolute() {
        // 否则"相对谁"取决于进程 cwd：换个启动方式就指到别处。
        let resolved = normalize_state_dir(PathBuf::from("scratch-state")).unwrap();
        assert!(resolved.is_absolute(), "相对路径必须补成绝对：{resolved:?}");
        assert!(resolved.ends_with("scratch-state"), "{resolved:?}");
    }

    #[test]
    fn an_absolute_state_dir_is_kept_as_is() {
        // 用 `std::env::temp_dir()` 造一个**本平台**的绝对路径：写死 `/tmp/…`
        // 在 Windows 上不是绝对路径（`\tmp\…` 落在当前盘），断言会失去意义。
        let raw = std::env::temp_dir().join("envboard-absolute");
        assert!(raw.is_absolute());
        assert_eq!(normalize_state_dir(raw.clone()).unwrap(), raw);
    }

    #[test]
    fn a_trailing_separator_is_trimmed() {
        // `C:\state\` 与 `C:\state` 是同一个目录，显示成两份就是"两份真相"。
        let base = std::env::temp_dir().join("envboard-state");
        let trimmed = normalize_state_dir(base.join("")).unwrap();
        assert_eq!(trimmed, base, "尾部分隔符必须去掉");
    }

    #[test]
    fn a_root_directory_never_loses_its_separator() {
        // `C:\` 削成 `C:` 就从"根目录"变成了"当前盘的相对路径" —— 语义不同，
        // 而用户给的就是根目录。`/` 同理。
        let root = if cfg!(windows) {
            PathBuf::from("C:\\")
        } else {
            PathBuf::from("/")
        };
        let trimmed = normalize_state_dir(root.clone()).unwrap();
        assert_eq!(trimmed, root, "根目录的分隔符不能被削掉");
        assert!(trimmed.is_absolute(), "根目录必须仍是绝对路径：{trimmed:?}");
    }

    #[test]
    fn the_platform_default_state_dir_is_absolute_or_absent() {
        // 回退链第 4 级要么给出一个**平台约定**下的绝对目录，要么干脆没有 ——
        // 不能给出一个相对路径（那就又变成"相对 cwd"了）。
        if let Some(dir) = platform_default_state_dir(APP) {
            assert!(dir.is_absolute(), "默认状态目录必须是绝对路径：{dir:?}");
            assert!(
                dir.ends_with(APP),
                "默认状态目录名必须就是传进来的应用名：{dir:?}"
            );
        }
    }

    #[test]
    fn a_portable_marker_next_to_the_exe_moves_the_state_dir_next_to_it() {
        // 绿色包的约定：标记在，数据落在 exe 同目录的 `<应用名>-data`。
        // 目的一并钉住两件事：①判据只看标记，不看目录是否存在（首次运行前它不存在）；
        // ②数据目录名由应用名推出来，不写死产品名。
        //
        // 夹具建在**测试二进制自己所在目录**（即 `target/…/deps/`）下的子目录里，
        // 不建在 `temp_dir()`：Windows 上 `temp_dir()` 读 `TMP`/`TEMP`，而 Git Bash
        // 传下来的是 `/tmp`（Windows 解释成 `C:\tmp`，写它要管理员权限），于是这条
        // 用例在 bash 里跑会得到 `拒绝访问`。`target/` 本就随构建再生、不入库。
        let exe = std::env::current_exe().expect("test binary path");
        let dir = exe
            .parent()
            .expect("test binary has a parent")
            .join(format!("envboard-portable-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("fixture dir");
        let exe = dir.join("envboard.exe");

        assert_eq!(
            super::portable_state_dir(&exe, "envboard"),
            None,
            "没有标记文件就不是便携模式"
        );

        std::fs::write(
            dir.join(format!("envboard{}", super::PORTABLE_MARKER_SUFFIX)),
            b"",
        )
        .unwrap();
        assert_eq!(
            super::portable_state_dir(&exe, "envboard"),
            Some(dir.join(format!("envboard{}", super::PORTABLE_DATA_SUFFIX))),
            "标记在，数据目录必须与 exe 同级"
        );
        assert_eq!(
            super::portable_state_dir(&exe, "other"),
            None,
            "别的应用的标记不该让本应用变成便携模式"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn without_an_app_name_there_is_no_platform_default() {
        // 名字是宿主的输入（来自它的 Cargo.toml）。没给名字时宁可没有默认目录，
        // 也不要凭空编一个 —— 编错的后果是账本写进 `%LOCALAPPDATA%\unknown`，
        // 症状是"环境列表空了"。
        let options = super::HostOptions::default();
        let error = options
            .into_config()
            .expect_err("没有名字就没有平台默认目录");
        assert_eq!(error.field.as_deref(), Some("state_dir"), "{error:?}");
    }

    #[test]
    fn an_explicit_state_dir_works_without_an_app_name() {
        // 显式给了目录就不需要名字 —— 名字只服务"平台默认"那一级。
        let options = super::HostOptions {
            state_dir: Some(std::env::temp_dir().join("envboard-no-name")),
            no_log_file: true,
            ..Default::default()
        };
        assert!(options.into_config().is_ok());
    }

    // ---- 三平台规则：在**任意宿主**上逐平台验证 ----
    //
    // 拆出纯函数 `default_state_dir_for` 就是为了这几个用例：本仓没有 Mac 构建机，
    // 而 macOS 那条分支写错的表现是"起了个空工作台"（账本在另一个目录），
    // 从界面上完全看不出来。

    /// 测试用的应用目录名。生产代码里它来自宿主二进制的 `Cargo.toml`
    /// （`env!("CARGO_BIN_NAME")`）；这里写死一份只为断言可读。
    const APP: &str = "envboard";

    #[test]
    fn each_platform_lands_in_its_own_convention() {
        use std::ffi::OsStr;
        let l = OsStr::new("/base/local-app-data");
        let h = OsStr::new("/home/alice");
        let x = OsStr::new("/home/alice/.local/state");

        assert_eq!(
            default_state_dir_for(Platform::Windows, APP, Some(l), Some(h), Some(x)),
            Some(PathBuf::from("/base/local-app-data/envboard")),
            "Windows 只剩 %LOCALAPPDATA% 这一处（%APPDATA% 不行：状态不该漫游）"
        );
        assert_eq!(
            default_state_dir_for(Platform::MacOs, APP, Some(l), Some(h), Some(x)),
            Some(PathBuf::from(
                "/home/alice/Library/Application Support/envboard"
            )),
            "macOS 必须走 Application Support —— 那边没有 state 档"
        );
        assert_eq!(
            default_state_dir_for(Platform::Linux, APP, Some(l), Some(h), Some(x)),
            Some(PathBuf::from("/home/alice/.local/state/envboard")),
            "Linux 必须尊重 $XDG_STATE_HOME"
        );
        assert_eq!(
            default_state_dir_for(Platform::Linux, APP, Some(l), Some(h), None),
            Some(PathBuf::from("/home/alice/.local/state/envboard")),
            "XDG 未设时回落到规范定义的 $HOME/.local/state"
        );
    }

    #[test]
    fn no_platform_puts_the_app_dir_at_the_home_root() {
        // 主目录根禁止放应用目录（Windows 有明文，macOS/Linux 同理）。
        // 这条用例专门防"顺手改回 ~/.envboard"。
        use std::ffi::OsStr;
        use std::path::Path;
        let home = Some(OsStr::new("/home/alice"));
        let home_root = Path::new("/home/alice");
        let base = Some(OsStr::new("/base"));

        let cases = [
            (Platform::Windows, base, home, None),
            (Platform::MacOs, None, home, None),
            // Linux 走**回落**那条才是有意义的检查：没有 $XDG_STATE_HOME 时
            // 最容易顺手写成 `$HOME/.envboard`。
            (Platform::Linux, None, home, None),
        ];
        for (platform, local, home, xdg) in cases {
            let dir = default_state_dir_for(platform, APP, local, home, xdg).unwrap();
            assert_ne!(
                dir.parent(),
                Some(home_root),
                "{platform:?} 把应用目录放到了主目录根：{dir:?}"
            );
        }
    }

    #[test]
    fn an_empty_platform_variable_counts_as_unset() {
        // XDG 对每个变量都写明"未设置**或为空**"都用默认值。当成未设置往下回退，
        // 否则 join 出来是 "envboard" 这样的相对路径 —— 状态会写到进程 cwd。
        use std::ffi::OsStr;
        let h = OsStr::new("/home/alice");
        let empty = OsStr::new("");

        assert_eq!(
            default_state_dir_for(Platform::Linux, APP, None, Some(h), Some(empty)),
            Some(PathBuf::from("/home/alice/.local/state/envboard"))
        );
        assert_eq!(
            default_state_dir_for(Platform::Windows, APP, Some(empty), None, None),
            None,
            "LOCALAPPDATA 为空时不能拼出相对路径"
        );
        assert_eq!(
            default_state_dir_for(Platform::MacOs, APP, None, Some(empty), None),
            None
        );
    }

    #[test]
    fn a_relative_xdg_state_home_is_ignored() {
        // XDG 原文：相对路径 "should consider the path invalid and ignore it"。
        // 照用的话"相对谁"取决于进程 cwd，正是规范要避免的。
        use std::ffi::OsStr;
        let h = OsStr::new("/home/alice");
        assert_eq!(
            default_state_dir_for(
                Platform::Linux,
                APP,
                None,
                Some(h),
                Some(OsStr::new("relative/state"))
            ),
            Some(PathBuf::from("/home/alice/.local/state/envboard")),
            "相对路径的 $XDG_STATE_HOME 必须被忽略并回落"
        );
    }

    #[test]
    fn an_empty_app_name_yields_no_default() {
        // 空名字会 `join` 出基础目录本身 —— 账本直接铺在 `%LOCALAPPDATA%` 里。
        use std::ffi::OsStr;
        let h = OsStr::new("/home/alice");
        assert_eq!(
            default_state_dir_for(Platform::Linux, "", None, Some(h), None),
            None
        );
        assert_eq!(
            default_state_dir_for(Platform::Windows, "", Some(h), Some(h), None),
            None
        );
    }
}

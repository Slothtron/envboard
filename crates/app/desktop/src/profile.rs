//! 正式版 / 便携版 / 预览版 / 开发版的**身份隔离**（ADR-014）。
//!
//! 问题不是编译产物放在哪，而是几个形态共用同一套机器级资源：
//!
//! * 状态目录（`%LOCALAPPDATA%\envboard`）：账本、规则库、共享 CA、`InstanceLock`；
//! * 二次启动激活事件：名字由 state-dir 哈希而来，同目录 = 同事件；
//! * WebView2 用户数据目录：Tauri 用 `app_local_data_dir()`，即 `%LOCALAPPDATA%\<identifier>`。
//!
//! 三样都按「正式版」定，于是换个形态就会互相踩：撞锁 / 把正在用的正式版拉到前台
//! （ADR-012 的激活事件）/ 用同一份账本改掉正式版的数据。
//!
//! ## 标签从哪来（优先级从高到低）
//!
//! 1. `$ENVBOARD_PROFILE` —— 显式覆盖；`release` / `none` 是「要正式版身份」的写法；
//! 2. **版本号的预发布段** —— `0.2.0-preview.1` → `preview`。这是预览渠道的判据：
//!    同一颗二进制装到哪、拷到哪都自带身份，不依赖构建参数或运行时环境变量；
//! 3. **便携标记** —— exe 同目录有 `envboard.portable` → `portable`。便携副本的数据
//!    本来就落在 exe 旁边（`envboard-data`），这里再把 WebView2 profile、标题与端口段
//!    一起切开，多个便携副本/一个便携副本 + 一个安装版可以同时开；
//! 4. debug 构建 → `dev`；release 且以上都没有 → **无标签**（正式版）。
//!
//! ## 标签落在哪
//!
//! * 平台默认状态目录 / 便携数据目录的名字：`envboard-preview`、`envboard-dev`
//!   （**便携版不改名**：数据位置由标记决定，仍是 `<exe 目录>/envboard-data`）；
//! * WebView2 用户数据目录：`com.envboard.desktop-<tag>`；
//! * 默认端口段 `17000-17999`（正式版是引擎默认 `16000-16999`）。账本各自独立，
//!   分配互相看不见，不换段就会「分配成功、bind 失败」；
//! * 窗口标题 / 托盘 tooltip / 退出菜单项后缀「（preview）」「（portable）」「（dev）」。
//!
//! 显式 `--state-dir` / `$ENVBOARD_STATE_DIR` 仍然压过一切（回退链第 1、2 级）：
//! 临时实验不需要动标签。

use std::path::PathBuf;

use envboard_host::options::PORTABLE_MARKER_SUFFIX;
use tauri::{AppHandle, Manager};

/// 覆盖标签的环境变量。
pub const PROFILE_ENV: &str = "ENVBOARD_PROFILE";

/// 非正式版共用的默认端口段（正式版是 `16000-16999`，见
/// `envboard_engine::domain::DEFAULT_PORT_RANGE`）。
pub const NON_STABLE_PORT_RANGE: (u16, u16) = (17_000, 17_999);

/// 便携副本的标签（由 exe 同目录的标记文件判定）。
pub const PORTABLE_TAG: &str = "portable";

/// debug 构建的标签。
pub const DEV_TAG: &str = "dev";

/// 当前进程的 profile。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    tag: Option<String>,
    /// exe 同目录是否有 `envboard.portable`（决定数据目录是否跟着 exe 走）。
    portable: bool,
}

impl Profile {
    /// 解析当前进程的身份。`base` 是产品名（`[[bin]] name`），便携标记按它查。
    pub fn current(base: &str) -> Self {
        let raw = std::env::var_os(PROFILE_ENV).map(|value| value.to_string_lossy().into_owned());
        let portable = portable_marker_present(base);
        Self {
            tag: resolve_tag(
                raw.as_deref(),
                channel_tag(env!("CARGO_PKG_VERSION")),
                portable,
                cfg!(debug_assertions),
            ),
            portable,
        }
    }

    /// 正式版身份（无标签）。只给「拿不到已解析 Profile」的兜底用 —— 正常路径上
    /// 每个进程只有一个 Profile，存在 `Shell` 里。
    pub fn stable() -> Self {
        Self {
            tag: None,
            portable: false,
        }
    }

    #[cfg(test)]
    pub fn with_tag(tag: Option<&str>) -> Self {
        Self {
            tag: tag.map(str::to_owned),
            portable: false,
        }
    }

    #[cfg(test)]
    pub fn with_parts(tag: Option<&str>, portable: bool) -> Self {
        Self {
            tag: tag.map(str::to_owned),
            portable,
        }
    }

    pub fn tag(&self) -> Option<&str> {
        self.tag.as_deref()
    }

    /// 是否不是正式版（决定默认端口段）。
    pub fn is_non_stable(&self) -> bool {
        self.tag.is_some()
    }

    /// 平台默认目录 / 便携模式里的应用目录名。
    ///
    /// 便携副本**沿用原名**：它的数据位置由 `envboard.portable` 标记决定
    /// （`<exe 目录>/envboard-data`），改名反而会让标记查找与自己打架——预览版
    /// 便携运行（`-preview.N` + 标记）同样落在 `<exe 目录>/envboard-data`。
    pub fn app_dir_name(&self, base: &str) -> String {
        if self.portable {
            return base.to_owned();
        }
        match self.tag.as_deref() {
            Some(tag) => format!("{base}-{tag}"),
            None => base.to_owned(),
        }
    }

    /// 给人看的名字（窗口标题、托盘 tooltip）：正式版保持原文，其余加后缀。
    pub fn display_name(&self, base: &str) -> String {
        match &self.tag {
            Some(tag) => format!("{base}（{tag}）"),
            None => base.to_owned(),
        }
    }

    /// WebView2 用户数据目录。`None` = 用 Tauri 默认（`app_local_data_dir`）。
    ///
    /// 每个形态必须各用一份：user data folder 带浏览器进程与缓存，共享会互相踩
    /// （同一目录被两个不同 args 的环境打开还会直接失败）。
    pub fn webview_data_dir(&self, app: &AppHandle) -> Option<PathBuf> {
        let tag = self.tag.as_ref()?;
        let base = app.path().app_local_data_dir().ok()?;
        let name = base.file_name()?.to_string_lossy().into_owned();
        Some(base.with_file_name(format!("{name}-{tag}")))
    }
}

/// 标签判定（纯函数，便于测试）。
///
/// 显式变量压过渠道与便携：`release` / `none` 是「显式要正式版身份」的两种写法；
/// 其余值规整成目录安全片段，规整后为空则退回默认判定。空值等同未设置
/// （与 `$ENVBOARD_STATE_DIR` 的口径一致）。
fn resolve_tag(
    raw: Option<&str>,
    channel: Option<String>,
    portable: bool,
    debug_build: bool,
) -> Option<String> {
    match raw.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value)
            if value.eq_ignore_ascii_case("release") || value.eq_ignore_ascii_case("none") =>
        {
            None
        }
        Some(value) => sanitize_tag(value).or_else(|| default_tag(channel, portable, debug_build)),
        None => default_tag(channel, portable, debug_build),
    }
}

fn default_tag(channel: Option<String>, portable: bool, debug_build: bool) -> Option<String> {
    channel
        .or_else(|| portable.then(|| PORTABLE_TAG.to_owned()))
        .or_else(|| debug_build.then(|| DEV_TAG.to_owned()))
}

/// 版本号预发布段 → 渠道标签：`0.2.0-preview.1` → `preview`，`0.2.0` → `None`。
///
/// 只取预发布段的第一个标识符（`-rc.2` → `rc`）；`+build` 元数据不参与。
fn channel_tag(version: &str) -> Option<String> {
    let core = version.split('+').next().unwrap_or(version);
    let (_, pre) = core.split_once('-')?;
    sanitize_tag(pre.split('.').next()?)
}

/// 便携标记是否与 exe 同目录（规则与组合根 `portable_state_dir` 一致）。
fn portable_marker_present(base: &str) -> bool {
    if base.is_empty() {
        return false;
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.parent()
                .map(|dir| dir.join(format!("{base}{PORTABLE_MARKER_SUFFIX}")))
        })
        .is_some_and(|marker| marker.exists())
}

/// 规整成目录安全的小写片段：非法字符折成 `-`，首尾 `-` 去掉；空结果 = 没有标签。
fn sanitize_tag(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    let mut last_dash = false;
    for ch in raw.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_builds_default_to_the_dev_tag() {
        assert_eq!(resolve_tag(None, None, false, true).as_deref(), Some("dev"));
        assert_eq!(
            resolve_tag(Some("   "), None, false, true).as_deref(),
            Some("dev")
        );
    }

    #[test]
    fn release_builds_have_no_tag() {
        assert_eq!(resolve_tag(None, None, false, false), None);
        assert_eq!(resolve_tag(Some(""), None, false, false), None);
    }

    #[test]
    fn explicit_release_words_drop_the_tag_even_in_debug() {
        for raw in ["release", "Release", "none", "NONE"] {
            assert_eq!(resolve_tag(Some(raw), None, false, true), None, "raw={raw}");
        }
    }

    #[test]
    fn explicit_tags_are_kept_but_made_directory_safe() {
        assert_eq!(
            resolve_tag(Some("Alice"), None, false, true).as_deref(),
            Some("alice")
        );
        assert_eq!(
            resolve_tag(Some("feat/x y"), None, false, true).as_deref(),
            Some("feat-x-y")
        );
        // 全是非法字符：没有可用标签，退回默认判定。
        assert_eq!(
            resolve_tag(Some("///"), None, false, true).as_deref(),
            Some("dev")
        );
    }

    #[test]
    fn prerelease_versions_carry_their_channel() {
        assert_eq!(channel_tag("0.1.0"), None);
        assert_eq!(channel_tag("0.2.0-preview.1").as_deref(), Some("preview"));
        assert_eq!(channel_tag("0.2.0-Preview.10").as_deref(), Some("preview"));
        assert_eq!(channel_tag("0.2.0-rc.2").as_deref(), Some("rc"));
        // build 元数据不参与。
        assert_eq!(channel_tag("0.2.0-beta.1+abc").as_deref(), Some("beta"));
        assert_eq!(channel_tag("0.2.0+build-7"), None);
    }

    #[test]
    fn channel_and_portable_outrank_the_debug_default() {
        // 渠道 > 便携 > debug。
        assert_eq!(
            resolve_tag(None, Some("preview".into()), true, true).as_deref(),
            Some("preview")
        );
        assert_eq!(
            resolve_tag(None, None, true, true).as_deref(),
            Some("portable")
        );
        // 显式覆盖仍然最优先。
        assert_eq!(
            resolve_tag(Some("alice"), Some("preview".into()), true, true).as_deref(),
            Some("alice")
        );
        assert_eq!(
            resolve_tag(Some("release"), Some("preview".into()), true, true),
            None
        );
    }

    #[test]
    fn portable_keeps_the_base_directory_name() {
        // 便携副本的数据位置由标记决定，名字不能变（否则标记查找与自己打架）——
        // 预览版便携运行也一样。
        assert_eq!(
            Profile::with_parts(Some("portable"), true).app_dir_name("envboard"),
            "envboard"
        );
        assert_eq!(
            Profile::with_parts(Some("preview"), true).app_dir_name("envboard"),
            "envboard"
        );
        assert_eq!(
            Profile::with_tag(Some("preview")).app_dir_name("envboard"),
            "envboard-preview"
        );
        assert_eq!(
            Profile::with_tag(Some("dev")).app_dir_name("envboard"),
            "envboard-dev"
        );
        assert_eq!(Profile::stable().app_dir_name("envboard"), "envboard");
    }

    #[test]
    fn names_and_titles_follow_the_tag() {
        let release = Profile::stable();
        assert_eq!(release.display_name("envboard 工作台"), "envboard 工作台");
        assert!(!release.is_non_stable());

        let preview = Profile::with_tag(Some("preview"));
        assert_eq!(
            preview.display_name("envboard 工作台"),
            "envboard 工作台（preview）"
        );
        assert!(preview.is_non_stable());
    }
}

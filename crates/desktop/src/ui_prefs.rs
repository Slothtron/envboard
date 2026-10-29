//! 界面偏好：**跨启动记住，但不进账本**。
//!
//! 主题是"这台机器上这个人喜欢怎么用"，不是环境事实 —— 所以它不进 `state.json`
//! （那是账本，由管理器单向写），而是同目录下一个自包含的小文件 `ui.json`：
//!
//! ```json
//! {"version": 1, "theme": "system"}
//! ```
//!
//! 三条纪律：
//!
//! * **坏文件不拦启动**：读不出来、字段不认识 → 回落到默认（跟随系统），
//!   而不是让界面起不来；
//! * **写失败要能看见**：设置屏把 `save` 的错误显示在选项下面（不静默丢偏好）；
//! * **只认三档**：明亮 / 暗色 / 跟随系统。`gpui-component` 的 `ThemeMode` 只有
//!   Light/Dark 两档，"跟随系统"是界面的偏好语义 —— 它由
//!   [`Theme::sync_system_appearance`] 落实，并在窗口外观变化时重新解析。

use std::path::{Path, PathBuf};

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Global, Window};

/// 偏好文件名（落在状态目录下，与 `state.json` 平级但彼此无关）。
pub const FILE_NAME: &str = "ui.json";

/// 主题偏好：界面自己的三档，比 `ThemeMode` 多一档"跟随系统"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemePreference {
    Light,
    Dark,
    #[default]
    System,
}

impl ThemePreference {
    /// 选项顺序就是设置屏下拉框的显示顺序（明亮 → 暗色 → 跟随系统）。
    pub const ALL: [Self; 3] = [Self::Light, Self::Dark, Self::System];

    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "明亮",
            Self::Dark => "暗色",
            Self::System => "跟随系统",
        }
    }

    /// 落盘用的稳定标识（不随界面文案变化）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::System => "system",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|pref| pref.as_str() == raw)
    }

    /// 设置屏下拉框的下标 ↔ 偏好（顺序由 [`Self::ALL`] 固定）。
    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|pref| *pref == self)
            .unwrap_or(Self::ALL.len() - 1)
    }

    /// 从下拉框的显示文案找回档位：选项的值就是 [`Self::label`]。
    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|pref| pref.label() == label)
    }
}

/// 界面偏好（目前只有主题；以后加窗口尺寸之类也放这里）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UiPrefs {
    pub theme: ThemePreference,
}

impl Global for UiPrefs {}

impl UiPrefs {
    /// 落盘正文。格式版本单列一个字段：将来加键时旧版本读到不认识的键会被忽略，
    /// 而不是把整份偏好判坏。
    fn body(self) -> String {
        format!(
            "{{\n  \"version\": 1,\n  \"theme\": \"{}\"\n}}\n",
            self.theme.as_str()
        )
    }

    /// 从文件正文解析；任何不认识的内容都回落到默认（跟随系统）。
    pub fn parse(raw: &str) -> Self {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
            return Self::default();
        };
        let theme = value
            .get("theme")
            .and_then(serde_json::Value::as_str)
            .and_then(ThemePreference::parse)
            .unwrap_or_default();
        Self { theme }
    }

    /// 读偏好：文件不存在 / 读失败 / 内容坏 → 默认（跟随系统）。**永不拦启动**。
    pub fn load(state_dir: &Path) -> Self {
        std::fs::read_to_string(path(state_dir))
            .map(|raw| Self::parse(&raw))
            .unwrap_or_default()
    }

    /// 写偏好：先写临时文件再改名（半截文件不会成为下次启动的输入）。
    pub fn save(self, state_dir: &Path) -> std::io::Result<()> {
        let tmp = state_dir.join(format!("{FILE_NAME}.tmp"));
        std::fs::write(&tmp, self.body())?;
        std::fs::rename(&tmp, path(state_dir))
    }

    /// 当前生效的偏好（设置屏与窗口外观观察器共用这一处）。
    pub fn current(cx: &App) -> Self {
        cx.try_global::<Self>().copied().unwrap_or_default()
    }

    /// 改成新偏好并立刻落实主题。返回**新的**偏好：调用方负责把写入失败显示出来。
    pub fn set_theme(
        self,
        theme: ThemePreference,
        window: Option<&mut Window>,
        cx: &mut App,
    ) -> Self {
        let next = Self { theme };
        // `App` 上没有 update_global（那是 AsyncApp/TestApp 的 API）：整份替换。
        // 偏好只有一个字段，替换是等价的。
        cx.set_global(next);
        apply(theme, window, cx);
        next
    }
}

fn path(state_dir: &Path) -> PathBuf {
    state_dir.join(FILE_NAME)
}

/// 把偏好落实成实际主题。
///
/// "跟随系统"走组件的 `sync_system_appearance`（它读 `window.appearance()`，
/// 没有窗口时读 `cx.window_appearance()`），所以启动早期也能调用。
pub fn apply(pref: ThemePreference, window: Option<&mut Window>, cx: &mut App) {
    match pref {
        ThemePreference::Light => Theme::change(ThemeMode::Light, window, cx),
        ThemePreference::Dark => Theme::change(ThemeMode::Dark, window, cx),
        ThemePreference::System => Theme::sync_system_appearance(window, cx),
    }
}

#[cfg(test)]
mod tests {
    use super::{ThemePreference, UiPrefs};

    #[test]
    fn the_default_is_follow_the_system() {
        // 首启不猜用户喜欢亮还是暗：交给系统设置，这是唯一不用问也知道答案的选项。
        assert_eq!(UiPrefs::default().theme, ThemePreference::System);
    }

    #[test]
    fn preferences_round_trip_through_the_file_body() {
        for theme in ThemePreference::ALL {
            let prefs = UiPrefs { theme };
            let parsed = UiPrefs::parse(&prefs.body());
            assert_eq!(parsed, prefs, "{theme:?} 写入再读回必须一致");
        }
    }

    #[test]
    fn unknown_or_broken_content_falls_back_instead_of_failing() {
        // 坏 JSON、缺键、认不出的档位都不许拦启动 —— 一律回落到默认。
        for raw in [
            "",
            "not json",
            "{}",
            "{\"theme\": \"neon\"}",
            "{\"theme\": 7}",
            "{\"version\": 99, \"theme\": \"dark\"}",
        ] {
            let parsed = UiPrefs::parse(raw);
            assert!(ThemePreference::ALL.contains(&parsed.theme), "{raw:?}");
        }
        // 认不出的档位 → 默认；认得出的档位哪怕版本号更大也照读（前向宽容）。
        assert_eq!(
            UiPrefs::parse("{\"theme\": \"neon\"}").theme,
            ThemePreference::System
        );
        assert_eq!(
            UiPrefs::parse("{\"version\": 99, \"theme\": \"dark\"}").theme,
            ThemePreference::Dark
        );
    }

    #[test]
    fn the_select_index_matches_the_declared_order() {
        // 设置屏的下拉框用下标定位当前项、用文案认领选中值；这里钉住两条映射。
        for (index, pref) in ThemePreference::ALL.into_iter().enumerate() {
            assert_eq!(pref.index(), index);
            assert_eq!(ThemePreference::from_label(pref.label()), Some(pref));
        }
    }
}

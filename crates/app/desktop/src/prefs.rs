//! 界面偏好（`<state_dir>/ui.json`）：主题等 UI 选择落盘在状态目录，
//! **不依赖 WebView 的 localStorage**。
//!
//! 为什么（ADR-012 实测）：关窗即销毁 WebView 后，WebView2 **不会把最近的
//! localStorage 写入刷盘** —— 关窗前写入的键在重开的窗口里读不到（
//! `probe-key` 实验：set 后 3s 关窗，重开读回 null）。主题偏好因此会丢。
//! 偏好由壳自己原子写进 ui.json，与窗口生命周期无关。
//!
//! 同一份文件也存窗口几何（位置 / 大小 / 最大化）：ADR-012 的窗口是「关即销毁、
//! 托盘重建」，重建走 `WindowConfig` 的默认宽高，不记住就会每次回到默认位置。
//!
//! 路径规则不在这里拼：调用方从组合根交付的 `Prepared::ui_prefs_file` 拿。

use std::path::{Path, PathBuf};

use envboard_engine::Error;
use serde::{Deserialize, Serialize};

/// 主题三档（与 frontend `theme.ts` 的 `ThemeChoice` 同值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    Light,
    Dark,
    System,
}

impl ThemeChoice {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            "system" => Some(Self::System),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::System => "system",
        }
    }
}

/// 窗口几何（**物理像素**，原因见 `geometry` 模块文档）。`x`/`y` 是外框左上角，
/// `width`/`height` 是客户区大小（与 `tauri.conf.json` 的 width/height 同义，正好
/// 对上 Tauri 的 set_position + set_size）。`maximized` 与正常态尺寸分开记：最大化
/// 时窗口矩形是屏幕矩形（Windows 还会外扩 8px），拿它当「还原尺寸」会让用户取消
/// 最大化后得到一个错位的窗口，所以壳只在非最大化时更新位置与宽高。
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowGeometry {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub maximized: bool,
    /// 单位标记：`true` = 物理像素（当前格式）。旧版本（未标记）存的是逻辑像素，
    /// 跨 DPI 无法换算回来，一律忽略 —— 宁可回默认位置，也不要半错位的窗口。
    pub physical: bool,
}

/// ui.json 的形状。`flatten` 保住未来版本写入的字段（壳只认 `theme` 与 `window`）。
#[derive(Debug, Default, Serialize, Deserialize)]
struct UiPrefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    theme: Option<ThemeChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window: Option<WindowGeometry>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

/// 界面偏好存储（读取宽容：坏文件/未知主题一律回落 `None`，永不拦启动）。
pub struct UiPrefsStore {
    path: PathBuf,
}

impl UiPrefsStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn theme(&self) -> Option<ThemeChoice> {
        self.load()?.theme
    }

    /// 读-改-写：只动 `theme`，其余字段原样保留。
    pub fn set_theme(&self, choice: ThemeChoice) -> Result<(), Error> {
        self.update(|prefs| prefs.theme = Some(choice))
    }

    pub fn window_geometry(&self) -> Option<WindowGeometry> {
        self.load()?.window
    }

    /// 读-改-写：只动 `window`（与 `theme` 互不覆盖）。
    pub fn set_window_geometry(&self, geometry: WindowGeometry) -> Result<(), Error> {
        self.update(|prefs| prefs.window = Some(geometry))
    }

    fn load(&self) -> Option<UiPrefs> {
        let raw = std::fs::read_to_string(&self.path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    fn update(&self, apply: impl FnOnce(&mut UiPrefs)) -> Result<(), Error> {
        let mut prefs = self.load().unwrap_or_default();
        apply(&mut prefs);
        self.write(&prefs)
    }

    fn write(&self, prefs: &UiPrefs) -> Result<(), Error> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| Error::io_at("create the state directory", parent, error))?;
        }
        let raw = serde_json::to_vec_pretty(prefs)
            .map_err(|error| Error::internal_error(error.to_string()))?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, raw)
            .map_err(|error| Error::io_at("write the ui prefs file", &tmp, error))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &self.path)
            .map_err(|error| Error::io_at("commit the ui prefs file", &self.path, error))
    }
}

/// 便于测试/诊断：路径只读。
impl UiPrefsStore {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("envboard-prefs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_file_reads_none_and_set_round_trips() {
        let dir = temp_dir("roundtrip");
        let store = UiPrefsStore::new(dir.join("ui.json"));
        assert_eq!(store.theme(), None);
        store.set_theme(ThemeChoice::Light).unwrap();
        assert_eq!(store.theme(), Some(ThemeChoice::Light));
        store.set_theme(ThemeChoice::System).unwrap();
        assert_eq!(store.theme(), Some(ThemeChoice::System));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_or_unknown_theme_falls_back_to_none() {
        let dir = temp_dir("corrupt");
        let path = dir.join("ui.json");
        std::fs::write(&path, "{ not json").unwrap();
        let store = UiPrefsStore::new(&path);
        assert_eq!(store.theme(), None);
        std::fs::write(&path, r#"{"theme":"neon"}"#).unwrap();
        assert_eq!(store.theme(), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn set_theme_preserves_unknown_fields() {
        let dir = temp_dir("extra");
        let path = dir.join("ui.json");
        std::fs::write(&path, r#"{"theme":"dark","future":{"a":1}}"#).unwrap();
        let store = UiPrefsStore::new(&path);
        store.set_theme(ThemeChoice::Light).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["theme"], "light");
        assert_eq!(value["future"]["a"], 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn window_geometry_and_theme_do_not_clobber_each_other() {
        let dir = temp_dir("geometry");
        let path = dir.join("ui.json");
        let store = UiPrefsStore::new(&path);
        assert_eq!(store.window_geometry(), None);

        let geometry = WindowGeometry {
            x: 120.0,
            y: 64.5,
            width: 1440.0,
            height: 900.0,
            maximized: false,
            physical: true,
        };
        store.set_window_geometry(geometry).unwrap();
        store.set_theme(ThemeChoice::Dark).unwrap();

        assert_eq!(store.window_geometry(), Some(geometry));
        assert_eq!(store.theme(), Some(ThemeChoice::Dark));

        // 最大化只改标志位，不动正常态尺寸。
        store
            .set_window_geometry(WindowGeometry {
                maximized: true,
                ..geometry
            })
            .unwrap();
        assert_eq!(
            store.window_geometry(),
            Some(WindowGeometry {
                maximized: true,
                ..geometry
            })
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn partial_window_object_falls_back_to_defaults() {
        let dir = temp_dir("partial");
        let path = dir.join("ui.json");
        std::fs::write(&path, r#"{"window":{"width":1024}}"#).unwrap();
        let store = UiPrefsStore::new(&path);
        let geometry = store.window_geometry().expect("window object parses");
        assert_eq!(geometry.width, 1024.0);
        assert_eq!(geometry.height, 0.0);
        assert!(!geometry.maximized);
        // 旧版（逻辑像素）记录没有 physical 标记 —— 壳会整条忽略。
        assert!(!geometry.physical);
        std::fs::remove_dir_all(&dir).ok();
    }
}

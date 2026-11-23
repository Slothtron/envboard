//! 界面偏好（`<state_dir>/ui.json`）：主题三档落盘在**状态目录**，不跟窗口走。
//!
//! 为什么在这里再写一份而不是复用桌面形态的 `prefs`：`crates/app/desktop` 迁移期间
//! 冻结（D-6），nsh 也不能反向依赖它。文件形状与桌面形态**逐键一致**
//! （`{"theme":"light|dark|system"}`，`flatten` 保住其它字段），所以两套界面读写的
//! 是同一份文件、同一个值 —— 换框架不换用户的偏好，用户也不会在两版之间互相覆盖掉
//! 对方写的键。
//!
//! 路径规则不在这里拼：`Prepared::ui_prefs_file` 由组合根交付（只有一份真值）。
//!
//! 读一律宽容：坏文件、未知值都回落 `None`，永不拦启动；写是原子替换
//! （临时文件 + rename），半路崩掉不会留下半个 JSON。

use std::path::{Path, PathBuf};

/// 主题三档（`ui.json` 的 `theme` 值，与桌面形态的 `ThemeChoice` 同值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    /// 界面上那三档的名字（按钮与标题栏都用它，别处不再各写一份）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "宣纸",
            Self::Dark => "夜黛",
            Self::System => "跟随系统",
        }
    }

    /// 落到 NSH 的外观开关（`set_color_scheme`）。
    pub fn appearance(self) -> nsh_core::platform::Appearance {
        match self {
            Self::Light => nsh_core::platform::Appearance::Light,
            Self::Dark => nsh_core::platform::Appearance::Dark,
            Self::System => nsh_core::platform::Appearance::System,
        }
    }
}

/// 读一条偏好（只认 `theme`）。文件不在 / 坏 / 值不认识 → `None`。
pub fn theme(path: &Path) -> Option<ThemeChoice> {
    let raw = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let theme = value.get("theme")?.as_str()?;
    ThemeChoice::parse(theme)
}

/// 写 `theme` 并保住文件里其余的键（`window` 等由桌面形态写，不许被这里抹掉）。
pub fn set_theme(path: &Path, choice: ThemeChoice) -> std::io::Result<()> {
    let mut object = std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|value| match value {
            serde_json::Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default();
    object.insert(
        "theme".to_owned(),
        serde_json::Value::String(choice.as_str().to_owned()),
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_vec_pretty(&serde_json::Value::Object(object))
        .map_err(std::io::Error::other)?;
    let tmp: PathBuf = path.with_extension("json.tmp");
    std::fs::write(&tmp, raw)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("eb-m4-prefs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_or_broken_file_reads_none() {
        let dir = temp_dir("missing");
        assert_eq!(theme(&dir.join("ui.json")), None);
        std::fs::write(dir.join("ui.json"), "{ not json").unwrap();
        assert_eq!(theme(&dir.join("ui.json")), None);
        std::fs::write(dir.join("ui.json"), r#"{"theme":"sepia"}"#).unwrap();
        assert_eq!(theme(&dir.join("ui.json")), None);
    }

    #[test]
    fn writing_keeps_the_other_keys() {
        let dir = temp_dir("keep");
        let path = dir.join("ui.json");
        std::fs::write(
            &path,
            r#"{"theme":"light","window":{"x":10.0,"y":20.0,"width":800.0,"height":600.0,"maximized":false,"physical":true}}"#,
        )
        .unwrap();
        set_theme(&path, ThemeChoice::Dark).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert_eq!(theme(&path), Some(ThemeChoice::Dark));
        assert!(raw.contains("\"window\""), "写入时把别的键丢了：{raw}");
        assert!(!dir.join("ui.json.tmp").exists(), "临时文件没清掉");
    }
}

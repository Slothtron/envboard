//! 品牌只有一个源：这三处显示（自绘标题栏、窗口标题、任务栏/Alt-Tab）都从这里取。
//!
//! 图标沿用 Tauri 2 侧的同一枚（`crates/app/desktop/icons/icon.png`：深色圆角方、
//! 绿色 `>` 加青色光标），不另画一枚——换框架不该换品牌。它在这里有两种形态：
//!
//! - `assets/envboard.ico`：9 个尺寸 16–256 的 PNG-in-ICO，喂 `AppOptions.icon`；
//! - `assets/brand-mark-32.rgba`：32×32 直通 RGBA，标题栏 16pt 记号直接喂
//!   `ImageSrc::rgba`，免得运行时解 PNG。
//!
//! 两者都由 `tools/envboard-brand/pack-icon.ps1` 从原图重出；换图标时两个文件一起换。
//! 开发期从 crate 目录读（`CARGO_MANIFEST_DIR`），打包期放在 exe 旁。

use std::path::PathBuf;

/// 品牌名：小写、无空格，标题栏与任务栏都用它。
pub const NAME: &str = "envboard";

/// 中文品类词：标题栏里跟在品牌名后面。
pub const TAGLINE: &str = "工作台 · 代理";

/// 窗口标题（任务栏悬浮与辅助技术读到的就是这句）。
pub const WINDOW_TITLE: &str = "envboard 代理工作台";

/// AppUserModelID：任务栏分组、通知归属都靠它；改它等于改名。
pub const APP_ID: &str = "dev.myclaw.envboard";

const ICON_FILE: &str = "envboard.ico";

/// 标题栏记号的边长（像素）：16pt 在 125%／150% 缩放下都是 2× 上采样。
pub const MARK_PX: u32 = 32;

/// 标题栏记号的像素：32×32 直通 RGBA（非预乘、每行无填充）。
pub const MARK_RGBA: &[u8] = include_bytes!("../assets/brand-mark-32.rgba");

/// 图标位置：先 exe 旁（打包后的形态），再 crate 内（`cargo run` 的形态）。
pub fn icon_path() -> Option<PathBuf> {
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(ICON_FILE)));
    if let Some(path) = beside_exe {
        if path.is_file() {
            return Some(path);
        }
    }
    let in_crate = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets").join(ICON_FILE);
    in_crate.is_file().then_some(in_crate)
}

#[cfg(test)]
mod tests {
    /// 记号字节数与边长必须对得上，否则 `ImageSrc::rgba` 会在起窗时 panic。
    #[test]
    fn mark_is_32x32_rgba() {
        let expected = (super::MARK_PX * super::MARK_PX * 4) as usize;
        assert_eq!(super::MARK_RGBA.len(), expected, "brand-mark-32.rgba 长度不对，重跑 pack-icon.ps1");
    }
}

//! 品牌只有一个源：这三处显示（自绘标题栏、窗口标题、任务栏/Alt-Tab）都从这里取。
//!
//! 图标是一枚 `.ico`（`assets/envboard.ico`，9 个尺寸 16–256，PNG-in-ICO），由
//! `tools/envboard-brand/make-icon.ps1` 画出来；它表达的是「进/出两条流」——
//! 上方白箭头向右、下方向左，靛青圆角方是主题的 `accent`。
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

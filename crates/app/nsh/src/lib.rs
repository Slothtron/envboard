//! `envboard-nsh` 的库：把业务语义（`model`）、布局（`pages`）、壳（`shell`）
//! 与数据面（`service` / `actions`）摆在一起，给 `main.rs` 一个 `App` 组件。
//!
//! 为什么是 lib + bin 而不是一个 main：golden 测试要能挂到这个 crate 上，
//! 而且 `model` 的纯函数（状态映射、计数、过滤）必须能在不开窗的情况下被单测钉住。

mod actions;
mod brand;
mod export;
mod model;
mod pages;
mod prefs;
mod service;
mod shell;
// N1（方案 §5.3 单实例）：preflight 是产品行为，不在 harness 后面 —— 发布物也要
// 拦住第二个写者。
pub mod single_instance;
// T2（方案 §6）：托盘是产品的常驻形态（关窗即隐藏、单击唤出、计数 tooltip）。
mod tray;
// B1（EB-DESIGN-006 §6.1）：可见性门读 NSH 的 `lifecycle()`（平台真值、零轮询），
// 页面循环挂在它门控的子树里 —— 隐藏即卸载，定时器随 future drop 一起消失。
mod live;
// T3（方案 §6.1 第 5 条）：退出走正常路径 —— 停环境（放端口）→ 合 WAL → 关窗
// 让 `run_app` 返回；托盘「退出」与「关窗即退」共用它。
mod shutdown;
// S4（方案 §4.1 F）：写 / 读两条常驻 worker，取代「一个动作一条临时线程」。
mod worker;
// T1（P6）托盘探针：只在 harness 构建里存在，产品构建不编这段代码。
#[cfg(feature = "harness")]
pub mod tray_probe;

pub use brand::{APP_ID, NAME, TAGLINE, WINDOW_TITLE, icon_path};
pub use service::record_boot_error;
pub use shell::App;

/// 起宿主（组合根 + reconcile + 唯一日志照看者）。窗口起来之前调一次；
/// 失败不致命：窗口照常开，错误落在界面的错误槽里（三层反馈的第一层）。
pub fn boot_host() -> Result<(), envboard_engine::Error> {
    service::boot()
}

// ---- 测试 / 压测口径（只在 `harness` 特征下生效）----------------------------
//
// 这几个变量（`NG_GOLDEN` / `NG_STRESS` / `NG_PAGE` / `NG_SELECTED` / `NG_DARK` /
// `NG_PLAIN` / `NG_DRAWER` / `NG_SETTINGS` / `NG_CONFIRM`）能改**数据来源**：铺 10k 条
// 假环境、不连宿主、跳到某一页。它们编进发布物的话，一个误设的环境变量就能让正式版
// 显示假数据，所以整族收在特征后面。
//
// 默认构建里这两个函数恒为「没有」：不读环境变量，也不因此报错。

/// `NAME=1` 形式的开关。
pub fn harness_flag(name: &str) -> bool {
    #[cfg(feature = "harness")]
    {
        std::env::var(name).map(|v| v == "1").unwrap_or(false)
    }
    #[cfg(not(feature = "harness"))]
    {
        let _ = name;
        false
    }
}

/// 取值型的开关（`NG_STRESS=10000`、`NG_PAGE=cert`）。
pub fn harness_var(name: &str) -> Option<String> {
    #[cfg(feature = "harness")]
    {
        std::env::var(name).ok()
    }
    #[cfg(not(feature = "harness"))]
    {
        let _ = name;
        None
    }
}

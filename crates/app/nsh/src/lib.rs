//! `envboard-nsh` 的库：把业务语义（`model`）、布局（`pages`）、壳（`shell`）
//! 与数据面（`service` / `actions`）摆在一起，给 `main.rs` 一个 `App` 组件。
//!
//! 为什么是 lib + bin 而不是一个 main：golden 测试要能挂到这个 crate 上，
//! 而且 `model` 的纯函数（状态映射、计数、过滤）必须能在不开窗的情况下被单测钉住。

mod actions;
mod brand;
mod model;
mod pages;
mod service;
mod shell;

pub use brand::{icon_path, APP_ID, NAME, TAGLINE, WINDOW_TITLE};
pub use service::record_boot_error;
pub use shell::App;

/// 起宿主（组合根 + reconcile + 唯一日志照看者）。窗口起来之前调一次；
/// 失败不致命：窗口照常开，错误落在界面的错误槽里（三层反馈的第一层）。
pub fn boot_host() -> Result<(), envboard_engine::Error> {
    service::boot()
}

//! envboard 桌面入口。发布构建走 windows 子系统（无控制台黑窗）；
//! debug 构建保留控制台，启动日志与错误可见。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    // 产品身份从自己的 [[bin]] name 取（组合根不猜也不硬编码）。
    envboard_desktop::run(env!("CARGO_BIN_NAME"))
}

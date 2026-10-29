//! L1：离屏渲染一帧，把像素读回来落成 PNG。
//!
//! 为什么是独立可执行文件而不是测试：
//! - Windows 的 `current_headless_renderer()` 返回 `None`（没有离屏 GPU 渲染器），
//!   所以 `HeadlessAppContext::capture_screenshot` 在本平台不可用；
//! - `VisualTestAppContext` 在 gpui-pre 0.3.9 里只在 macOS 导出；
//! - 构造 `App` 的 `App::new_app` 是 crate 私有，测试进程里拿不到真实平台。
//!
//! 所以走应用自己的启动路径：开一个**放在屏幕外**的真窗口，等它画完第一帧，
//! 调 `Window::render_to_image()`（test-support 下公开）读回像素。
//! 它在 Windows 上是 D3D11 staging texture 回读，**不经过屏幕捕获**，
//! 所以锁屏时画到自己的 swapchain 上仍然成立。
//!
//! 用法：cargo run -p envboard-desktop --example screenshot -- [输出路径]

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use envboard_desktop::{Host, UiPrefs, Workbench};
use envboard_engine_fake::FakeEngine;
use envboard_host::{HostOptions, prepare_with};
use gpui_kit::{App, AppContext as _, px};

/// 装配界面需要的全局对象：一个真管理器 + 假引擎。
///
/// 引擎用 `FakeEngine`：它真的绑定端口、不代理流量，与真引擎同构。
fn install_globals(cx: &mut App, state_dir: &Path, runtime: &tokio::runtime::Runtime) {
    gpui_kit::init(cx);

    let options = HostOptions {
        state_dir: Some(state_dir.to_path_buf()),
        port_range: Some((17_100, 17_199)),
        capture_budget: 0,
        log_dir: None,
        no_log_file: true,
        max_log_bytes: None,
    };
    let prepared =
        prepare_with(options, Arc::new(FakeEngine::new())).expect("应当能用假引擎装配出宿主");
    cx.set_global(Host::new(&prepared, runtime.handle().clone()));
    cx.set_global(UiPrefs::load(&prepared.state_dir));
}

fn main() {
    let out = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../shots/workbench-offscreen.png")
        });

    // 状态目录落在 target/ 下：本机默认用户 TEMP 拒绝访问（见 ADR-001「本机验证边界」）。
    let state_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("ui-shot-state");
    let _ = std::fs::remove_dir_all(&state_dir);
    std::fs::create_dir_all(&state_dir).expect("应当能建状态目录");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("应当能建 tokio runtime");

    let captured = Rc::new(Cell::new(false));
    let result = captured.clone();
    // 闭包要用 state_dir，退出后还要清它 —— 所以给它一份 clone。
    let state_dir_for_run = state_dir.clone();

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            install_globals(cx, &state_dir_for_run, &runtime);

            // 放到屏幕外：对用户不可见，但仍然参与正常的换帧与合成。
            let (handle, _root) = gpui_kit::open_window(
                gpui_kit::WindowOptions {
                    window_bounds: Some(gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds {
                        origin: gpui_kit::point(px(-10000.0), px(-10000.0)),
                        size: gpui_kit::size(px(1180.0), px(760.0)),
                    })),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| Workbench::new(window, cx)),
            )
            .expect("应当能建起窗口");

            handle
                .update(cx, |_, window, _cx| {
                    window.on_next_frame(move |window, cx| {
                        match window.render_to_image() {
                            Ok(image) => {
                                if let Some(parent) = out.parent() {
                                    let _ = std::fs::create_dir_all(parent);
                                }
                                match image.save(&out) {
                                    Ok(()) => {
                                        eprintln!(
                                            "screenshot: {} ({}x{})",
                                            out.display(),
                                            image.width(),
                                            image.height()
                                        );
                                        captured.set(true);
                                    }
                                    Err(error) => eprintln!("写 PNG 失败：{error}"),
                                }
                            }
                            Err(error) => eprintln!("render_to_image 失败：{error}"),
                        }
                        cx.quit();
                    });
                })
                .expect("应当能安排下一帧");
        });

    let _ = std::fs::remove_dir_all(&state_dir);
    if !result.get() {
        std::process::exit(1);
    }
}

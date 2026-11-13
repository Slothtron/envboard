//! 系统托盘与后台运行入口（ADR-012）。
//!
//! 语义：**窗口关闭 = 窗口与 WebView 一起销毁，进程与引擎常驻**。托盘是回前台的
//! 唯一入口 —— 左键或菜单「打开工作台」都走 [`open_workbench`]：窗口在就
//! `unminimize + show + focus`，不在就按 `tauri.conf.json` 的同一份窗口配置**重建**
//! （`WebviewWindowBuilder::from_config`，capabilities 按 label 解析、CSP 由协议侧注入）。
//!
//! 托盘创建失败只降级（`Shell.background = false` ⇒ 关窗即退出）+ 一次警告，
//! 不留「关掉就找不回来」的死路；这也让无 explorer 的测试环境不被托盘硬依赖。
//!
//! 注意：窗口是**销毁**而不是隐藏。隐藏的 WebView 仍活着，而三条推送流本来
//! （现已两条）就是 Rust 侧轮询任务 —— 不销毁 + 不取消流，后台照样在跑。
//!
//! 重建窗口要**先设好几何、等首帧再显示**：`from_config` 会按配置的默认宽高
//! （1280×800）建窗并立即显示，WebView 首帧之前窗口还是白的 —— 先显示再贴几何，
//! 用户看到的就是「白窗出现在默认位置、随即跳到上次的位置」。

use std::sync::atomic::Ordering;
use std::time::Duration;

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Manager, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_dialog::DialogExt;

use crate::Shell;

/// 主窗口 label（tauri.conf.json / capabilities 都是这个值，不许另起）。
pub const MAIN_LABEL: &str = "main";

/// 创建托盘并挂到 `Shell`。返回是否就绪（false = 已降级为「关窗即退出」）。
pub fn install(app: &tauri::App, no_dialog: bool) -> bool {
    let Some(icon) = app.default_window_icon().cloned() else {
        warn(
            app.handle(),
            no_dialog,
            "exe 没有可用图标，系统托盘不可用；关闭窗口将退出应用",
        );
        return false;
    };
    match build(app, icon) {
        Ok(tray) => {
            // 句柄必须有人持有：TrayIcon 引用计数，drop 即移除图标。
            let shell = app.state::<Shell>();
            shell.background.store(true, Ordering::Relaxed);
            *shell.tray.lock().unwrap() = Some(tray);
            true
        }
        Err(error) => {
            warn(
                app.handle(),
                no_dialog,
                &format!("系统托盘创建失败（{error}）；关闭窗口将退出应用"),
            );
            false
        }
    }
}

fn build(app: &tauri::App, icon: Image<'_>) -> tauri::Result<TrayIcon<tauri::Wry>> {
    // ADR-014：多个形态可以同时挂着托盘图标，tooltip / 菜单项要能一眼分清。
    let profile = current_profile(app.handle());
    let open = MenuItem::with_id(app, "open", "打开工作台", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit_label = profile.display_name("退出 envboard");
    let quit = MenuItem::with_id(app, "quit", quit_label, true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &separator, &quit])?;

    TrayIconBuilder::with_id("envboard-tray")
        .icon(icon)
        .tooltip(profile.display_name("envboard 工作台"))
        .menu(&menu)
        // 左键 = 回前台，右键 = 菜单（tray-icon 的 Windows 默认两者都弹菜单）。
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().0.as_str() {
            "open" => open_workbench(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                open_workbench(tray.app_handle());
            }
        })
        .build(app)
}

/// 回前台：窗口在就恢复，不在就按 config 重建。
pub fn open_workbench(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_LABEL) {
        // 还不可见 = 正在等首帧（`reveal` 会自己露面）：这时候别抢着 show，
        // 否则又会出现一块白底空窗。
        if window.is_visible().unwrap_or(true) {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        }
        return;
    }

    match create_workbench(app, false) {
        Ok(_) => {}
        Err(error) => {
            // 留在托盘态，用户可以再点一次；不退出进程。
            eprintln!("envboard: 重建工作台窗口失败：{error}");
            app.dialog()
                .message(format!("重建工作台窗口失败：{error}"))
                .title("envboard 工作台")
                .kind(tauri_plugin_dialog::MessageDialogKind::Error)
                .blocking_show();
        }
    }
}

/// 创建主窗口：**首次启动与托盘重建共用一条路径**（`tauri.conf.json` 的 main 窗口
/// `create: false`，窗口由壳自己建）。顺序固定：隐藏建窗 → 重放加固 → 贴回几何 →
/// 首帧后再显示。
///
/// 为什么不能交给 Tauri 按 config 建：它按默认宽高建窗并**立即显示**，几何得等
/// `build()` 返回后才贴 —— 用户看到的是一块白底空窗出现在默认位置、再跳到上次的
/// 位置。`first` 区分「进程的第一个窗口」：只有它播 boot 自检屏。
pub fn create_workbench(app: &AppHandle, first: bool) -> Result<WebviewWindow, String> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == MAIN_LABEL)
        .cloned()
        .ok_or_else(|| "tauri.conf.json 里没有 label=main 的窗口配置".to_string())?;

    // 先取出要还原的几何：跨屏时 `WM_DPICHANGED` 会重设窗口，光贴一次会被盖掉，
    // 首帧前要再贴一次（见下面 `on_page_load`）。
    let desired = app
        .try_state::<Shell>()
        .and_then(|shell| shell.ui_prefs.window_geometry());

    let builder =
        WebviewWindowBuilder::from_config(app, &config).map_err(|error| error.to_string())?;
    // ADR-014：开发版 / 便携版 / 预览版带可读后缀，并把 WebView2 用户数据目录挪开
    // （正式版共享同一个 `%LOCALAPPDATA%\<identifier>`，多份 profile 互相踩缓存）。
    let profile = current_profile(app);
    let builder = builder.title(profile.display_name(&config.title));
    let builder = match profile.webview_data_dir(app) {
        Some(dir) => builder.data_directory(dir),
        None => builder,
    };
    let builder = if first {
        builder
    } else {
        // ADR-012：托盘重建出来的窗口不是「首次打开应用」，前端据此跳过 boot 自检屏
        // （`frontend/src/Workbench.tsx` 读这个全局；用初始化脚本而不是 IPC，是为了在
        // React 首帧之前就已知，不会先闪一下工作台再盖上自检屏）。
        builder.initialization_script("window.__envboardRecreated = true;")
    };

    let window = builder
        .visible(false)
        .on_page_load(move |window, payload| {
            if payload.event() == PageLoadEvent::Finished {
                if let Some(geometry) = desired {
                    crate::geometry::apply(&window, &geometry);
                }
                reveal(&window);
            }
        })
        .build()
        .map_err(|error| error.to_string())?;

    // ADR-009 的 WebView2 settings 不会随着重建自动重放，必须再钉一次。
    crate::harden_window(&window);
    // 窗口还不可见：几何记忆（ADR-012）在这里贴回去，用户看不到任何挪动。
    if let Some(geometry) = desired {
        crate::geometry::apply(&window, &geometry);
    }
    crate::geometry::track(app);
    // 兜底：页面加载事件不来（资源坏了 / WebView2 卡住）也必须露面，
    // 不能让「点了托盘没反应」。
    reveal_eventually(window.clone(), desired, Duration::from_millis(1500));
    Ok(window)
}

/// 露面：先 show 再 focus（对隐藏窗口 focus 是空操作）。
fn reveal(window: &WebviewWindow) {
    let _ = window.show();
    let _ = window.set_focus();
}

/// 超时兜底：只在仍不可见时动手，避免和 `PageLoadEvent::Finished` 抢着 show。
fn reveal_eventually(
    window: WebviewWindow,
    geometry: Option<crate::prefs::WindowGeometry>,
    delay: Duration,
) {
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        if !window.is_visible().unwrap_or(true) {
            if let Some(geometry) = geometry {
                crate::geometry::apply(&window, &geometry);
            }
            reveal(&window);
        }
    });
}

/// 本进程已解析的形态身份（ADR-014）：`Shell` 在 setup 里先 manage，托盘与窗口
/// 的创建都晚于它；兜底用正式版身份只是为了不 panic。
fn current_profile(app: &AppHandle) -> crate::profile::Profile {
    app.try_state::<Shell>()
        .map(|shell| shell.profile.clone())
        .unwrap_or_else(crate::profile::Profile::stable)
}

fn warn(app: &AppHandle, no_dialog: bool, message: &str) {
    eprintln!("envboard: {message}");
    if !no_dialog {
        app.dialog()
            .message(message)
            .title("envboard 托盘不可用")
            .kind(tauri_plugin_dialog::MessageDialogKind::Warning)
            .blocking_show();
    }
}

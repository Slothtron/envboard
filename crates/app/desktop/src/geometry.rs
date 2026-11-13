//! 窗口几何的跨会话记忆（ADR-012）：关窗即销毁窗口，托盘重建走的是
//! `tauri.conf.json` 的默认宽高 —— 不记位置/大小，每次回前台都会跳回默认值。
//!
//! 为什么不用 `tauri-plugin-window-state`：它的落点是应用级 config 目录（按
//! identifier 共享），而几何要跟主题一样随 **state-dir** 走（便携副本各记各的）。
//! 所以几何与主题同住 `<state_dir>/ui.json`，读写 best-effort，永不拦窗口创建。
//!
//! 存的是**物理像素**（`outer_position` / `inner_size` 的原值）。逻辑像素在多屏
//! 不同缩放比的环境里不是同一个坐标空间：逻辑值 1714.86 在 125% 屏上是 2143 物理、
//! 在 140% 屏上是 2400 物理。实测把窗口放到 140% 的第二块屏再重开，按逻辑还原会
//! 落到 (1810,143) 而不是 (2300,200)。物理坐标配合 `PhysicalPosition/PhysicalSize`
//! 不经过任何缩放换算，Windows 侧就是原样 SetWindowPos。
//!
//! 还有一件必须做的事：窗口被放到另一块 DPI 不同的屏时会收到 `WM_DPICHANGED`，
//! tao 按比例重设窗口尺寸（实测 1000×680 → 1400×952），会盖掉刚贴上的几何。所以
//! 几何贴两次 —— 建窗后立刻一次、首帧前再一次（这时 DPI 变更已处理完）；两次都在
//! 窗口不可见时完成，用户看不到任何挪动。
//!
//! 语义与 Tauri 的窗口 API 对齐（这是能原样还原的关键）：`x`/`y` 是**外框**左上角
//! （`outer_position` / `set_position`），`width`/`height` 是**客户区**大小
//! （`inner_size` / `set_size`，与 `tauri.conf.json` 的 width/height 同义）。
//! 混用 `outer_size` 与 `set_size` 会让窗口每关一次就胖一圈边框（14×38px @125%）。

use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};

use crate::Shell;
use crate::prefs::WindowGeometry;

/// 判定「还在屏上」：与任一显示器至少有这么大的重叠（物理像素）。
const MIN_VISIBLE_WIDTH: f64 = 160.0;
const MIN_VISIBLE_HEIGHT: f64 = 80.0;
/// 尺寸合法性：比工作台最小尺寸还小、或大到离谱（多屏拼接残留）的一律不恢复。
const MIN_WIDTH: f64 = 320.0;
const MIN_HEIGHT: f64 = 240.0;
const MAX_WIDTH: f64 = 16_384.0;
const MAX_HEIGHT: f64 = 16_384.0;

/// 显示器矩形（物理像素）。
#[derive(Debug, Clone, Copy, PartialEq)]
struct MonitorRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// 把保存的几何贴到窗口上（窗口应当还不可见；两次调用的第二次在首帧前）。
pub fn apply(window: &WebviewWindow, geometry: &WindowGeometry) {
    // 未标记 `physical` 的是旧版（逻辑像素）记录：跨 DPI 无法正确换算，忽略。
    if !geometry.physical {
        return;
    }
    let monitors = monitor_rects(window);
    if visible_on_any(geometry, &monitors) {
        let _ = window.set_position(PhysicalPosition::new(
            geometry.x.round() as i32,
            geometry.y.round() as i32,
        ));
    }
    if sane_size(geometry) {
        let _ = window.set_size(PhysicalSize::new(
            geometry.width.round() as u32,
            geometry.height.round() as u32,
        ));
    }
    // 位置/尺寸先落，再最大化：最大化态下 Windows 只认最大化矩形。
    if geometry.maximized {
        let _ = window.maximize();
    }
}

/// 记录当前（正常态）几何到内存。`Moved` / `Resized` 事件很密，这里只动内存，
/// 落盘留到关窗 / 退出 —— 每个事件写一次磁盘既慢又会把 ui.json 写坏在半路。
pub fn track(app: &AppHandle) {
    let Some(window) = app.get_webview_window(crate::tray::MAIN_LABEL) else {
        return;
    };
    // 最小化 / 最大化时窗口矩形不是用户想要回的那个位置，别覆盖正常态记录。
    if window.is_minimized().unwrap_or(false) || is_zoomed(&window) {
        return;
    }
    if let Some(geometry) = capture(&window) {
        remember(app, geometry);
    }
}

/// 关窗 / 退出前落盘。
pub fn save(app: &AppHandle, window: &WebviewWindow) {
    // 最小化时窗口矩形不可信：保留上一次正常态记录即可。
    if window.is_minimized().unwrap_or(false) {
        return;
    }
    let Some(shell) = app.try_state::<Shell>() else {
        return;
    };
    let tracked = *shell.window_geometry.lock().unwrap();
    let geometry = if is_zoomed(window) {
        // 最大化只记标志位：位置与宽高沿用最近一次正常态，否则取消最大化后
        // 会落回最大化时的屏幕矩形（Windows 上还带 8px 外扩）。
        let Some(mut geometry) = tracked else {
            return;
        };
        geometry.maximized = true;
        geometry
    } else {
        let Some(geometry) = capture(window) else {
            return;
        };
        WindowGeometry {
            maximized: false,
            ..geometry
        }
    };
    remember(app, geometry);
    if let Err(error) = shell.ui_prefs.set_window_geometry(geometry) {
        eprintln!("envboard: 保存窗口几何失败：{error}");
    }
}

fn remember(app: &AppHandle, geometry: WindowGeometry) {
    if let Some(shell) = app.try_state::<Shell>() {
        *shell.window_geometry.lock().unwrap() = Some(geometry);
    }
}

/// 读当前窗口几何（物理像素；最大化标志原样带回）。
fn capture(window: &WebviewWindow) -> Option<WindowGeometry> {
    let position = window.outer_position().ok()?;
    // 客户区大小：`set_size` 写的就是它，读外框会每次多算一圈边框。
    let size = window.inner_size().ok()?;
    Some(WindowGeometry {
        x: position.x as f64,
        y: position.y as f64,
        width: size.width as f64,
        height: size.height as f64,
        maximized: window.is_maximized().unwrap_or(false),
        physical: true,
    })
}

/// 是否处于最大化。Windows 上用**实时**的 `IsZoomed`：tao 的 `is_maximized`
/// 读的是缓存标志，而最大化瞬间 `WM_MOVE` 可能先于 `WM_SIZE` 到达 —— 那一刻
/// 窗口矩形已是屏幕大小、标志却还是 false，会把最大化矩形误记成「正常态」，
/// 用户取消最大化后就落回一个错位的窗口。
#[cfg(windows)]
fn is_zoomed(window: &WebviewWindow) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsZoomed;

    match window.hwnd() {
        // SAFETY: 句柄属于本进程的窗口，IsZoomed 只读窗口状态。
        Ok(hwnd) => unsafe { IsZoomed(hwnd.0) != 0 },
        Err(_) => window.is_maximized().unwrap_or(false),
    }
}

#[cfg(not(windows))]
fn is_zoomed(window: &WebviewWindow) -> bool {
    window.is_maximized().unwrap_or(false)
}

fn monitor_rects(window: &WebviewWindow) -> Vec<MonitorRect> {
    window
        .available_monitors()
        .map(|monitors| {
            monitors
                .iter()
                .filter_map(|monitor| {
                    let position = monitor.position();
                    let size = monitor.size();
                    (size.width > 0 && size.height > 0).then_some(MonitorRect {
                        x: position.x as f64,
                        y: position.y as f64,
                        width: size.width as f64,
                        height: size.height as f64,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn visible_on_any(geometry: &WindowGeometry, monitors: &[MonitorRect]) -> bool {
    if !sane_size(geometry) || !geometry.x.is_finite() || !geometry.y.is_finite() {
        return false;
    }
    monitors.iter().any(|monitor| {
        let overlap_width = (geometry.x + geometry.width).min(monitor.x + monitor.width)
            - geometry.x.max(monitor.x);
        let overlap_height = (geometry.y + geometry.height).min(monitor.y + monitor.height)
            - geometry.y.max(monitor.y);
        overlap_width >= MIN_VISIBLE_WIDTH && overlap_height >= MIN_VISIBLE_HEIGHT
    })
}

fn sane_size(geometry: &WindowGeometry) -> bool {
    (MIN_WIDTH..=MAX_WIDTH).contains(&geometry.width)
        && (MIN_HEIGHT..=MAX_HEIGHT).contains(&geometry.height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(x: f64, y: f64, width: f64, height: f64) -> MonitorRect {
        MonitorRect {
            x,
            y,
            width,
            height,
        }
    }

    fn geometry(x: f64, y: f64, width: f64, height: f64) -> WindowGeometry {
        WindowGeometry {
            x,
            y,
            width,
            height,
            maximized: false,
            physical: true,
        }
    }

    #[test]
    fn window_fully_on_a_primary_monitor_is_visible() {
        let monitors = [monitor(0.0, 0.0, 2560.0, 1440.0)];
        assert!(visible_on_any(
            &geometry(100.0, 80.0, 1600.0, 1000.0),
            &monitors
        ));
    }

    #[test]
    fn window_on_a_negative_origin_monitor_is_visible() {
        let monitors = [
            monitor(-2560.0, 0.0, 2560.0, 1440.0),
            monitor(0.0, 0.0, 2560.0, 1440.0),
        ];
        assert!(visible_on_any(
            &geometry(-2000.0, 100.0, 1600.0, 1000.0),
            &monitors
        ));
    }

    #[test]
    fn window_on_a_second_monitor_is_visible() {
        // 第二块屏起点 2560（物理），窗口整个落在它上面。
        let monitors = [
            monitor(0.0, 0.0, 2560.0, 1440.0),
            monitor(2560.0, 0.0, 2742.0, 1542.0),
        ];
        assert!(visible_on_any(
            &geometry(2875.0, 250.0, 1750.0, 1190.0),
            &monitors
        ));
    }

    #[test]
    fn window_mostly_off_screen_is_not_visible() {
        let monitors = [monitor(0.0, 0.0, 2560.0, 1440.0)];
        // 完全在右侧屏外（模拟拔掉的副屏）。
        assert!(!visible_on_any(
            &geometry(4000.0, 100.0, 1600.0, 1000.0),
            &monitors
        ));
        // 只露出 100px 宽：不足以抓住标题栏。
        assert!(!visible_on_any(
            &geometry(2460.0, 100.0, 1600.0, 1000.0),
            &monitors
        ));
    }

    #[test]
    fn invalid_window_floats_or_sizes_are_rejected() {
        let monitors = [monitor(0.0, 0.0, 2560.0, 1440.0)];
        assert!(!visible_on_any(
            &geometry(f64::NAN, 0.0, 1600.0, 1000.0),
            &monitors
        ));
        // 宽度 0：没写全的 ui.json 或旧版本残留。
        assert!(!visible_on_any(&geometry(0.0, 0.0, 0.0, 1000.0), &monitors));
        assert!(!sane_size(&geometry(0.0, 0.0, f64::INFINITY, 1000.0)));
        assert!(!sane_size(&geometry(0.0, 0.0, 100.0, 100.0)));
    }

    #[test]
    fn no_monitor_information_means_no_position_restore() {
        assert!(!visible_on_any(&geometry(0.0, 0.0, 1600.0, 1000.0), &[]));
    }
}

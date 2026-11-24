//! T1（P6）托盘探针：**只存在于 `harness` 特征构建**，产品构建里整模块不编。
//!
//! 它要回答的问题都是「真机上到底怎样」，不是「按文档应当怎样」：
//!
//! 1. 关窗后进程是否还活着（窗口隐藏而不是退出）；
//! 2. 隐藏时渲染/定时器是否还在跑（`perf_stats().fps` 与心跳一起看）；
//! 3. 关窗后 `127.0.0.1:<port>` 是否仍 LISTEN、抓包是否继续进表；
//! 4. 托盘单击是否唤出并聚焦；
//! 5. 右键菜单四项是否都能回调到 `on_select`；
//! 6. `set_tray` 连续调用会不会让图标闪（视觉判定，日志只记时序）。
//!
//! 开关：`NG_TRAY_PROBE=1`；日志：`EB_PROBE_TRAY=<文件路径>`，一行一件事，
//! 前缀是 unix 毫秒。写失败不致命——探针不许改产品的行为。
//!
//! 退出项在这里**只记日志**：真正的退出路径是 T3（停环境 → 放实例锁 →
//! WAL checkpoint），不在探针里造第二套。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nsh_core::element::desktop::{
    current_window, main_window, set_tray, tray, MenuItem, TrayClickEvent,
};
use nsh_core::element::perf_stats;
use nsh_core::prelude::{spawn, timer};

/// 探针是否启用（`NG_TRAY_PROBE=1`；只有 harness 构建读得到它）。
pub fn enabled() -> bool {
    crate::harness_var("NG_TRAY_PROBE").as_deref() == Some("1")
}

/// 主窗要不要自己接 closePress：由 `main.rs` 在建窗前问这里。
/// 探针要「关窗 = 隐藏」，所以这时把 `close_on_press` 交给应用处理。
pub fn wants_close_press() -> bool {
    enabled()
}

fn sink() -> Option<&'static Mutex<File>> {
    static SINK: OnceLock<Option<Mutex<File>>> = OnceLock::new();
    SINK.get_or_init(|| {
        let path: PathBuf = crate::harness_var("EB_PROBE_TRAY")?.into();
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
            .map(Mutex::new)
    })
    .as_ref()
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn log(line: &str) {
    if let Some(sink) = sink() {
        let mut file = sink.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writeln!(file, "{} {}", now_ms(), line);
        let _ = file.flush();
    }
}

fn visible_line(tag: &str) {
    let state = current_window().state();
    let (visible, focused) = match state {
        Some(state) => (state.visible, state.focused),
        None => (false, false),
    };
    log(&format!("{tag} visible={visible} focused={focused}"));
}

fn show_window() {
    let window = current_window();
    window.show();
    window.focus();
    visible_line("show");
}

fn hide_window() {
    current_window().hide();
    visible_line("hide");
}

/// 组一份「和产品最终形态同形」的托盘：tooltip + 菜单 + 单击。
/// `round` 只进 tooltip，用来在日志里区分第几次 `set_tray`。
fn build_tray(round: u32) {
    let icon = crate::icon_path();
    let mut tray = tray().tooltip(format!("envboard · P6 探针 · r{round}"));
    if let Some(icon) = icon {
        tray = tray.icon(icon.to_string_lossy().into_owned());
    }
    set_tray(
        tray.menu([
            MenuItem::new("显示窗口").on_select(|| {
                log("menu:show");
                show_window();
            }),
            MenuItem::new("隐藏窗口").on_select(|| {
                log("menu:hide");
                hide_window();
            }),
            MenuItem::new("环境（探针占位）").on_select(|| {
                log("menu:envs");
            }),
            MenuItem::new("抓包（探针占位）").on_select(|| {
                log("menu:capture");
            }),
            MenuItem::new("重设托盘 ×1").on_select(|| {
                log("menu:reset1");
                build_tray(1);
                log("menu:reset1:done");
            }),
            MenuItem::new("重设托盘 ×10").on_select(|| {
                log("menu:reset10:begin");
                for round in 0..10 {
                    build_tray(round);
                }
                log("menu:reset10:done");
            }),
            MenuItem::new("退出（仅记录）").on_select(|| {
                log("menu:quit");
            }),
        ])
        .on_click(|event: &TrayClickEvent| {
            let bounds = event
                .bounds
                .map(|b| format!("{:.0},{:.0}", b.origin.x.0, b.origin.y.0))
                .unwrap_or_else(|| "none".to_owned());
            log(&format!("click at={bounds}"));
            show_window();
        }),
    );
}

/// 在组件作用域里装一次（`App()` 里调）。
pub fn install() {
    if !enabled() {
        return;
    }
    log("install:begin");

    // 关窗 = 隐藏：真机验收第 1 条（进程在、监听在）靠它成立。
    main_window().on_close_press(|reason| {
        log(&format!("close_press reason={reason:?}"));
        hide_window();
    });

    // 连设两次：Tray 文档说「last set wins」且会重建，会不会闪由这里暴露。
    build_tray(0);
    log("install:set0");
    build_tray(1);
    log("install:set1");

    // 心跳：隐藏后定时器要是停了，日志会直接断在那里。
    spawn(async move {
        let mut beat = 0u64;
        loop {
            timer(Duration::from_millis(1000)).await;
            beat += 1;
            let fps = perf_stats().map(|stats| stats.fps);
            let state = current_window().state();
            let (visible, focused) = match state {
                Some(state) => (state.visible, state.focused),
                None => (false, false),
            };
            log(&format!(
                "hb n={beat} fps={} visible={visible} focused={focused}",
                fps.map(|f| format!("{f:.1}"))
                    .unwrap_or_else(|| "none".to_owned())
            ));
        }
    });

    log("install:done");
}

//! T2（方案 §6）：托盘是**产品**的常驻形态，不在 harness 后面。
//!
//! envboard 的核心价值是代理一直挂着：用户关窗时把窗口藏起来、进程与抓包会话
//! 继续，需要时从托盘叫回来。TT1 探针已经把「关窗 = 隐藏」「单击唤出」「菜单回调」
//! 「`SetTray` 连设不闪」四条在真机上验过（见 v2 计划 §6.3.1），这里是把探针形状
//! 落成产品行为，并补上探针没做的两件事：
//!
//! 1. **隐藏态降频**（T1 的硬要求）：T1 实测「隐藏不省 CPU」（环境页 0.12% vs 可见
//!    0.16%，抓包页隐藏态反而更高）。所以这里的 `window_visible()` 是给定时器读的
//!    唯一判据——抓包页版本拍 80ms → 1000ms、环境页快照 800ms → 2000ms，可见即恢复。
//! 2. **计数进托盘**：tooltip 与菜单里的数字取 `Ctx.envs` / `Ctx.captured`，
//!    **只在值变化时**才 `set_tray`（T1 实测一次重建 ≈33ms，每帧重建纯属浪费）。
//!
//! 可见性为什么不直接读 `state().visible`：T1 实测 `WindowRef::state()` 读的是**上一拍**
//! （`hide()` 之后立刻读仍是 `true`，~90ms 后才 false）。自己先置位，降频下一拍就生效；
//! 每秒的刷新循环再从 `state()` 把「外部 show（例如 N2 的二次启动激活）」
//! 校正回来。
//!
//! 退出只走**正常返回**路径（关窗 → `run_app` 返回 → 进程退出），不碰 `process::exit`。
//! 收摊（停所有在跑实例 → `state.db` WAL checkpoint）由 `shutdown::quit()` 负责
//! （方案 §6.1 第 5 条 / §6.6）：菜单「退出」与「常驻关掉后点 X」都把它当唯一入口，
//! 这里不造第二套退出。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::actions::Ctx;

/// 窗口此刻是否可见（降频判据）。初值 `true`：首次启动是正常显示窗口（§6.1 第 7 条）。
static VISIBLE: AtomicBool = AtomicBool::new(true);
/// 用户点了菜单里的「退出」：关窗处理要看它，决定「藏」还是「真关」。
static QUIT: AtomicBool = AtomicBool::new(false);

/// 隐藏时抓包页的版本拍间隔（T1 硬要求：80 → 1000）。
pub const CAPTURE_POLL_MS_VISIBLE: u64 = 80;
pub const CAPTURE_POLL_MS_HIDDEN: u64 = 1000;
/// 隐藏时环境页快照轮询间隔（T1 硬要求：800 → 2000）。
pub const SNAPSHOT_POLL_MS_VISIBLE: u64 = 800;
pub const SNAPSHOT_POLL_MS_HIDDEN: u64 = 2000;

/// 窗口可见吗？页面里所有「按拍数干活」的循环都读它。
pub fn window_visible() -> bool {
    VISIBLE.load(Ordering::Acquire)
}

/// 抓包页这一拍该等多久（可见 80ms / 隐藏 1000ms）。
pub fn capture_poll_interval() -> Duration {
    Duration::from_millis(if window_visible() {
        CAPTURE_POLL_MS_VISIBLE
    } else {
        CAPTURE_POLL_MS_HIDDEN
    })
}

/// 环境页这一拍该等多久（可见 800ms / 隐藏 2000ms）。
pub fn snapshot_interval() -> Duration {
    Duration::from_millis(if window_visible() {
        SNAPSHOT_POLL_MS_VISIBLE
    } else {
        SNAPSHOT_POLL_MS_HIDDEN
    })
}

/// 托盘上显示的三组数字。`PartialEq` 用来判「值到底变没变」。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Counts {
    running: usize,
    total: usize,
    captured: u64,
}

fn counts(cx: Ctx) -> Counts {
    let envs = cx.envs.get();
    let c = crate::model::counts(&envs);
    Counts {
        running: c.running,
        total: c.total,
        captured: cx.captured.get(),
    }
}

/// 菜单「抓包」在手里没有会话时该落到哪个环境上：优先运行中的，其次第一条。
/// 挑不出来（账本空）就留空——那样页面显示的是「还没有环境」，至少不是假装在读。
fn capture_default_env(envs: &[crate::model::Env]) -> Option<String> {
    envs.iter()
        .find(|env| env.health == crate::model::Health::Running)
        .or_else(|| envs.first())
        .map(|env| env.name.clone())
}

#[cfg(windows)]
mod imp {
    use super::{counts, Counts, QUIT, VISIBLE};
    use crate::actions::Ctx;
    use nsh_core::desktop::CloseReason;
    use nsh_core::element::desktop::{
        main_window, set_tray, tray, MenuItem, TrayClickEvent,
    };
    use nsh_core::prelude::{spawn, timer};
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    /// 显示并聚焦主窗（单击托盘、菜单「显示窗口」、菜单「环境」「抓包」都走它）。
    pub(super) fn show_window(cx: Ctx) {
        VISIBLE.store(true, Ordering::Release);
        let window = main_window();
        window.show();
        window.focus();
        log::info!("托盘：唤出主窗口（可见性置真，轮询回满速）");
        // 从托盘回来时把设置框留着不动：用户可能正是从那里退出去的。
        let _ = cx;
    }

    fn hide_window() {
        // 先置不可见、再隐藏：降频下一拍就生效，不用等 `state()` 追上来。
        VISIBLE.store(false, Ordering::Release);
        main_window().hide();
        log::info!("托盘：主窗口已隐藏（轮询降频）");
    }

    /// 菜单「抓包」：切到抓包页。手里没有抓包会话时挑一个环境打开
    /// （优先运行中的）——否则点进去只会看到一张空表，等于没反应。
    fn open_capture(cx: Ctx) {
        cx.page.set(0);
        if cx.capture_env.get().is_some() {
            return;
        }
        let pick = super::capture_default_env(&cx.envs.get());
        cx.capture_env.set(pick);
    }

    fn request_quit() {
        log::info!("托盘：退出（走正常关闭路径）");
        // T3：这里不再自己 close。收摊（停环境 → 合库）走完之后才关窗，
        // 否则 `run_app` 立刻返回，进程在 WAL 还没合、端口还没放的时候就没了。
        crate::shutdown::quit();
    }

    /// 组一份托盘。标签里的数字是当前值，所以每次值变化都重建一次（T1 实测 ≈33ms）。
    fn build(cx: Ctx, c: Counts) {
        let mut t = tray().tooltip(format!(
            "{} · {} 运行中 · 捕获 {}",
            crate::brand::NAME,
            c.running,
            c.captured
        ));
        if let Some(icon) = crate::icon_path() {
            t = t.icon(icon.to_string_lossy().into_owned());
        }
        set_tray(
            t.menu([
                MenuItem::new("显示窗口").on_select(move || {
                    log::info!("托盘菜单：显示窗口");
                    show_window(cx);
                }),
                MenuItem::new(format!("环境（{}/{}）", c.running, c.total)).on_select(move || {
                    log::info!("托盘菜单：环境");
                    show_window(cx);
                    cx.page.set(0);
                    cx.capture_env.set(None);
                }),
                MenuItem::new(format!("抓包（{}）", c.captured)).on_select(move || {
                    log::info!("托盘菜单：抓包");
                    show_window(cx);
                    open_capture(cx);
                }),
                MenuItem::new("退出").on_select(request_quit),
            ])
            .on_click(move |_event: &TrayClickEvent| {
                log::info!("托盘：单击");
                show_window(cx)
            }),
        );
    }

    /// 在组件作用域里装一次（`shell.rs` 的 `App()`）。
    pub fn install(cx: Ctx) {
        // 闭包里不捕获 `WindowRef`：`on_close_press` 借用着它，再往里搬就成自引用。
        // 关窗发生时环境一定已经起来了，`main_window()` 现取即可。
        main_window().on_close_press(move |reason: CloseReason| {
            if QUIT.load(Ordering::Acquire) {
                // 菜单「退出」：真的关。关掉之后 `run_app` 返回，进程正常退出。
                main_window().close();
                return;
            }
            if cx.close_to_tray.get() {
                log::info!("托盘：关窗即隐藏（{reason:?}）");
                hide_window();
            } else {
                // 用户在设置里关掉了「常驻」：行为回到没有托盘时 —— 关窗即退。
                // T3：也是退出，所以和菜单「退出」共用同一条收摊路径（停环境、
                // 合库，再关窗）；这里不能直接 close，否则进程在 WAL 还没合的时候就没。
                log::info!("托盘：常驻已关闭，关窗即退（{reason:?}）");
                crate::shutdown::quit();
            }
        });

        let initial = counts(cx);
        build(cx, initial);
        log::info!("托盘：已挂载（{initial:?}）");

        spawn(async move {
            let mut last = initial;
            loop {
                timer(Duration::from_millis(1000)).await;
                // 外部 show（N2 的二次启动激活、任务栏还原）不改我们的标志，
                // 每秒从平台真值校正一次。`state()` 读上一拍，但 1s 粒度足够。
                if let Some(state) = main_window().state() {
                    VISIBLE.store(state.visible, Ordering::Release);
                }
                let now = counts(cx);
                if now != last {
                    last = now;
                    build(cx, now);
                }
            }
        });
    }
}

#[cfg(windows)]
pub use imp::install;

#[cfg(not(windows))]
pub fn install(_cx: Ctx) {}

/// 把「放行关窗」拨上（T3）：收摊完成后 `close()` 进来时 `on_close_press` 直接放行。
#[cfg(windows)]
pub(crate) fn allow_close() {
    QUIT.store(true, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_menu_lands_on_a_running_env() {
        let envs = crate::model::fixture();
        assert_eq!(
            capture_default_env(&envs).as_deref(),
            Some("proxy-a"),
            "抓包菜单要落到运行中的环境上"
        );
        assert_eq!(capture_default_env(&[]), None, "空账本不该编出一个名字");
    }

    #[test]
    fn capture_menu_falls_back_to_the_first_env() {
        let mut envs = crate::model::fixture();
        for env in &mut envs {
            env.health = crate::model::Health::Stopped;
        }
        assert_eq!(capture_default_env(&envs).as_deref(), Some("proxy-a"));
    }

    #[test]
    fn hidden_poll_is_the_slow_one() {
        // 两个间隔是「隐藏必须更省」的全部依据，钉住数值免得被人改回 80。
        assert_eq!(CAPTURE_POLL_MS_VISIBLE, 80);
        assert_eq!(CAPTURE_POLL_MS_HIDDEN, 1000);
        assert_eq!(SNAPSHOT_POLL_MS_VISIBLE, 800);
        assert_eq!(SNAPSHOT_POLL_MS_HIDDEN, 2000);
    }
}

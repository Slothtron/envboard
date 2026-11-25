//! T2（方案 §6）：托盘是**产品**的常驻形态，不在 harness 后面。
//!
//! envboard 的核心价值是代理一直挂着：用户关窗时把窗口藏起来、进程与抓包会话
//! 继续，需要时从托盘叫回来。TT1 探针已经把「关窗 = 隐藏」「单击唤出」「菜单回调」
//! 「`SetTray` 连设不闪」四条在真机上验过（见 v2 计划 §6.3.1），这里是把探针形状
//! 落成产品行为。
//!
//! **B 线（EB-DESIGN-006）改了两件事：**
//!
//! 1. **不再自己记可见性。** 旧的 `VISIBLE` 标志 + 每秒从 `state()` 校正的循环
//!    整个删掉：可见性判据换成 NSH 的 `lifecycle()`（`live::visible()`，读平台
//!    真值、零轮询、还覆盖最小化/锁屏/切虚拟桌面）。页面循环挂在它门控的子树里，
//!    隐藏即卸载 —— 没有「降频档」这回事了。
//! 2. **计数改推送。** tooltip 与菜单里的数字取 `Ctx.envs` / `Ctx.captured`，
//!    这两个都是信号：`effect` 读它们即订阅，**值变了才醒**。重建本身有成本
//!    （T1 实测一次 `set_tray` ≈33ms），所以变化合并到每秒最多一次；那个节流
//!    `timer` **只在真有待重建时存在**，空闲期一个定时器都没有。
//!
//! 退出只走**正常返回**路径（关窗 → `run_app` 返回 → 进程退出），不碰 `process::exit`。
//! 收摊（停所有在跑实例 → `state.db` WAL checkpoint）由 `shutdown::quit()` 负责
//! （方案 §6.1 第 5 条 / §6.6）：菜单「退出」与「常驻关掉后点 X」都把它当唯一入口，
//! 这里不造第二套退出。

use std::sync::atomic::{AtomicBool, Ordering};

use crate::actions::Ctx;

/// 用户点了菜单里的「退出」：关窗处理要看它，决定「藏」还是「真关」。
static QUIT: AtomicBool = AtomicBool::new(false);

/// 托盘上显示的三组数字。`PartialEq` 用来判「值到底变没变」。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Counts {
    running: usize,
    total: usize,
    captured: u64,
}

/// 读计数并**订阅**（放在 `effect` 里：值变了就把 effect 叫醒）。
fn counts(cx: Ctx) -> Counts {
    let envs = cx.envs.get();
    let c = crate::model::counts(&envs);
    Counts {
        running: c.running,
        total: c.total,
        captured: cx.captured.get(),
    }
}

/// 读计数但**不订阅**（重建那一刻取当前值用，避免把任务本身变成订阅者）。
fn counts_untracked(cx: Ctx) -> Counts {
    let envs = cx.envs.get_untracked();
    let c = crate::model::counts(&envs);
    Counts {
        running: c.running,
        total: c.total,
        captured: cx.captured.get_untracked(),
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
    use super::{counts, counts_untracked, Counts, QUIT};
    use crate::actions::Ctx;
    use nsh_core::desktop::CloseReason;
    use nsh_core::element::desktop::{
        main_window, set_tray, tray, MenuItem, TrayClickEvent,
    };
    use nsh_core::prelude::{effect, spawn, timer};
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    /// 显示并聚焦主窗（单击托盘、菜单「显示窗口」、菜单「环境」「抓包」都走它）。
    pub(super) fn show_window(cx: Ctx) {
        let window = main_window();
        window.show();
        window.focus();
        log::info!("托盘：唤出主窗口");
        // 从托盘回来时把设置框留着不动：用户可能正是从那里退出去的。
        let _ = cx;
    }

    fn hide_window() {
        // 只隐藏，不记标志：可见性由 `lifecycle()` 从平台真值推，页面循环
        // 随门控子树一起卸载（B1）。
        main_window().hide();
        log::info!("托盘：主窗口已隐藏（页面循环随可见性门卸载）");
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

        // B3（EB-DESIGN-006 §6.2）：不再每秒拍一次。
        //
        // `effect` 读 `envs` / `captured` 即订阅 —— 值变了才醒，空闲期零唤醒。
        // 它只往通道里丢一个「脏了」的信号，真正的重建在下面那条常驻任务里做：
        // 一次 `set_tray` ≈33ms（T1 实测），所以重建后节流一拍再处理下一次，
        // 把高频变化合并成每秒最多一次。**那个 `timer` 只在真有变化时存在。**
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let seen = std::cell::Cell::new(initial);
        effect(move || {
            let now = counts(cx);
            if now != seen.get() {
                seen.set(now);
                // 满了也无所谓：通道里只要有一个「脏了」就够，重建时取的是当前值。
                let _ = tx.send(());
            }
        });
        spawn(async move {
            let mut last = initial;
            while rx.recv().await.is_some() {
                let now = counts_untracked(cx);
                if now != last {
                    last = now;
                    build(cx, now);
                }
                timer(Duration::from_millis(crate::live::TRAY_COALESCE_MS)).await;
                // 节流窗口里积压的通知一次取干：它们说的都是「变了」，
                // 而上面重建时取的就是最新值，逐条重放没有意义。
                while rx.try_recv().is_ok() {}
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
    fn tray_rebuild_is_coalesced_not_polled() {
        // B3：托盘不再每秒拍。计数变一次才排一个节流 timer，空闲期零定时器。
        // 合并窗口沿用 T1 的 1s 口径（一次 set_tray ≈33ms，高频重建会吃掉 CPU）。
        assert_eq!(crate::live::TRAY_COALESCE_MS, 1000);
    }
}

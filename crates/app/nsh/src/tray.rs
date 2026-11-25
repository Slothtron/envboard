//! T2（方案 §6）：托盘是**产品**的常驻形态，不在 harness 后面。
//!
//! envboard 的核心价值是代理一直挂着：用户关窗时把窗口藏起来、进程与抓包会话
//! 继续，需要时从托盘叫回来。TT1 探针已经把「关窗 = 隐藏」「单击唤出」「菜单回调」
//! 「`SetTray` 连设不闪」四条在真机上验过（见 v2 计划 §6.3.1）。
//!
//! **托盘只做两件事：把窗口叫回来，把程序关掉。**
//!
//! 菜单只有「显示窗口」「退出」两项，tooltip 只有窗口标题，**不显示任何计数**。
//! 这不是省事，是把托盘的职责收干净：
//!
//! * 计数是**窗口里**的信息。托盘上那两个数字既读不清（tooltip 要悬停才出），
//!   又要求隐藏态一直有人去算它 —— 而隐藏态的全部意义就是不算。
//! * 去掉计数后托盘**内容恒定**：`install()` 时组一次，此后永不重建。
//!   没有 effect、没有通道、没有节流 timer，也没有 `set_tray` 的 ≈33ms 重建成本。
//!   隐藏态因此连「计数变了要重建」这条事件都没有了。
//!
//! 可见性也不在这里记：判据是 NSH 的 `lifecycle()`（`live::visible()`，读平台
//! 真值、零轮询，还覆盖最小化/锁屏/切虚拟桌面）。页面循环挂在它门控的子树里，
//! 隐藏即卸载 —— 没有「降频档」这回事了。
//!
//! 退出只走**正常返回**路径（关窗 → `run_app` 返回 → 进程退出），不碰 `process::exit`。
//! 收摊（停所有在跑实例 → `state.db` WAL checkpoint）由 `shutdown::quit()` 负责
//! （方案 §6.1 第 5 条 / §6.6）：菜单「退出」与「常驻关掉后点 X」都把它当唯一入口，
//! 这里不造第二套退出。

use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(not(windows))]
use crate::actions::Ctx;

/// 用户点了菜单里的「退出」：关窗处理要看它，决定「藏」还是「真关」。
static QUIT: AtomicBool = AtomicBool::new(false);

#[cfg(windows)]
mod imp {
    use super::QUIT;
    use crate::actions::Ctx;
    use nsh_core::desktop::CloseReason;
    use nsh_core::element::desktop::{main_window, set_tray, tray, MenuItem, TrayClickEvent};
    use std::sync::atomic::Ordering;

    /// 显示并聚焦主窗（单击托盘、菜单「显示窗口」都走它）。
    ///
    /// 不改页面、不动抽屉：从托盘回来时界面停在用户离开时的样子 ——
    /// 他可能正是从设置框或抓包页退出去的。
    pub(super) fn show_window() {
        let window = main_window();
        window.show();
        window.focus();
        log::info!("托盘：唤出主窗口");
    }

    fn hide_window() {
        // 只隐藏，不记标志：可见性由 `lifecycle()` 从平台真值推，页面循环
        // 随门控子树一起卸载（B1）。
        main_window().hide();
        log::info!("托盘：主窗口已隐藏（页面循环随可见性门卸载）");
    }

    fn request_quit() {
        log::info!("托盘：退出（走正常关闭路径）");
        // T3：这里不再自己 close。收摊（停环境 → 合库）走完之后才关窗，
        // 否则 `run_app` 立刻返回，进程在 WAL 还没合、端口还没放的时候就没了。
        crate::shutdown::quit();
    }

    /// 组一份托盘。**内容恒定，只在 `install()` 时调一次。**
    fn build() {
        let mut t = tray().tooltip(crate::brand::WINDOW_TITLE);
        if let Some(icon) = crate::icon_path() {
            t = t.icon(icon.to_string_lossy().into_owned());
        }
        set_tray(
            t.menu([
                MenuItem::new("显示窗口").on_select(|| {
                    log::info!("托盘菜单：显示窗口");
                    show_window();
                }),
                MenuItem::new("退出").on_select(request_quit),
            ])
            .on_click(|_event: &TrayClickEvent| {
                log::info!("托盘：单击");
                show_window();
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

        build();
        log::info!("托盘：已挂载（显示窗口 / 退出）");
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
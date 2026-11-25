//! T3（方案 §6.1 第 5 条、§6.4）：退出走**正常路径**。
//!
//! 顺序是硬的，而且必须趁进程还活着走完：
//!
//! 1. 停掉所有在跑的实例 —— 端口当场释放，下一个实例才不会撞 `PortConflict`；
//!    `desired` 不动（关程序 ≠ 用户要停这些环境，下次启动 reconcile 会拉回来）。
//! 2. 把 `state.db` 的 WAL 合进主库 —— `state.db-wal` 归零。
//! 3. 关窗让 `run_app` 正常返回 —— 进程走常规退出。`InstanceLock` 与 sqlite 句柄
//!    都在 `Service` 里，进程结束即释放；**不碰 `process::exit`**，也不靠外部强杀。
//!
//! 两个入口共用这一条：托盘菜单「退出」、以及设置里关掉「常驻」之后点 X。
//! 为什么要一个 `SHUTTING` 闸：收摊期间窗口还开着（最多几百毫秒到几秒），用户
//! 完全有机会再点一次 X 或再点一次菜单 —— 两套 `park_all` 叠着跑会互相抢句柄。

use std::sync::atomic::AtomicBool;
use std::time::Duration;

/// 收摊的兜底预算：`park_all` 里每个实例自己就有 5s 的 join 上限，这里再给一层
/// 上限，保证「引擎卡住」也不会让退出变成永远不关窗。
const SHUTDOWN_BUDGET: Duration = Duration::from_secs(15);

/// 已经在收摊（防重复入口叠跑）。
static SHUTTING: AtomicBool = AtomicBool::new(false);

#[cfg(windows)]
mod imp {
    use super::{SHUTDOWN_BUDGET, SHUTTING};
    use crate::service::ShutdownReport;
    use nsh_core::element::desktop::main_window;
    use nsh_core::prelude::spawn;
    use std::sync::atomic::Ordering;

    /// 唯一退出实现：收摊 → 关窗（让 `run_app` 返回）。重复调用是空操作。
    pub fn quit() {
        if SHUTTING.swap(true, Ordering::AcqRel) {
            log::info!("退出：已在收摊，忽略这次请求");
            return;
        }
        // 先把「放行关窗」拨上：等会儿 `main_window().close()` 进来时，
        // `on_close_press` 看到 `QUIT` 就真的关，不再拦成隐藏。
        crate::tray::allow_close();
        log::info!("退出：开始收摊（停环境 → WAL checkpoint → 关窗）");

        let rx = crate::service::shutdown();
        spawn(async move {
            let report = match tokio::time::timeout(SHUTDOWN_BUDGET, rx).await {
                Ok(Ok(report)) => report,
                Ok(Err(_)) => {
                    log::error!("退出：收摊线程没回话，按空报告继续");
                    ShutdownReport::default()
                }
                Err(_) => {
                    log::warn!(
                        "退出：收摊超过 {}s 还没回来，先关窗（已尽力停下能停的）",
                        SHUTDOWN_BUDGET.as_secs()
                    );
                    ShutdownReport::default()
                }
            };
            for problem in &report.problems {
                log::warn!("退出：实例没停下来 —— {problem}");
            }
            match &report.error {
                Some(error) => log::warn!("退出：WAL checkpoint 失败 —— {error}"),
                None if report.checkpointed => log::info!("退出：state.db-wal 已并入主库"),
                // 没有宿主时 checkpointed 也是 false，别把它记成失败。
                None => log::info!("退出：没有宿主，无 WAL 需要合并"),
            }
            log::info!(
                "退出：停掉 {} 个实例，关窗（`run_app` 正常返回）",
                report.stopped
            );
            main_window().close();
        });
    }
}

#[cfg(not(windows))]
mod imp {
    /// 非 Windows 只存在测试与编译检查，退出没有托盘入口。
    pub fn quit() {
        log::info!("退出：非 Windows 平台，没有托盘退出入口");
    }
}

pub use imp::quit;

#[cfg(all(test, windows))]
mod tests {
    use super::SHUTDOWN_BUDGET;
    use std::time::Duration;

    #[test]
    fn budget_is_bounded() {
        // 退出不能无限等：这条预算是「引擎卡住也不至于永不关窗」的唯一依据。
        assert_eq!(SHUTDOWN_BUDGET, Duration::from_secs(15));
    }
}

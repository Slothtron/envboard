//! B 线（EB-DESIGN-006 §6.1）：可见性门与「可见态兜底拍」。
//!
//! 判据是 NSH 的 `lifecycle()`，**不是我们自己的标志位**。它读平台真值
//! （`IsWindowVisible(root)` && 未最小化 && 未被 shell cloak），由
//! `WM_WINDOWPOSCHANGED` / `WM_ACTIVATEAPP` 与 presence 的系统通知驱动，
//! **全程不轮询**（`nsh-platform-windows/src/system.rs:390-420`、`presence.rs:1-28`）。
//! 因此比旧的 `tray::VISIBLE` 强三处：
//!
//! 1. 单实例激活那条绕过 `show_window()` 的 `focus()` 路径照样被它捕捉到，
//!    不必先把所有 show 入口收敛到一扇门；
//! 2. 最小化、切到别的虚拟桌面、会话锁屏、显示器关闭一并算「看不见」——
//!    这四种旧标志全漏了，页面照拍；
//! 3. 它是响应式的：读它就订阅，放进 `when` 的条件里即可随平台状态挂卸。
//!
//! **用法**：把「按拍数干活」的循环放进一个只在 `visible()` 为真时挂载的子树
//! （`when(live::visible, ...)`）。子树卸载 → owner 结束 → `spawn` 取消 →
//! NSH 的 `TimerKey` 在 future drop 时把定时器从队列里取回
//! （`nsh-core/src/element/task.rs:551-574`）——**隐藏态零残留，不需要写「停」的代码**。
//!
//! 下面三个常数是**可见态**的兜底拍，不是隐藏态的降频档：隐藏时循环整个不存在，
//! 没有「慢一档」这回事了。

use nsh_core::platform::Lifecycle;
use nsh_core::prelude::lifecycle;

/// 抓包页的兜底拍：版本号没变就什么都不做（S3 口径），这条只兜「通知漏了」。
pub const CAPTURE_FALLBACK_MS: u64 = 80;
/// 环境页的兜底拍：代次没变不动信号。
pub const SNAPSHOT_FALLBACK_MS: u64 = 800;

/// 窗口此刻真的看得见吗？读它会订阅。
pub fn visible() -> bool {
    lifecycle() != Lifecycle::Background
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_ticks_keep_the_t1_numbers() {
        // 可见态的两拍是 S3/T1 验过的口径，钉住免得被人随手改大。
        assert_eq!(CAPTURE_FALLBACK_MS, 80);
        assert_eq!(SNAPSHOT_FALLBACK_MS, 800);
    }
}
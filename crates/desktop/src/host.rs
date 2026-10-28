//! tokio ↔ GPUI 的唯一跨界通道。
//!
//! 两件事在这里解决：
//!
//! 1. [`Host`] —— 组合根交付的东西（Admin 门面 + tokio `Handle` + 宿主信息）作为
//!    GPUI Global 存在，视图经 `cx.global::<Host>()` 取用。**视图里不存第二份
//!    `Arc<AdminService>`**：数据只有一处真相，界面只有一处入口。
//! 2. [`hop`] —— 唯一一条"在 tokio 上跑一个用例、结果送回主线程"的路径。
//!
//! 为什么这条通道成立（都核对过实现，不是推测）：
//!
//! * `tokio::sync::oneshot` 是**传输**原语而不是 reactor 原语：`Receiver` 的 poll
//!   不需要时间/网络能力，跨线程唤醒走标准 `std::task::Waker`；
//! * `Context::spawn_in` 把 future 放在**主线程**执行器上（`gpui-pre 0.3.9`
//!   `src/executor.rs` 的 `ForegroundExecutor`），dipswitch 已在同一位置 await
//!   `background_spawn` 的结果，机制同构；
//! * `AdminService` 的 `start/stop/restart/reconcile` 本来就是 `async` 且 future 是
//!   `Send`（此前被 axum 的 boxed handler 要求过），所以直接交给 `Handle::spawn`
//!   即可，不需要 `LocalSet`；
//! * `WeakEntity::update_in` 在实体已随窗口销毁时返回 `Err` 而不是 panic。
//!
//! 任务所有权：[`hop`] 返回的 `Task` 必须存进视图字段（drop 即取消），但**关闭
//! 面板不取消在途写操作** —— 写要么发生要么没发生，中途取消会留下半态。

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::{Context, Global, Task, Window};
use tokio::runtime::Handle;

use envboard_admin::AdminService;
use envboard_host::Prepared;

/// 宿主：装配好的后端 + 它所属的 tokio 运行时句柄。
pub struct Host {
    admin: Arc<AdminService>,
    handle: Handle,
    state_dir: PathBuf,
    port_range: (u16, u16),
    warnings: Vec<String>,
}

impl Global for Host {}

impl Host {
    /// 由 `main` 在装配之后、开界面之前安装成 Global。
    ///
    /// `Runtime` 本体**不进**这里：它的生命周期由 `main` 的栈帧控制，
    /// 界面退出时按超时关停（见 `main` 的注释）。
    pub fn new(prepared: &Prepared, handle: Handle) -> Self {
        Self {
            admin: Arc::clone(&prepared.admin),
            handle,
            state_dir: prepared.state_dir.clone(),
            port_range: prepared.port_range,
            warnings: prepared.warnings.clone(),
        }
    }

    /// 管理门面：界面唯一的取数与下命令入口。
    pub fn admin(&self) -> &Arc<AdminService> {
        &self.admin
    }

    pub fn handle(&self) -> &Handle {
        &self.handle
    }

    pub fn state_dir(&self) -> &PathBuf {
        &self.state_dir
    }

    pub fn port_range(&self) -> (u16, u16) {
        self.port_range
    }

    /// 装配期的响亮警告（CA 被重新物化这类不能静默的事）。
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

/// 在 tokio 上跑一个用例，结果送回 GPUI 主线程交给 `on_done`。
///
/// 同步用例也走这条路（`async move { admin.list() }` 就行）：主线程一个字节都不该
/// 为后端买单，哪怕今天的读是纯内存的 —— 明天它可能读盘。
pub fn hop<View, Out>(
    window: &mut Window,
    cx: &mut Context<View>,
    work: impl Future<Output = Out> + Send + 'static,
    on_done: impl FnOnce(&mut View, Out, &mut Window, &mut Context<View>) + 'static,
) -> Task<()>
where
    View: 'static,
    Out: Send + 'static,
{
    let handle = cx.global::<Host>().handle().clone();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    handle.spawn(async move {
        let outcome = work.await;
        // 接收端可能随窗口销毁而消失：那正是"结果没人要了"，不是错误。
        let _ = sender.send(outcome);
    });

    cx.spawn_in(window, async move |this, cx| {
        if let Ok(outcome) = receiver.await {
            let _ = this.update_in(cx, |view, window, cx| on_done(view, outcome, window, cx));
        }
    })
}

//! 宿主编排：崩溃自愈（reconcile）与日志体积照看。
//!
//! 这两件事是**宿主**的职责而不是界面的职责：挂在一个界面的推送流里，就会每个客户端
//! 各干一遍。所以它们跟着组合根走，任意界面形态调 [`crate::Prepared::startup`] 就都拿到
//! 同一份语义。

use std::time::Duration;

use envboard_protocol::ReconcileReport;

/// 日志体积的照看间隔。
///
/// 每次只是给每个环境做一次 `stat`，只有超上限才真正轮转（copytruncate）——
/// 所以这个节拍可以很便宜。
const LOG_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(30);

impl crate::assembly::Prepared {
    /// 起宿主：先 reconcile（崩溃自愈），再起**唯一一个**日志照看者。
    ///
    /// 由调用方在 tokio 运行时上下文里 `.await`（宿主二进制 `runtime.block_on`，
    /// 界面形态 `handle.spawn`）。照看任务是**分离式**的：它随运行时结束而结束，
    /// 宿主不需要持有它。
    pub async fn startup(&self) -> Result<ReconcileReport, envboard_engine::Error> {
        let report = self.reconcile_now().await?;
        // 启动时先照看一次，不等第一个节拍。
        self.manager.maintain_logs();
        let manager = std::sync::Arc::clone(&self.manager);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(LOG_MAINTENANCE_INTERVAL).await;
                manager.maintain_logs();
            }
        });
        Ok(report)
    }

    /// 只跑一次崩溃自愈，不起照看者。
    ///
    /// 存在的理由：测试与「界面自己决定何时补一次 reconcile」的形态要用它；
    /// 生产宿主走 [`crate::Prepared::startup`]。
    pub async fn reconcile_now(&self) -> Result<ReconcileReport, envboard_engine::Error> {
        self.manager.reconcile().await
    }
}

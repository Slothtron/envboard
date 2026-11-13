//! 组合根的接缝测试：`prepare_with` 是"真管理器 + 假引擎"的唯一入口，
//! 界面形态的测试与宿主都走它，所以这里必须钉住它交付的语义。
//!
//! 断言面（只测组合根自己的职责，域规则由 manager/engine 的测试与 tests/contract-tests/fixtures 负责）：
//!
//! 1. 装配出来的门面真的能用（写一次、读得到、快照是全量）；
//! 2. 单实例锁由组合根持有，冲突时**响亮失败**且错误码与可操作文案都在；
//! 3. `startup()` 先 reconcile 再起日志照看，返回的报告是数据不是打印。

use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};

use envboard_engine::{ErrorCode, ProxyEngine};
use envboard_engine_fake::FakeEngine;
use envboard_host::{HostOptions, Prepared, prepare_with};

/// 每个测试一段独立的端口窗口，避免并行测试互相抢端口（与 manager 测试同一手法）。
static NEXT_WINDOW: AtomicU16 = AtomicU16::new(23_000);

/// 独立状态目录 + 独立端口窗口的组合根入参。
fn options(tag: &str) -> (std::path::PathBuf, HostOptions) {
    let window = NEXT_WINDOW.fetch_add(20, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "envboard-host-{tag}-{}-{window}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    (
        dir.clone(),
        HostOptions {
            state_dir: Some(dir),
            port_range: Some((window, window + 10)),
            // 测试不落盘日志：内存环形缓冲足够，且不往临时目录堆文件。
            no_log_file: true,
            ..Default::default()
        },
    )
}

fn fake_engine() -> Arc<dyn ProxyEngine> {
    Arc::new(FakeEngine::new())
}

#[test]
fn prepare_with_hands_the_facade_that_the_screens_use() {
    let (_dir, options) = options("facade");
    let prepared: Prepared =
        prepare_with(options, fake_engine()).expect("组合根必须能装出一个可用宿主");

    // 写：建一个环境（请求体形状 = envboard-admin crate 文档 的 environment.create）。
    let created = prepared
        .admin
        .create(&serde_json::json!({ "name": "alpha", "listen": { "port": 16_600 } }))
        .expect("create must land through the facade");
    assert_eq!(created.name, "alpha");

    // 读：列表与快照都从同一条路径出，快照是**全量**（界面不必自己拼增量）。
    let listed = prepared.admin.list().expect("list must read the ledger");
    assert_eq!(listed.len(), 1, "实际：{listed:?}");
    let (generation, snapshot) = prepared.admin.snapshot().expect("snapshot must not fail");
    assert_eq!(snapshot.len(), 1);
    assert_eq!(
        generation,
        prepared.state_generation(),
        "快照与状态栏读到的代次必须是同一个"
    );

    // 空快照形状：新建的环境期望运行但还没起，界面按 `desired`/`health` 渲染，
    // 组合根不许替它把状态"猜"成 running。
    assert_eq!(snapshot[0].desired, envboard_protocol::Desired::Stopped);
}

#[test]
fn second_host_on_one_state_dir_fails_loudly() {
    let (dir, options) = options("lock");
    let _first = prepare_with(options.clone(), fake_engine()).expect("第一个宿主必须起得来");

    let error = prepare_with(options, fake_engine())
        .err()
        .expect("同一状态目录必须有单实例锁");
    assert_eq!(error.code, ErrorCode::Conflict, "错误码是契约：{error:?}");
    assert!(
        error.message.contains("already owns this state dir"),
        "拿不到锁必须给一句可操作的话，实际：{}",
        error.message
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn startup_returns_the_reconcile_report_instead_of_printing_it() {
    let (_dir, options) = options("startup");
    let prepared = prepare_with(options, fake_engine()).expect("组合根必须能装出一个可用宿主");
    prepared
        .admin
        .create(&serde_json::json!({ "name": "beta", "listen": { "port": 16_601 } }))
        .expect("create must land");

    // startup 只做编排：返回报告，打印由调用方决定（宿主 println、界面渲染）。
    let report = prepared.startup().await.expect("startup must not fail");
    // 崩溃自愈对"刚建好、期望停着"的环境不该编排出任何动作。
    assert!(
        report.actions.is_empty(),
        "干净状态不应产生 reconcile 动作：{:?}",
        report.actions
    );
}

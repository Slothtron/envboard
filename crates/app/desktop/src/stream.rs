//! 两条推送流（自 web 传输面的 SSE 轮询器移植，帧形状与节拍不变）。
//!
//! 传输换成 Tauri `Channel`：每帧 = `{event, data}`，event 名沿用 SSE 的
//! `snapshot / events / error`，前端按名分发（`src/api/stream.ts`）。
//! 游标域两条流各不相同，不得混用（envboard-protocol crate 文档）：
//! - 快照流：cursor = 账本代次（advisory，同代次两帧内容仍可能不同）；
//! - 抓包流：cursor = 会话内 request_id。
//!
//! 生命周期：命令返回流 id，前端 `close()` 时调 `unwatch(id)` —— 注册表里的
//! watch sender 被丢弃，任务在下一个 select 拍退出。**不能只靠 Channel send
//! 失败兜底**：JS 侧丢弃 Channel 并不保证 send 失败，抽屉每开合一次就会泄漏
//! 一个轮询任务。send 失败（窗口销毁）仍是第二道退出闸。

use std::sync::Arc;
use std::time::Duration;

use envboard_protocol::{DebugEvents, DebugSnapshot, ErrorBody, SnapshotFrame};
use serde_json::{Value, json};
use tauri::State;
use tauri::ipc::Channel;
use tokio::time::MissedTickBehavior;

use crate::Shell;

/// 快照流：每秒一帧全环境视图（刻意全量而非事件溯源：丢帧不会长期显示错误）。
#[tauri::command]
pub fn watch_environments(shell: State<'_, Shell>, on_event: Channel<Value>) -> u64 {
    let (id, mut cancel) = shell.register_stream();
    let admin = Arc::clone(&shell.admin);
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(1_000));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = cancel.changed() => return,
                _ = ticker.tick() => {}
            }
            let cursor = admin.state_generation();
            let frame = match admin.list() {
                Ok(views) => SnapshotFrame {
                    ok: true,
                    cursor,
                    environments: views
                        .iter()
                        .map(|view| serde_json::to_value(view).unwrap_or(Value::Null))
                        .collect(),
                    error: None,
                },
                Err(error) => SnapshotFrame {
                    ok: false,
                    cursor,
                    environments: Vec::new(),
                    error: Some(ErrorBody {
                        code: error.code.as_str().to_string(),
                        message: error.message,
                        field: error.field,
                    }),
                },
            };
            if on_event
                .send(json!({"event": "snapshot", "data": frame}))
                .is_err()
            {
                return;
            }
        }
    });
    id
}

/// 调试抓包流：首帧无条件 `snapshot`（无目标时 env:null）；会话换代重发 snapshot；
/// 目标消失发 `{cursor:0, env:null, capture:null}`；其余 500ms 增量 `events`。
#[tauri::command]
pub fn watch_debug(shell: State<'_, Shell>, on_event: Channel<Value>) -> u64 {
    let (id, mut cancel) = shell.register_stream();
    let admin = Arc::clone(&shell.admin);
    tauri::async_runtime::spawn(async move {
        let mut cursor = 0u64;
        let mut session_key: Option<(u64, u64)> = None;
        let mut last_counts: Option<(u64, u64)> = None;
        let mut announced = false;
        let mut ticker = tokio::time::interval(Duration::from_millis(500));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = cancel.changed() => return,
                _ = ticker.tick() => {}
            }
            let Some((env, delta)) = admin.debug_delta(cursor, 500) else {
                if !announced || session_key.is_some() {
                    session_key = None;
                    cursor = 0;
                    last_counts = None;
                    announced = true;
                    let frame = DebugSnapshot {
                        cursor: 0,
                        env: None,
                        capture: None,
                    };
                    if on_event
                        .send(json!({"event": "snapshot", "data": frame}))
                        .is_err()
                    {
                        return;
                    }
                }
                continue;
            };
            announced = true;
            let key = (delta.session.id, delta.session.generation);
            if session_key != Some(key) {
                // 新会话/换代：snapshot 用尾部窗口（与拉取命令同形），
                // 游标推进到窗口内最新一条；本 tick 的 delta 记录被窗口覆盖，丢弃。
                let Some(view) = admin.captures(&env, 500).ok().flatten() else {
                    continue; // 目标实例刚好停了：下个 tick 走 idle 分支
                };
                cursor = view
                    .records
                    .last()
                    .and_then(|record| record.get("request_id"))
                    .and_then(Value::as_u64)
                    .unwrap_or(cursor);
                session_key = Some(key);
                last_counts = Some((view.captured, view.dropped));
                let frame = DebugSnapshot {
                    cursor,
                    env: Some(env),
                    capture: Some(view),
                };
                if on_event
                    .send(json!({"event": "snapshot", "data": frame}))
                    .is_err()
                {
                    return;
                }
            } else {
                let counts = (delta.captured, delta.dropped);
                if delta.records.is_empty() && last_counts == Some(counts) {
                    continue;
                }
                if let Some(id) = delta
                    .records
                    .last()
                    .and_then(|record| record.get("request_id"))
                    .and_then(Value::as_u64)
                {
                    cursor = id;
                }
                last_counts = Some(counts);
                let frame = DebugEvents {
                    cursor,
                    records: delta.records,
                    captured: counts.0,
                    dropped: counts.1,
                };
                if on_event
                    .send(json!({"event": "events", "data": frame}))
                    .is_err()
                {
                    return;
                }
            }
        }
    });
    id
}

/// 前端 `close()` 的落点：丢弃 watch sender → 任务在下一拍退出。
/// id 不存在（已退出/已取消）时静默成功 —— 取消是幂等的。
#[tauri::command]
pub fn unwatch(shell: State<'_, Shell>, id: u64) {
    shell.streams.lock().unwrap().remove(&id);
}

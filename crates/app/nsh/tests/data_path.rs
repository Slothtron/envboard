//! M3 的数据面验收：**列表读写通**。
//!
//! 不是点界面，而是直接打 `Prepared` 拿到的门面 —— 界面用的就是同一个门面，
//! 所以这里绿了，界面点下去会走的路径也绿了。四个动作一条链：
//! 建 → 起 → 停 → 删，每一步之后都读一次 `snapshot()` 看它确实变了；
//! 再补两条错误路径（重名、坏名字），确认信封是 `code` 而不是 panic。
//!
//! 状态目录固定在 `%TEMP%\eb-m3-gui`：测试之外还留着，给人工看窗用
//! （`ENVBOARD_STATE_DIR` 指过去即可看到这三条真数据）。

use std::path::PathBuf;

use envboard_host::HostOptions;
use serde_json::json;

fn state_dir() -> PathBuf {
    std::env::temp_dir().join("eb-m3-gui")
}

fn fresh_state_dir() -> PathBuf {
    let dir = state_dir();
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    dir
}

#[test]
fn list_read_write_round_trip() {
    let dir = fresh_state_dir();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("test runtime");

    let prepared = envboard_host::prepare(HostOptions {
        state_dir: Some(dir.clone()),
        app_dir_name: Some("envboard".to_owned()),
        ..HostOptions::default()
    })
    .expect("prepare");

    runtime.block_on(async {
        let _ = prepared.startup().await.expect("startup");
        let admin = &prepared.admin;

        // 空账本：一个环境都没有，界面的空态就是这一帧。
        assert!(admin.list().expect("list").is_empty());

        // 建两条（端口由管理器在区间里随机分）。
        let a = admin.create(&json!({ "name": "alpha" })).expect("create alpha");
        let b = admin.create(&json!({ "name": "beta" })).expect("create beta");
        assert_eq!(a.health, "stopped");
        assert_ne!(a.listen.port, b.listen.port, "两个环境不该分到同一个端口");

        // 起 alpha：start 之后 get 看到的必须是 running（不是 starting 的旧值）。
        let started = admin.start("alpha").await.expect("start alpha");
        assert_eq!(started.health, "running", "启动后应为 running：{started:?}");
        assert_eq!(admin.get("alpha").expect("get").health, "running");

        // 快照带代次：写操作之后代次必须变（界面靠它省掉无差异重绘）。
        let (cursor_before, _) = admin.snapshot().expect("snapshot");
        let stopped = admin.stop("alpha").await.expect("stop alpha");
        assert_eq!(stopped.health, "stopped");
        let (cursor_after, rows) = admin.snapshot().expect("snapshot");
        assert!(cursor_after > cursor_before, "账本代次应当前进");
        assert_eq!(rows.len(), 2);

        // 错误信封：重名、坏名字都必须是 Error（code + message），不是 panic。
        let dup = admin.create(&json!({ "name": "beta" })).unwrap_err();
        assert!(!dup.message.is_empty(), "错误必须带人话 message：{dup:?}");
        let bad = admin.create(&json!({ "name": "has space / slash" })).unwrap_err();
        assert!(!bad.code.as_str().is_empty());

        // 删一条：列表少一行，再读一次确认真的没了。
        admin.remove("beta").expect("remove beta");
        let rows = admin.list().expect("list");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "alpha");
    });
}

//! SQLite 账本（ADR-007）的存储契约：round-trip / 顺序 / 重启持久 /
//! 旧 JSON 一次性迁移（含 v2 遗物）/ DB 优先 / 版本门。
//!
//! 管理器行为（commit 先落盘后发事件等）由 manager_lifecycle 承载；
//! 这里只钉存储实现自己的语义。

use std::collections::BTreeMap;
use std::path::PathBuf;

use envboard_manager::state::{JsonFileStateRepo, PersistedState, StoredRules};
use envboard_manager::{SqliteStateRepo, StateRepo};
use serde_json::json;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "envboard-sqlite-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn sample_state() -> PersistedState {
    PersistedState {
        // 顺序有意非字母序：ord 必须保账本顺序而不是主键序。
        environments: vec![json!({"name": "zeta"}), json!({"name": "alpha"})],
        rules: vec![
            serde_json::from_value::<StoredRules>(json!({
                "name": "r1", "rendered": "127.0.0.1 a.test\n", "imported_at": 7
            }))
            .unwrap(),
        ],
        proxies: vec![json!({"name": "corp", "host": "127.0.0.1", "port": 8080})],
        // desired 的值类型是 engine::domain::Desired（与 protocol 的同名枚举不同源），
        // 经 serde 构造避免在测试里引第二个类型。
        desired: serde_json::from_value(json!({"zeta": "running"})).unwrap(),
        auto_port: BTreeMap::from([("zeta".to_string(), true)]),
        ..PersistedState::default()
    }
}

#[test]
fn round_trip_preserves_entities_order_and_maps() {
    let dir = temp_dir("roundtrip");
    let repo = SqliteStateRepo::open(&dir.join("state.db"), &dir.join("state.json")).unwrap();
    let state = sample_state();
    repo.save(&state).unwrap();
    let loaded = repo.load().unwrap();
    assert_eq!(loaded.environments, state.environments, "实体与顺序");
    assert_eq!(loaded.rules, state.rules);
    assert_eq!(loaded.proxies, state.proxies);
    assert_eq!(loaded.desired, state.desired);
    assert_eq!(loaded.auto_port, state.auto_port);
    assert_eq!(loaded.version, state.version);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn restart_reloads_from_disk() {
    let dir = temp_dir("restart");
    let db = dir.join("state.db");
    let json = dir.join("state.json");
    {
        let repo = SqliteStateRepo::open(&db, &json).unwrap();
        repo.save(&sample_state()).unwrap();
    }
    let repo = SqliteStateRepo::open(&db, &json).unwrap();
    let loaded = repo.load().unwrap();
    assert_eq!(loaded.environments.len(), 2, "重启后账本仍在");
    assert_eq!(loaded.environments[0]["name"], "zeta");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn empty_db_loads_default_state() {
    let dir = temp_dir("empty");
    let repo = SqliteStateRepo::open(&dir.join("state.db"), &dir.join("state.json")).unwrap();
    let loaded = repo.load().unwrap();
    assert!(loaded.environments.is_empty());
    assert_eq!(loaded.version, envboard_manager::STATE_VERSION);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn legacy_json_migrates_once_and_keeps_a_backup() {
    let dir = temp_dir("migrate");
    let json = dir.join("state.json");
    let db = dir.join("state.db");
    let legacy = JsonFileStateRepo::new(json.clone());
    legacy.save(&sample_state()).unwrap();

    let repo = SqliteStateRepo::open(&db, &json).unwrap();
    let loaded = repo.load().unwrap();
    assert_eq!(loaded.environments.len(), 2, "旧账本内容进 DB");
    assert!(!json.exists(), "旧文件应改名留备份");
    assert!(dir.join("state.json.migrated").exists(), "备份在");

    // 再 open 一次：不重复迁移、备份不被覆盖。
    let repo2 = SqliteStateRepo::open(&db, &json).unwrap();
    assert_eq!(repo2.load().unwrap().environments.len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn v2_relic_json_still_migrates() {
    // v2 遗物（records/config_seals）靠 serde 默认值与未知键丢弃消化 ——
    // 迁移源复用 JsonFileStateRepo 的解析口径，这条保证升级链不断。
    let dir = temp_dir("v2");
    let json = dir.join("state.json");
    std::fs::write(
        &json,
        r#"{"version":1,"environments":[{"name":"old"}],"records":{},"config_seals":{}}"#,
    )
    .unwrap();
    let repo = SqliteStateRepo::open(&dir.join("state.db"), &json).unwrap();
    let loaded = repo.load().unwrap();
    assert_eq!(loaded.environments.len(), 1);
    assert_eq!(loaded.environments[0]["name"], "old");
    assert!(dir.join("state.json.migrated").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn populated_db_wins_over_a_stray_json() {
    let dir = temp_dir("dbwins");
    let json = dir.join("state.json");
    let db = dir.join("state.db");
    let repo = SqliteStateRepo::open(&db, &json).unwrap();
    repo.save(&sample_state()).unwrap();
    drop(repo);
    // 迁移完成后又出现的 state.json（比如用户从备份拷回）不得覆盖 DB。
    JsonFileStateRepo::new(json.clone())
        .save(&PersistedState::default())
        .unwrap();
    let repo = SqliteStateRepo::open(&db, &json).unwrap();
    assert_eq!(repo.load().unwrap().environments.len(), 2, "DB 是权威");
    assert!(json.exists(), " stray json 不被误删/误改名");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn newer_db_version_is_rejected_loudly() {
    let dir = temp_dir("version");
    let db = dir.join("state.db");
    {
        let repo = SqliteStateRepo::open(&db, &dir.join("state.json")).unwrap();
        repo.save(&sample_state()).unwrap();
    }
    // 直接改 meta 模拟"未来版本写的库"。
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute("UPDATE meta SET value = '99' WHERE key = 'version'", [])
        .unwrap();
    drop(conn);
    let repo = SqliteStateRepo::open(&db, &dir.join("state.json")).unwrap();
    let error = repo.load().expect_err("新版本库必须响亮拒绝");
    assert_eq!(error.code.as_str(), "store_failure");
    assert!(error.message.contains("99"), "got {error:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

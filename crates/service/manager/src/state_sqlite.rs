//! SQLite 账本存储（ADR-007）：`state.json` 的权威继任者。
//!
//! 接缝是 [`StateRepo`](crate::ports::StateRepo)（load/save 两方法）——管理器
//! 的 commit/new 一行不改，换存储只换注入的实现。姿势照 QingJian `crates/store`：
//! `Mutex<Connection>` + WAL + `PRAGMA user_version` 的 append-only 迁移。
//!
//! 形状：**按实体建表**（environments/rules/proxies 各一行一实体，`ord` 保账本
//! 顺序），meta 表放 version 与三张映射（desired/auto_port/marks，JSON 文本）。
//! 每实体 doc 仍是既有的归一化 JSON —— serde 契约与字段校验口径不变，
//! 加载时管理器照旧逐条重校验。save = 单事务全删全插（账本规模小，原子性靠事务）。
//!
//! 迁移：open 时若 DB 无 version 且旧 `state.json` 在 → 经
//! [`JsonFileStateRepo`] 解析（复用它的版本门与 serde 默认值）→ 写 DB →
//! 旧文件改名 `state.json.migrated` 留备份。DB 已有数据则以 DB 为准，不碰旧文件。
//!
//! 权限：unix 0600（与 JSON 实现同口径）；Windows 靠目录 ACL（同现状）。
//! `-wal`/`-shm` 伴生文件由 SQLite 自管。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::Connection;

use crate::ports::StateRepo;
use crate::state::{JsonFileStateRepo, PersistedState, STATE_VERSION, restrict_permissions};
use envboard_engine::{Error, ErrorCode};

/// append-only 迁移批（`user_version` = 已应用批数）。只增不改不删。
const MIGRATIONS: &[&str] = &[
    // v1：账本三表 + meta。
    "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
     CREATE TABLE IF NOT EXISTS environments (name TEXT PRIMARY KEY, ord INTEGER NOT NULL, doc TEXT NOT NULL);
     CREATE TABLE IF NOT EXISTS rules (name TEXT PRIMARY KEY, ord INTEGER NOT NULL, doc TEXT NOT NULL);
     CREATE TABLE IF NOT EXISTS proxies (name TEXT PRIMARY KEY, ord INTEGER NOT NULL, doc TEXT NOT NULL);",
];

pub struct SqliteStateRepo {
    conn: Mutex<Connection>,
    #[allow(dead_code)] // 排障指路用（错误信封里带路径）
    path: PathBuf,
}

fn sql_error(path: &Path, error: rusqlite::Error) -> Error {
    Error::at(
        ErrorCode::StoreFailure,
        "state_db",
        format!("cannot access {}: {error}", path.display()),
    )
}

impl SqliteStateRepo {
    /// 打开（或创建）账本数据库，并完成旧 JSON 的一次性迁移。
    pub fn open(db_path: &Path, legacy_json: &Path) -> Result<Self, Error> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| Error::io_at("create", parent, error))?;
        }
        let conn = Connection::open(db_path).map_err(|error| sql_error(db_path, error))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|error| sql_error(db_path, error))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|error| sql_error(db_path, error))?;
        migrate(&conn).map_err(|error| sql_error(db_path, error))?;
        restrict_permissions(db_path)?;
        let repo = Self {
            conn: Mutex::new(conn),
            path: db_path.to_path_buf(),
        };
        repo.migrate_legacy_json(legacy_json)?;
        Ok(repo)
    }

    /// 旧 JSON → DB 的一次性迁移；DB 已有数据则跳过（DB 是权威）。
    fn migrate_legacy_json(&self, legacy: &Path) -> Result<(), Error> {
        if !legacy.exists() {
            return Ok(());
        }
        let has_version: bool = {
            let conn = self.conn.lock().unwrap();
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM meta WHERE key = 'version')",
                [],
                |row| row.get(0),
            )
            .map_err(|error| sql_error(&self.path, error))?
        };
        if has_version {
            return Ok(());
        }
        let state = JsonFileStateRepo::new(legacy.to_path_buf()).load()?;
        self.save(&state)?;
        let backup = legacy.with_extension("json.migrated");
        std::fs::rename(legacy, &backup).map_err(|error| Error::io_at("rename", legacy, error))?;
        Ok(())
    }
}

/// `user_version` 迁移：逐批 apply 未应用的，幂等（IF NOT EXISTS）。
fn migrate(conn: &Connection) -> Result<(), rusqlite::Error> {
    let version: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    for (index, batch) in MIGRATIONS.iter().enumerate() {
        if (index as u32) < version {
            continue;
        }
        conn.execute_batch(batch)?;
        conn.pragma_update(None, "user_version", index as u32 + 1)?;
    }
    Ok(())
}

impl StateRepo for SqliteStateRepo {
    fn load(&self) -> Result<PersistedState, Error> {
        let conn = self.conn.lock().unwrap();
        let version: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key = 'version'", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|error| sql_error(&self.path, error))?;
        let Some(raw) = version else {
            return Ok(PersistedState::default());
        };
        let version: u32 = raw.parse().map_err(|_| {
            Error::at(
                ErrorCode::StoreFailure,
                "state_db",
                format!("bad state version {raw:?}"),
            )
        })?;
        if version > STATE_VERSION {
            return Err(Error::at(
                ErrorCode::StoreFailure,
                "state_db",
                format!(
                    "state db version {version} is newer than supported {STATE_VERSION}; upgrade envboard"
                ),
            ));
        }

        let mut state = PersistedState {
            version,
            ..PersistedState::default()
        };
        state.environments =
            read_docs(&conn, "environments").map_err(|error| sql_error(&self.path, error))?;
        state.rules = read_docs(&conn, "rules").map_err(|error| sql_error(&self.path, error))?;
        state.proxies =
            read_docs(&conn, "proxies").map_err(|error| sql_error(&self.path, error))?;
        state.desired =
            read_meta(&conn, "desired").map_err(|error| sql_error(&self.path, error))?;
        state.auto_port =
            read_meta(&conn, "auto_port").map_err(|error| sql_error(&self.path, error))?;
        state.marks = read_meta(&conn, "marks").map_err(|error| sql_error(&self.path, error))?;
        Ok(state)
    }

    fn save(&self, state: &PersistedState) -> Result<(), Error> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|error| sql_error(&self.path, error))?;
        let map_err = |error: rusqlite::Error| sql_error(&self.path, error);
        tx.execute("DELETE FROM meta", []).map_err(map_err)?;
        tx.execute("DELETE FROM environments", [])
            .map_err(map_err)?;
        tx.execute("DELETE FROM rules", []).map_err(map_err)?;
        tx.execute("DELETE FROM proxies", []).map_err(map_err)?;

        let put_meta = |key: &str, value: String| -> Result<(), Error> {
            tx.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)",
                rusqlite::params![key, value],
            )
            .map_err(map_err)?;
            Ok(())
        };
        put_meta("version", state.version.to_string())?;
        put_meta(
            "desired",
            serde_json::to_string(&state.desired).unwrap_or_default(),
        )?;
        put_meta(
            "auto_port",
            serde_json::to_string(&state.auto_port).unwrap_or_default(),
        )?;
        put_meta(
            "marks",
            serde_json::to_string(&state.marks).unwrap_or_default(),
        )?;

        let rule_docs: Vec<serde_json::Value> = state
            .rules
            .iter()
            .map(|rule| serde_json::to_value(rule).unwrap_or_default())
            .collect();
        for (table, rows) in [
            ("environments", state.environments.clone()),
            ("rules", rule_docs),
            ("proxies", state.proxies.clone()),
        ] {
            for (ord, doc) in rows.iter().enumerate() {
                let name = doc
                    .get("name")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default();
                tx.execute(
                    &format!("INSERT INTO {table} (name, ord, doc) VALUES (?1, ?2, ?3)"),
                    rusqlite::params![name, ord as i64, doc.to_string()],
                )
                .map_err(map_err)?;
            }
        }
        tx.commit().map_err(map_err)?;
        Ok(())
    }
}

/// 按 ord 读一表的 doc 列并反序列化（实体形状契约不变）。
fn read_docs<T: serde::de::DeserializeOwned>(
    conn: &Connection,
    table: &str,
) -> Result<Vec<T>, rusqlite::Error> {
    let mut stmt = conn.prepare(&format!("SELECT doc FROM {table} ORDER BY ord"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        let text = row?;
        // 反序列化失败 = 库被外部改坏：响亮失败而不是静默丢实体。
        let item: T = serde_json::from_str(&text).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                text.len(),
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        out.push(item);
    }
    Ok(out)
}

/// meta 里的 JSON 映射；缺键 = 空映射（与 serde(default) 同口径）。
fn read_meta<T: serde::de::DeserializeOwned + Default>(
    conn: &Connection,
    key: &str,
) -> Result<T, rusqlite::Error> {
    let value: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()?;
    match value {
        Some(text) => serde_json::from_str(&text).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                text.len(),
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        }),
        None => Ok(T::default()),
    }
}

/// rusqlite 的 optional 扩展（query_row 无行 → None）。
trait OptionalRow<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalRow<T> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

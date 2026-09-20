//! manager 需要的端口 —— 逻辑与外界之间的唯一通道（也要能被假实现替换）。
//!
//! 每个端口都有"Fake/内存"实现（本模块）与"真实"实现（[`crate::infra`]）。
//! 管理器**不直接**碰 `/proc`、socket、文件系统：那些都在 `infra` 里，
//! 这样端口分配、reconcile、健康判定这些逻辑就能在没有网络、没有进程的测试里跑。

use std::collections::BTreeSet;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use envboard_engine::Error;

/// 判断某个地址的端口是否空闲。
///
/// **语义是契约的一部分**（`spec/capabilities.md` §端口分配第 5 条）：
/// 绑定地址必须与实例的 `listen.host` 一致，且**必须关闭 `SO_REUSEADDR`** ——
/// `tokio` 的 bind 默认打开它，会让某些已被占用的端口绑定成功而被误判为空闲。
pub trait PortProbe: Send + Sync {
    fn is_free(&self, host: IpAddr, port: u16) -> bool;

    /// 一批端口里哪些不空闲 —— 用于生成 `taken` 集合。
    ///
    /// 签名刻意用 `&[u16]` 而不是泛型迭代器：这个 trait 要被 `dyn` 使用
    /// （管理器持有 `Arc<dyn PortProbe>`），泛型方法会让它不再是 dyn 兼容的。
    fn taken_among(&self, host: IpAddr, ports: &[u16]) -> BTreeSet<u16> {
        ports
            .iter()
            .copied()
            .filter(|port| !self.is_free(host, *port))
            .collect()
    }
}
/// 状态存储。实现负责原子写与权限（0600）。
pub trait StateRepo: Send + Sync {
    fn load(&self) -> Result<crate::state::PersistedState, Error>;
    fn save(&self, state: &crate::state::PersistedState) -> Result<(), Error>;
}

/// 最小文件系统端口 —— 只放管理器真正需要的两件事：存在性、读取。
pub trait FilePort: Send + Sync {
    fn exists(&self, path: &Path) -> bool;
    fn read_to_string(&self, path: &Path) -> Result<String, Error>;
}

/// 控制面事件日志的存储端口（append-only JSONL；0600 与原子性由实现负责）。
pub trait EventStorePort: Send + Sync {
    /// 追加一行（不存在则创建，0600）。
    fn append(&self, path: &Path, line: String) -> Result<(), Error>;
    /// 当前字节数（None = 文件不存在）。
    fn size(&self, path: &Path) -> Option<u64>;
    /// 轮转：当前文件挪到 `<name>.1`（覆盖上一份）。
    fn rotate(&self, path: &Path) -> Result<(), Error>;
    /// 有界尾读（历史查询与 seq 恢复共用，不给"整文件读进内存"留路径）。
    fn read_tail(&self, path: &Path, max_bytes: u64) -> Result<String, Error>;
    /// 从 `offset` 增量读取（SSE 跟随用）。None = 文件不存在。
    /// 返回 (新 offset, 文本)；文本以整行结尾（半行留在下一次）。
    fn read_from(&self, path: &Path, offset: u64) -> Result<Option<(u64, String)>, Error>;
}

// --------------------------------------------------------------------------- //
// 测试替身（内存实现）。放在这里而不是 tests/ 里，是为了让 cli 的 smoke 与
// 契约测试都能直接复用同一份替身，避免"三套 fake 各有一套行为"。
// --------------------------------------------------------------------------- //

/// 内存端口探活：可声明任意端口为"已占用"。
#[derive(Debug, Default, Clone)]
pub struct MemoryPortProbe {
    occupied: BTreeSet<u16>,
}

impl MemoryPortProbe {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn occupy(&mut self, ports: impl IntoIterator<Item = u16>) {
        self.occupied.extend(ports);
    }
}

impl PortProbe for MemoryPortProbe {
    fn is_free(&self, _host: IpAddr, port: u16) -> bool {
        !self.occupied.contains(&port)
    }
}
/// 内存状态存储。
#[derive(Debug, Default)]
pub struct MemoryStateRepo {
    state: std::sync::Mutex<crate::state::PersistedState>,
}

impl MemoryStateRepo {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> crate::state::PersistedState {
        self.state.lock().unwrap().clone()
    }
}

impl StateRepo for MemoryStateRepo {
    fn load(&self) -> Result<crate::state::PersistedState, Error> {
        Ok(self.state.lock().unwrap().clone())
    }

    fn save(&self, state: &crate::state::PersistedState) -> Result<(), Error> {
        *self.state.lock().unwrap() = state.clone();
        Ok(())
    }
}

/// 内存文件端口。
#[derive(Debug, Default)]
pub struct MemoryFiles {
    files: std::sync::Mutex<std::collections::BTreeMap<PathBuf, String>>,
}

impl MemoryFiles {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn put(&self, path: impl Into<PathBuf>, contents: impl Into<String>) {
        self.files
            .lock()
            .unwrap()
            .insert(path.into(), contents.into());
    }
}

impl FilePort for MemoryFiles {
    fn exists(&self, path: &Path) -> bool {
        self.files.lock().unwrap().contains_key(path)
    }

    fn read_to_string(&self, path: &Path) -> Result<String, Error> {
        self.files
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .ok_or_else(|| {
                Error::new(
                    envboard_engine::ErrorCode::NotFound,
                    format!("{} does not exist", path.display()),
                )
            })
    }
}

/// 内存事件存储（测试替身）。
#[derive(Debug, Default)]
pub struct MemoryEventStore {
    files: std::sync::Mutex<std::collections::BTreeMap<PathBuf, String>>,
}

impl EventStorePort for MemoryEventStore {
    fn append(&self, path: &Path, line: String) -> Result<(), Error> {
        let mut files = self.files.lock().unwrap();
        match files.get_mut(path) {
            Some(existing) => existing.push_str(&line),
            None => {
                files.insert(path.to_path_buf(), line);
            }
        }
        Ok(())
    }

    fn size(&self, path: &Path) -> Option<u64> {
        self.files
            .lock()
            .unwrap()
            .get(path)
            .map(|text| text.len() as u64)
    }

    fn rotate(&self, path: &Path) -> Result<(), Error> {
        let mut files = self.files.lock().unwrap();
        if let Some(text) = files.remove(path) {
            let rotated = path.with_extension("jsonl.1");
            files.insert(rotated, text);
        }
        Ok(())
    }

    fn read_from(&self, path: &Path, offset: u64) -> Result<Option<(u64, String)>, Error> {
        let files = self.files.lock().unwrap();
        let Some(text) = files.get(path) else {
            return Ok(None);
        };
        let offset = (offset as usize).min(text.len());
        Ok(Some((text.len() as u64, text[offset..].to_string())))
    }

    fn read_tail(&self, path: &Path, max_bytes: u64) -> Result<String, Error> {
        let files = self.files.lock().unwrap();
        let Some(text) = files.get(path) else {
            return Ok(String::new());
        };
        let start = text.len().saturating_sub(max_bytes as usize);
        let slice = &text[start..];
        // 从行首开始，避免半行。
        let slice = match slice.find('\n') {
            Some(at) if start > 0 => &slice[at + 1..],
            _ => slice,
        };
        Ok(slice.to_string())
    }
}

//! manager 需要的端口 —— 逻辑与外界之间的唯一通道（也要能被假实现替换）。
//!
//! 每个端口都有"Fake/内存"实现（本模块）与"真实"实现（[`crate::infra`]）。
//! 管理器**不直接**碰 `/proc`、socket、文件系统：那些都在 `infra` 里，
//! 这样端口分配、reconcile、健康判定这些逻辑就能在没有网络、没有进程的测试里跑。

use std::collections::BTreeSet;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use envboard_core_api::{Error, ProcessIdentity};

/// 判断某个地址的端口是否空闲。
///
/// **语义是契约的一部分**（`core/spec/capabilities.md` §端口分配第 5 条）：
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

/// 进程表：孤儿清理与"实例是否还活着"的判据来源。
pub trait ProcessTable: Send + Sync {
    /// 读取某个 pid 的身份（启动时刻 + cmdline）。进程不在则 `None`。
    fn identity(&self, pid: i32) -> Option<ProcessIdentity>;

    /// 杀掉一个**已经确认过身份**的进程：先温和，超时再强杀。
    fn terminate(&self, identity: &ProcessIdentity) -> Result<(), Error>;
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

/// 内存进程表：显式声明"哪些进程活着"，用于精确构造 PID 复用场景。
#[derive(Debug, Default, Clone)]
pub struct MemoryProcessTable {
    alive: Vec<ProcessIdentity>,
    terminated: std::sync::Arc<std::sync::Mutex<Vec<i32>>>,
}

impl MemoryProcessTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_alive(mut self, identities: impl IntoIterator<Item = ProcessIdentity>) -> Self {
        self.alive = identities.into_iter().collect();
        self
    }

    /// 被要求终止的 pid（断言"没杀不该杀的进程"用）。
    pub fn terminated(&self) -> Vec<i32> {
        self.terminated.lock().unwrap().clone()
    }
}

impl ProcessTable for MemoryProcessTable {
    fn identity(&self, pid: i32) -> Option<ProcessIdentity> {
        self.alive
            .iter()
            .find(|identity| identity.pid == pid)
            .cloned()
    }

    fn terminate(&self, identity: &ProcessIdentity) -> Result<(), Error> {
        self.terminated.lock().unwrap().push(identity.pid);
        Ok(())
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
                    envboard_core_api::ErrorCode::NotFound,
                    format!("{} does not exist", path.display()),
                )
            })
    }
}

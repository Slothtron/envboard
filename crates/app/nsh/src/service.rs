//! 真数据的一端：把 `envboard-host` 的组合根与 `AdminService` 装成一个进程内单例，
//! 给界面线程提供**阻塞即可调用**的读写方法。
//!
//! 三件事在这里定死：
//!
//! 1. **只有本文件知道 host/admin 的存在**。`pages` / `shell` 只认 `model::Env`
//!    与 `Ctx`，换实现（假数据、只读演示）不用动界面；
//! 2. **状态目录是产品身份**：`HostOptions::app_dir_name` 取 `brand::NAME`
//!    （`envboard`），默认状态目录因此与 Tauri 侧、与将来 `nsh package` 的发行物
//!    是同一份（`%LOCALAPPDATA%\envboard`）。可用 `ENVBOARD_STATE_DIR` 覆盖；
//! 3. **异步门面不能就地 await**：`spawn` 的 future 被 NSH 引擎线程轮询，那已经在
//!    tokio 上下文里，`Handle::block_on` 会 panic。所以 `start`/`stop` 交给一条
//!    **普通 std 线程**跑 `Handle::block_on`，结果经 oneshot 回到引擎线程。

use std::sync::{Arc, OnceLock};

use envboard_admin::AdminService;
use envboard_engine::Error;
use envboard_host::{HostOptions, Prepared};
use envboard_protocol::EnvView;
use tokio::runtime::Handle;
use tokio::sync::oneshot;

/// 进程内单例（`boot()` 成功后才有）。
static SERVICE: OnceLock<Service> = OnceLock::new();

/// 启动失败的原因（`boot()` 的 `Err` 落一份在这里）。
///
/// 窗口是照常开的：没有宿主时界面不该假装“已连接”，它得把这句话写进错误槽
/// （三层反馈里最上面那层），用户才知道是账本打不开而不是环境没了。
static BOOT_ERROR: OnceLock<String> = OnceLock::new();

pub struct Service {
    /// 让 runtime 活到进程结束（放进单例，drop 就不会发生）。
    runtime: tokio::runtime::Runtime,
    admin: Arc<AdminService>,
}

/// 组合根跑完（含一次 reconcile）才算启动成功；失败就把人话错误带回去，
/// 界面照常起窗、在错误槽里显示，而不是启动即自杀。
pub fn boot() -> Result<(), Error> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("envboard-host")
        .build()
        .map_err(|error| Error::internal_error(error.to_string()))?;

    let prepared: Prepared = envboard_host::prepare(HostOptions {
        app_dir_name: Some(crate::brand::NAME.to_owned()),
        ..HostOptions::default()
    })?;

    let report = runtime.block_on(prepared.startup())?;
    log::info!(
        "host 就绪：reconcile {} 个动作，{} 条警告",
        report.actions.len(),
        report.warnings.len()
    );

    let service = Service {
        admin: Arc::clone(&prepared.admin),
        runtime,
    };
    let _ = SERVICE.set(service);
    Ok(())
}

/// 启动失败时的那句话；启动成功或还没调 `boot()` 都是 `None`。
pub fn boot_error() -> Option<&'static str> {
    BOOT_ERROR.get().map(String::as_str)
}

pub fn record_boot_error(line: String) {
    let _ = BOOT_ERROR.set(line);
}

pub fn service() -> Option<&'static Service> {
    SERVICE.get()
}

/// 一次快照：账本代次 + 全量环境。同步（管理器内存态），引擎线程直接调不阻塞。
pub fn snapshot() -> Result<(u64, Vec<EnvView>), Error> {
    match service() {
        Some(s) => s.admin.snapshot(),
        None => Err(Error::internal_error("宿主尚未启动")),
    }
}

pub fn list() -> Result<Vec<EnvView>, Error> {
    match service() {
        Some(s) => s.admin.list(),
        None => Err(Error::internal_error("宿主尚未启动")),
    }
}

pub fn create(body: serde_json::Value) -> Result<EnvView, Error> {
    match service() {
        Some(s) => s.admin.create(&body),
        None => Err(Error::internal_error("宿主尚未启动")),
    }
}

pub fn remove(name: &str) -> Result<(), Error> {
    match service() {
        Some(s) => s.admin.remove(name),
        None => Err(Error::internal_error("宿主尚未启动")),
    }
}

pub fn reallocate_port(name: &str) -> Result<EnvView, Error> {
    match service() {
        Some(s) => s.admin.reallocate_port(name),
        None => Err(Error::internal_error("宿主尚未启动")),
    }
}

/// 异步动作的落点：普通线程 + `Handle::block_on` + oneshot。
///
/// 线程只活到这次动作结束（一秒级的 start/stop），不是线程池——M3 的写入频率
/// 是个位数每分钟，不值得引入调度设施。
fn off_engine<F, T>(future: F) -> oneshot::Receiver<Result<T, Error>>
where
    F: std::future::Future<Output = Result<T, Error>> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    let handle: Handle = match service() {
        Some(s) => s.runtime.handle().clone(),
        None => {
            let _ = tx.send(Err(Error::internal_error("宿主尚未启动")));
            return rx;
        }
    };
    std::thread::spawn(move || {
        let out = handle.block_on(future);
        let _ = tx.send(out);
    });
    rx
}

pub fn start(name: String) -> oneshot::Receiver<Result<EnvView, Error>> {
    let admin = service().map(|s| Arc::clone(&s.admin));
    off_engine(async move {
        match admin {
            Some(admin) => admin.start(&name).await,
            None => Err(Error::internal_error("宿主尚未启动")),
        }
    })
}

pub fn stop(name: String) -> oneshot::Receiver<Result<EnvView, Error>> {
    let admin = service().map(|s| Arc::clone(&s.admin));
    off_engine(async move {
        match admin {
            Some(admin) => admin.stop(&name).await,
            None => Err(Error::internal_error("宿主尚未启动")),
        }
    })
}

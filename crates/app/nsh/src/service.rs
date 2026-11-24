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

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use envboard_admin::AdminService;
use envboard_engine::Error;
use envboard_host::{CaControl, HostOptions, Prepared, StorePresence};
use envboard_protocol::{CaptureRows, DebugStartReq, EnvCreateReq, EnvView, Record};
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
    /// 共享 CA 的操作面（证书页唯一的数据来源）。`None` = 本宿主没有物化 CA。
    ca: Option<CaControl>,
    /// 界面偏好文件（主题）。组合根交付路径，nsh 不自己拼 `state_dir/ui.json`。
    ui_prefs: PathBuf,
    /// CA 材料目录（证书页「文件路径」指向它）。
    ca_dir: PathBuf,
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
        ca: prepared.ca.clone(),
        ui_prefs: prepared.ui_prefs_file.clone(),
        ca_dir: prepared.ca_dir.clone(),
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

pub fn create(request: &EnvCreateReq) -> Result<EnvView, Error> {
    match service() {
        Some(s) => s.admin.create(request),
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

/* ---------------- 抓包（EB-DESIGN-004：列表读瘦行，详情单独取） ---------------- */

/// 抓包列表的一拍瘦行（尾部窗口，按 request_id 升序；不搬正文）。
/// `None` = 该环境没在跑/无会话。
pub fn capture_rows(name: &str, limit: usize) -> Result<Option<CaptureRows>, Error> {
    match service() {
        Some(s) => s.admin.capture_rows(name, limit),
        None => Err(Error::internal_error("宿主尚未启动")),
    }
}

/// 单条抓包详情（完整记录本体，含 headers 与正文三形状）。
///
/// 拿到的是 `Arc<Record>` —— 引擎缓冲里那一份的句柄，不做深拷贝。
pub fn capture_detail(name: &str, request_id: u64) -> Result<Option<Arc<Record>>, Error> {
    match service() {
        Some(s) => s.admin.capture_detail(name, request_id),
        None => Err(Error::internal_error("宿主尚未启动")),
    }
}

/// 详情页用的异步形态：`Body` 的文本化（BASE64 解码 / JSON 重排）在线程上做，
/// 让点击那一帧先把骨架画出来（ADR-8「点行 ≤200ms 出骨架」）。
pub fn capture_detail_async(
    name: String,
    request_id: u64,
) -> oneshot::Receiver<Result<Option<Arc<Record>>, Error>> {
    off_thread(move || capture_detail(&name, request_id))
}


/// 开启抓包：工作区级单例会话切到这个环境（环境得先在跑）。
pub fn start_debug(name: &str) -> Result<envboard_protocol::DebugView, Error> {
    match service() {
        Some(s) => s.admin.start_debug(&DebugStartReq {
            env: name.to_owned(),
        }),
        None => Err(Error::internal_error("宿主尚未启动")),
    }
}

/// 停止抓包（会话保留，可再开启）。
pub fn stop_debug() -> Result<bool, Error> {
    match service() {
        Some(s) => s.admin.stop_debug(),
        None => Err(Error::internal_error("宿主尚未启动")),
    }
}

/// 清空抓包缓冲（不停止会话）。
pub fn clear_capture(name: &str) -> Result<bool, Error> {
    match service() {
        Some(s) => s.admin.clear_capture(name),
        None => Err(Error::internal_error("宿主尚未启动")),
    }
}

/// 一份导出的结果：文件落在哪、装了多少条。
pub struct HarExport {
    pub path: PathBuf,
    pub entries: usize,
}

/// 导出当前会话为 HAR 1.2：拉全部记录（含正文）→ 拼 HAR → 写进下载目录。
///
/// 记录是几百 MB 级的 `Value`（256 MiB/环境缓冲 + 序列化），所以整段放普通线程跑；
/// 文件名对齐桌面形态（`<env>-session-<id>.har`），重名不覆盖。
pub fn export_har(name: String) -> oneshot::Receiver<Result<HarExport, Error>> {
    off_thread(move || {
        let s = service().ok_or_else(|| Error::internal_error("宿主尚未启动"))?;
        let view = s
            .admin
            .captures(&name, usize::MAX)?
            .ok_or_else(|| Error::internal_error(format!("{name} 没有进行中的抓包会话")))?;
        let har = crate::model::har::har(&name, &view.records);
        let entries = har
            .get("log")
            .and_then(|log| log.get("entries"))
            .and_then(|entries| entries.as_array())
            .map(Vec::len)
            .unwrap_or(0);
        let text = serde_json::to_string_pretty(&har)
            .map_err(|error| Error::internal_error(format!("HAR 序列化失败：{error}")))?;
        let stem = format!("{name}-session-{}", view.session.id);
        let path = crate::export::write_unique(&crate::export::downloads_dir(), &stem, "har", &text)?;
        Ok(HarExport { path, entries })
    })
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

// ---- 界面偏好（主题） ------------------------------------------------------

/// `ui.json` 的路径（组合根交付的那一份）。没有宿主时 `None`。
pub fn ui_prefs_file() -> Option<&'static Path> {
    service().map(|s| s.ui_prefs.as_path())
}

/// 引擎核的版本（设置框「关于」里那一行）。
pub fn core_version() -> Option<(String, String)> {
    service().map(|s| {
        let core = s.admin.core_info();
        (core.name, core.version)
    })
}

/// CA 材料目录（设置框「关于」里指出证书文件在哪一层）。
pub fn ca_dir() -> Option<&'static std::path::Path> {
    service().map(|s| s.ca_dir.as_path())
}

// ---- 共享 CA（证书页） -----------------------------------------------------

/// 本宿主有没有物化 CA。没有（测试注入的假引擎）时证书页显示「不可用」，
/// 而不是把四个空值排成一张表。
pub fn ca_available() -> bool {
    service().is_some_and(|s| s.ca.is_some())
}

fn presence(p: StorePresence) -> crate::model::Trust {
    match p {
        StorePresence::Installed => crate::model::Trust::Installed,
        StorePresence::Missing => crate::model::Trust::Missing,
        StorePresence::Unknown => crate::model::Trust::Unknown,
    }
}

/// 读一屏证书页的数据。**会碰系统证书存储**（`trust()` 是 Win32 探测），
/// 所以调用方要用 `background(...)` 放到工作线程上，别占渲染线程。
pub fn ca_snapshot() -> Result<crate::model::Ca, Error> {
    let s = service().ok_or_else(|| Error::internal_error("宿主尚未启动"))?;
    let control = s
        .ca
        .as_ref()
        .ok_or_else(|| Error::internal_error("本宿主没有共享 CA（假引擎装配）"))?;
    let trust = control.trust();
    // `info()` 是证书 DER 的结构摘要（`CertInfo` 的 JSON 形态）：字段读不全就留空，
    // 页面显示「—」，不猜。
    let info = control.info().unwrap_or(serde_json::Value::Null);
    let text = |key: &str| {
        info.get(key)
            .and_then(|v| v.as_str())
            .map(str::to_owned)
    };
    let pubkey = match (text("pubkey_alg"), text("pubkey_curve"), info.get("pubkey_bits")) {
        (Some(alg), Some(curve), _) => Some(format!("{alg} · {curve}")),
        (Some(alg), None, Some(bits)) => bits
            .as_u64()
            .map(|bits| format!("{alg} {bits}")),
        (Some(alg), None, None) => Some(alg),
        _ => None,
    };
    Ok(crate::model::Ca {
        user: presence(trust.current_user),
        machine: presence(trust.machine),
        detail: trust.detail,
        common_name: text("common_name"),
        organization: text("organization"),
        serial_hex: text("serial_hex"),
        not_before: text("not_before"),
        not_after: text("not_after"),
        sig_alg: text("sig_alg"),
        pubkey,
        fingerprint: control.fingerprint(),
        cert_path: control.cert_path().display().to_string(),
    })
}

/// 把当前 CA 装进**当前用户** ROOT 存储（免管理员、幂等）。
///
/// 走普通线程：系统证书库的写入是 Win32 调用，几百毫秒级，不能压在渲染线程上。
pub fn ca_install() -> oneshot::Receiver<Result<(), Error>> {
    let ca = service().and_then(|s| s.ca.clone());
    off_thread(move || match ca {
        Some(control) => control.install_trust(),
        None => Err(Error::internal_error("本宿主没有共享 CA（假引擎装配）")),
    })
}

/// 从**当前用户** ROOT 存储卸载当前 CA。与桌面形态同一语义：只处理当前用户，
/// 计算机存储里若还有一份只**报告**、不代删（提权助手不在本版本范围内）。
pub fn ca_uninstall() -> oneshot::Receiver<Result<crate::model::Uninstall, Error>> {
    let ca = service().and_then(|s| s.ca.clone());
    off_thread(move || {
        let control = ca.ok_or_else(|| Error::internal_error("本宿主没有共享 CA（假引擎装配）"))?;
        let trust = control.trust();
        let removed = if trust.current_user == StorePresence::Installed {
            control.uninstall_trust()?
        } else {
            false
        };
        let warning = (trust.machine == StorePresence::Installed).then(|| {
            "计算机存储里仍有一份旧证书；本版本只管理当前用户存储，需要时请在系统证书管理器里手动移除"
                .to_owned()
        });
        Ok(crate::model::Uninstall { removed, warning })
    })
}

/// 阻塞动作的落点：**普通线程**跑，结果经 oneshot 回到引擎线程。
///
/// 与 [`off_engine`] 同一形状，只是这里跑的是同步的 Win32 读写（证书库、
/// 系统偏好），不需要 tokio 上下文。
fn off_thread<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> oneshot::Receiver<Result<T, Error>> {
    let (tx, rx) = oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    rx
}

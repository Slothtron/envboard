//! N 线（单实例，方案 §5）：同一 state-dir 只允许一个写者，第二个进程在 `boot`
//! **之前**就给出结论 —— 而不是先撞 `InstanceLock`，把一句 `conflict` 甩给一个
//! 还没有窗口的进程（用户看到的是「双击没反应」）。
//!
//! N1 只判「谁是主」；N2 补上握手、调用方校验与前台化：
//!
//! 通道是 `\\.\pipe\envboard-nsh-activate-<FNV1a(state_dir)>`（`Local\` 作用域，
//! 前缀带 `nsh`，与旧 Tauri 版的 `envboard-activate-` 不同名——两版共用账本但
//! 激活通道各自独立，旧版占锁时由 `lock_held_message` 给出人话提示）。主实例以
//! `FILE_FLAG_FIRST_PIPE_INSTANCE` + owner-only DACL 建
//! 命名管道；次实例连上它握手，被接受就 `exit 0`，主实例把窗口拉到前台。
//!
//! **为什么不是内核事件**：事件不携带调用方身份，同会话任意进程都能 `SetEvent`
//! 触发开窗（SOP §6 的反模式）。管道能在服务端用 `GetNamedPipeClientProcessId`
//! 与 `QueryFullProcessImageNameW` 取调用方 exe 路径，与本程序比对，**只有同一个
//! 可执行文件才被接受**；owner-only DACL 再挡跨用户。握手是双向的：client 写完
//! 请求后**保持连接等回执**（保证服务端取 PID 时 client 仍活着，避开 PID 复用），
//! 服务端写完回执后等 client 关闭再 `DisconnectNamedPipe`（避免回执被丢弃）。
//!
//! **payload 纪律**：只放激活意图（`action`，将来可带「打开到哪一页」），任何
//! env / capture 查询都不许从这里进——那是门面的事。
//!
//! 非 Windows 上整套退化为 `Unavailable`，行为回到「锁被占就报错退出」。

use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

pub use imp::{
    Activated, Guard, Preflight, activation_failed_message, lock_held_message, preflight, prompt,
    spawn_waiter,
};

/// 管道名前缀。与旧 Tauri 版的 `envboard-activate-` 不同名（历史身份，不改）。
const PIPE_PREFIX: &str = "envboard-nsh-activate-";

/// 握手字节：client → server 的请求是 `[MAGIC, VERSION, action]`，
/// server → client 的应答是单字节 `RESP_ACCEPT` / `RESP_REJECT`。
const MAGIC: u8 = 0x4e; // 'N'
const PROTO_VERSION: u8 = 2;
const ACTION_OPEN: u8 = 1;
const RESP_ACCEPT: u8 = 1;
const RESP_REJECT: u8 = 0;
/// client 读完回执后的确认字节。服务端收到它才 `DisconnectNamedPipe`：
/// 断得太早，回执会跟着连接一起没掉。
const RESP_ACK: u8 = 0x06;

/// 待处理的激活次数（等待线程 `fetch_add`，UI 线程 `swap(0)` 消费）。
///
/// 为什么是计数而不是回调：命名管道在 std 线程里读写，而 NSH 的窗口操作要走
/// 组件作用域的命令队列（`main_window().show()/focus()` 只能在 UI 线程发命令）。
static PENDING: AtomicU32 = AtomicU32::new(0);

/// B3（EB-DESIGN-006 §6.3）：等待线程 → UI 的**推送**桥，取代 120ms 轮询。
///
/// 能这么桥的依据：NSH 任务的 waker（`nsh-core/src/element/task.rs:39-60` 的
/// `TaskWaker`）持 `Arc<Ready>`（`Mutex<Vec<u64>>` + `Mutex<Option<Notify>>`），
/// `wake_by_ref` 可从**任意线程**调用 —— `background()` 就是这么把 tokio 线程上
/// 完成的活唤回引擎线程的。所以引擎线程上的任务可以直接 `await` 一个 tokio
/// 通道，等待线程 `send` 即唤醒，**空闲期零唤醒**（旧口径是 8.3 次/秒）。
static ACTIVATE: OnceLock<tokio::sync::mpsc::UnboundedSender<()>> = OnceLock::new();

fn bump_pending() {
    PENDING.fetch_add(1, Ordering::Release);
    if let Some(tx) = ACTIVATE.get() {
        // 收端已经没了（应用正在退）：丢弃即可，计数还留着给测试读。
        let _ = tx.send(());
    }
}

/// UI 线程调用：取走至今累计的激活请求数（0 表示没有）。
pub fn pending_activations() -> u32 {
    PENDING.swap(0, Ordering::AcqRel)
}

/// 测试用：直接投一次激活请求（不经过管道）。
#[cfg(test)]
fn queue_for_test() {
    bump_pending();
}

/// 按 state-dir 派生管道名（纯函数，单测钉住）。
///
/// Windows 路径大小写不敏感：先折小写再哈希；哈希成十六进制，避免路径里的 `\`
/// 被当成管道名的层级分隔符。
pub fn pipe_name(state_dir: &Path) -> String {
    let normalized = state_dir.to_string_lossy().to_lowercase();
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in normalized.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("\\\\.\\pipe\\{PIPE_PREFIX}{hash:016x}")
}

/// 两个 exe 路径是否指向同一个可执行文件（纯函数，单测钉住）。
///
/// Windows 路径大小写不敏感；两侧都先 `canonicalize`（调用方负责），这里只做
/// 「折小写后逐字比较」。`canonicalize` 会给出 `\\?\` 扩展形式，去不掉的前缀
/// 差异也一并折掉，省得 `C:\x` 与 `\\?\C:\x` 被当成两个程序。
pub fn same_executable(a: &Path, b: &Path) -> bool {
    fn normalize(path: &Path) -> String {
        let text = path.to_string_lossy().to_lowercase();
        text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned()
    }
    normalize(a) == normalize(b)
}

/// UI 侧安装：等激活请求推过来，把窗口拉到前台。在组件作用域里调一次
/// （`shell.rs` 的 `App()`）。非 Windows 上什么也不做。
///
/// B3：不再每 120ms 问一次。等待线程 `bump_pending()` 时直接往通道里投一条，
/// 这里的任务 `await` 在通道上 —— **没有激活请求时零唤醒**。
#[cfg(windows)]
pub fn install() {
    use std::time::Duration;

    use nsh_core::element::desktop::main_window;
    use nsh_core::prelude::{spawn, timer};

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    if ACTIVATE.set(tx).is_err() {
        // 已经装过一次（不该发生）：不重复起任务，避免两条循环抢焦点。
        log::warn!("单实例：激活通道已安装，跳过");
        return;
    }

    spawn(async move {
        while rx.recv().await.is_some() {
            // 一次唤出合并掉积压的请求：用户连点三次图标只要上来一次。
            while rx.try_recv().is_ok() {}
            let _ = pending_activations();

            // focus() 的文档承诺「显示 + 到本层最前 + 聚焦」，最小化与隐藏
            // 都由它一并处理（`show()` 也会 restore）。这里不自己拼
            // ShowWindow/SetForegroundWindow：窗口归 NSH 管，绕过去会让它的
            // 内部状态与平台真值分叉。
            main_window().focus();
            log::info!("单实例：已唤出主窗口");

            // 焦点校验：`focus()` 之后隔一拍再读 `state()`——真机上
            // `WindowRef::state()` 读的是上一拍（T1 实测），立刻读必然读到旧值。
            // **这个 timer 只在真收到激活时存在**，空闲期没有它。
            timer(Duration::from_millis(300)).await;
            let focused = main_window().state().map(|s| s.focused).unwrap_or(false);
            if !focused {
                // SetForegroundWindow 在真机常被前台锁挡住（方案 §5.2 已写
                // 明）。退一步用系统级的「要注意力」：任务栏按钮闪，用户一点
                // 就上来，不抢焦点。
                main_window().request_attention(true);
                log::info!("单实例：聚焦未生效，改请任务栏闪烁");
            }
            while rx.try_recv().is_ok() {}
        }
    });
}

#[cfg(not(windows))]
pub fn install() {}

#[cfg(test)]
mod tests {
    use super::pipe_name;
    use std::path::Path;

    #[test]
    fn same_state_dir_yields_the_same_pipe() {
        let a = pipe_name(Path::new(r"C:\Users\x\AppData\Local\envboard"));
        let b = pipe_name(Path::new(r"C:\Users\x\AppData\Local\envboard"));
        assert_eq!(a, b);
    }

    #[test]
    fn pipe_name_is_case_insensitive() {
        // Windows 上同一目录的大小写变体必须落到同一条管道，否则两个实例互不认识。
        let lower = pipe_name(Path::new(r"c:\users\x\envboard-dev"));
        let upper = pipe_name(Path::new(r"C:\Users\X\ENVBOARD-DEV"));
        assert_eq!(lower, upper);
    }

    #[test]
    fn different_state_dirs_yield_different_pipes() {
        let a = pipe_name(Path::new(r"C:\Users\x\envboard"));
        let b = pipe_name(Path::new(r"C:\Users\x\envboard-portable"));
        assert_ne!(a, b);
    }

    #[test]
    fn pipe_name_is_well_formed() {
        let name = pipe_name(Path::new(r"C:\Users\x\envboard"));
        assert!(
            name.starts_with(r"\\.\pipe\envboard-nsh-activate-"),
            "{name}"
        );
        // 十六位小写十六进制，且不含路径分隔符（`\` 会破坏管道名的层级）。
        let suffix = name.trim_start_matches(r"\\.\pipe\envboard-nsh-activate-");
        assert_eq!(suffix.len(), 16, "{name}");
        assert!(
            suffix
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn activation_failed_message_names_the_reason() {
        let message = super::activation_failed_message("已有实例，但 1.5s 内没有应答");
        assert!(message.contains("已在运行"), "{message}");
        assert!(message.contains("1.5s"), "{message}");
    }

    #[test]
    fn same_executable_ignores_case_and_extended_prefix() {
        assert!(super::same_executable(
            Path::new(r"C:\Apps\envboard-nsh.exe"),
            Path::new(r"c:\apps\ENVBOARD-NSH.EXE")
        ));
        assert!(super::same_executable(
            Path::new(r"\\?\C:\Apps\envboard-nsh.exe"),
            Path::new(r"C:\Apps\envboard-nsh.exe")
        ));
        assert!(!super::same_executable(
            Path::new(r"C:\Apps\envboard-nsh.exe"),
            Path::new(r"C:\Apps\envboard-nsh-copy.exe")
        ));
    }

    #[test]
    fn pending_activations_are_consumed_once() {
        super::queue_for_test();
        super::queue_for_test();
        assert_eq!(super::pending_activations(), 2);
        assert_eq!(super::pending_activations(), 0);
    }

    /// 真实管道走一遍（Windows）：主实例 → 次实例被接受 → 异 dir 互不影响 →
    /// 主实例句柄关闭后管道消失、能重新当主。
    #[cfg(windows)]
    #[test]
    fn owner_accepts_a_same_executable_client_and_frees_after_drop() {
        use super::Preflight;
        use super::imp::preflight_at;

        let stamp = std::process::id();
        let dir = std::env::temp_dir().join(format!("eb-n2-pipe-{stamp}-a"));
        let other_dir = std::env::temp_dir().join(format!("eb-n2-pipe-{stamp}-b"));

        let first = preflight_at(&dir);
        match &first {
            Preflight::Owner(guard) => super::spawn_waiter(guard),
            _ => panic!("首次 claim 应当是主实例"),
        }

        // 次实例（另一个线程）连上来：调用方就是本 exe，必须被接受。
        let activated = super::imp::activate_at(&dir, 2000);
        assert!(
            matches!(activated, super::Activated::Accepted),
            "同 exe 的次实例应当被接受，实际 {activated:?}"
        );
        // 激活计数由等待线程在断开之后累加，可能比 client 拿到回执晚一点：
        // 给 1s 的窗口，取到即止（`pending_activations` 是 swap，不能重复读）。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let mut seen = 0;
        while seen == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(25));
            seen = super::pending_activations();
        }
        assert!(seen >= 1, "接受后要给 UI 留一次激活");

        // 异 dir 互不影响：另一条管道同样能当主。
        assert!(matches!(preflight_at(&other_dir), Preflight::Owner(_)));

        // 主实例退出 = 句柄关闭 = 管道对象消失：同一 dir 立刻能重新当主。
        drop(first);
        assert!(matches!(preflight_at(&dir), Preflight::Owner(_)));
    }

    /// 真实管道走一遍（Windows）：管道在、但没人应答（主实例被挂起的情形）——
    /// 次实例必须在预算内报「无响应」，不能无限挂死。
    #[cfg(windows)]
    #[test]
    fn a_client_times_out_when_nobody_answers() {
        use super::Activated;

        let stamp = std::process::id();
        let dir = std::env::temp_dir().join(format!("eb-n2-pipe-{stamp}-mute"));

        // 只建管道、不起等待线程：client 的请求会写进缓冲，但永远等不到回执。
        let owner = super::imp::preflight_at(&dir);
        assert!(matches!(owner, super::Preflight::Owner(_)), "应当能建管道");

        let started = std::time::Instant::now();
        let outcome = super::imp::activate_at(&dir, 500);
        let elapsed = started.elapsed();
        assert!(
            matches!(outcome, Activated::NoResponse(_)),
            "无人应答时必须报 NoResponse，实际 {outcome:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(3),
            "无响应预算不该超过 3s，实际 {elapsed:?}"
        );
        // owner 的句柄活到测试结束（提前关掉会让管道在断言前消失）。
        drop(owner);
    }
}

#[cfg(windows)]
mod imp {
    use std::mem::size_of;
    use std::path::{Path, PathBuf};
    use std::ptr::{null, null_mut};
    use std::time::{Duration, Instant};

    use envboard_host::HostOptions;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY,
        ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE,
        INVALID_HANDLE_VALUE, LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile,
        WriteFile,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
        GetNamedPipeServerProcessId, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE, PIPE_WAIT, PeekNamedPipe, WaitNamedPipeW,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MessageBoxW,
    };

    /// 次实例等回执的预算：主实例被挂起时要在这里报「无响应」（方案 §5.3
    /// 验收要求 3s 内给结论）。
    const CLIENT_RESPONSE_TIMEOUT: Duration = Duration::from_millis(1500);
    /// 连不上时最多等主实例空出一个实例位的时间。
    const CLIENT_CONNECT_TIMEOUT: Duration = Duration::from_millis(2000);
    /// 服务端等一个 client 把请求写完的时间：防止连上不说话的进程把等待线程
    /// 永久占住（那会让后续所有激活都排队到超时）。
    const SERVER_REQUEST_TIMEOUT: Duration = Duration::from_millis(1200);
    /// 回执发出后等 client 关闭的时间（`DisconnectNamedPipe` 会丢弃未读数据，
    /// 所以要等它读完再断）。
    const SERVER_DRAIN_TIMEOUT: Duration = Duration::from_millis(600);
    const PEEK_INTERVAL: Duration = Duration::from_millis(25);

    /// 主实例的管道句柄。**必须活到进程结束**：句柄一关，管道对象就消失，
    /// 下一个进程会把自己当成主实例。
    ///
    /// 句柄的关闭权归属分两种情况，这不是洁癖：**另一个线程正卡在
    /// `ConnectNamedPipe` 时，本线程 `CloseHandle` 会一直阻塞**（实测），
    /// 那么「起了等待线程又提前退出」就变成「进程不退出」。
    ///
    /// - 没起等待线程：`Drop` 自己关句柄（`handle` 非 0）。
    /// - 起了等待线程：句柄交给线程（`handle` 归 0），`Drop` 只负责叫停、
    ///   把卡在连接里的线程叫醒、等它关句柄。
    pub struct Guard {
        /// 句柄值；交给等待线程之后置 0。
        handle: std::sync::atomic::AtomicIsize,
        /// 管道名：`Drop` 用它自连一次叫醒等待线程。
        pipe: String,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        waiter: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
    }

    impl Guard {
        fn new(handle: HANDLE, pipe: String) -> Self {
            Self {
                handle: std::sync::atomic::AtomicIsize::new(handle as isize),
                pipe,
                stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                waiter: std::sync::Mutex::new(None),
            }
        }

        /// 把句柄挪出来交给等待线程（只成功一次）。
        fn take(&self) -> Option<HANDLE> {
            let raw = self.handle.swap(0, std::sync::atomic::Ordering::AcqRel);
            (raw != 0).then_some(raw as HANDLE)
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::Release);
            let raw = self.handle.swap(0, std::sync::atomic::Ordering::AcqRel);
            if raw != 0 {
                // SAFETY: 没有等待线程，句柄由本结构独占，只在这里关一次。
                unsafe { CloseHandle(raw as HANDLE) };
                return;
            }
            // 等待线程可能正卡在 `ConnectNamedPipe`：自连一次让它醒过来。
            // 它看到 stop 就会关句柄退出（所以这里不关，交给它关）。
            wake_pending_connect(&self.pipe);
            if let Ok(mut slot) = self.waiter.lock()
                && let Some(join) = slot.take()
            {
                let _ = join.join();
            }
        }
    }

    pub enum Preflight {
        /// 本进程是主实例：句柄要一直活到 `run_app` 结束。
        Owner(Guard),
        /// 已有实例接受了激活：调用方应立刻 `exit 0`。
        Activated,
        /// 管道在，但调用方不是本程序：`exit` 非 0。
        Rejected(String),
        /// 管道在，但主实例没应答（被挂起 / 刚崩在握手里）：`exit` 非 0。
        NoResponse(String),
        /// 机制不可用（解析不出 state-dir、建管道报别的错、非 Windows）：
        /// 照常启动，互斥仍由 `InstanceLock` 兜底。
        Unavailable { reason: String },
    }

    /// 次实例看到的四种结局（`preflight` 的内部形态，单测直接断言它）。
    #[derive(Debug)]
    pub enum Activated {
        /// 管道不存在：本进程可以当主实例。
        Absent,
        Accepted,
        Rejected(String),
        NoResponse(String),
    }

    /// 与宿主同一条回退链：`ENVBOARD_STATE_DIR` → 便携标记 → 平台默认目录。
    pub fn state_dir() -> Option<PathBuf> {
        HostOptions {
            app_dir_name: Some(crate::brand::NAME.to_owned()),
            ..HostOptions::default()
        }
        .resolve_state_dir()
        .ok()
    }

    pub fn preflight() -> Preflight {
        let Some(dir) = state_dir() else {
            return Preflight::Unavailable {
                reason: "解析不出 state-dir".to_owned(),
            };
        };
        preflight_at(&dir)
    }

    /// 给定 state-dir 走一次 preflight。拆出来是为了让单测能用临时目录。
    ///
    /// 先当次实例试握手：管道在就说明有人是主。**只有** `FILE_NOT_FOUND`
    /// （管道不存在）才轮到我们当主——`FIRST_PIPE_INSTANCE` 让「谁是主」没有
    /// 中间态，避开「主实例正在 boot、管道还没建好」的窗口期。
    pub fn preflight_at(dir: &Path) -> Preflight {
        match activate_at(dir, CLIENT_CONNECT_TIMEOUT.as_millis() as u32) {
            Activated::Accepted => Preflight::Activated,
            Activated::Rejected(why) => Preflight::Rejected(why),
            Activated::NoResponse(why) => Preflight::NoResponse(why),
            Activated::Absent => {
                let pipe = crate::single_instance::pipe_name(dir);
                let wide = to_wide(&pipe);
                // SAFETY: wide 在本函数内有效；失败时句柄无效。
                let handle = unsafe { create_owner_pipe(&wide) };
                if handle != INVALID_HANDLE_VALUE {
                    return Preflight::Owner(Guard::new(handle, pipe));
                }
                // SAFETY: 紧随 CreateNamedPipeW 失败之后取错误码。
                let err = unsafe { GetLastError() };
                // 两种都是「刚被别人抢先建了」：跨进程是 ACCESS_DENIED
                // （FIRST_PIPE_INSTANCE 的语义），同进程 maxInstances=1 用满是
                // PIPE_BUSY（实测 231）。回到 client 路径再试一次。
                if err == ERROR_ACCESS_DENIED || err == ERROR_PIPE_BUSY {
                    return match activate_at(dir, CLIENT_CONNECT_TIMEOUT.as_millis() as u32) {
                        Activated::Accepted => Preflight::Activated,
                        Activated::Rejected(why) => Preflight::Rejected(why),
                        Activated::NoResponse(why) => Preflight::NoResponse(why),
                        Activated::Absent => Preflight::Unavailable {
                            reason: "管道刚出现又消失".to_owned(),
                        },
                    };
                }
                Preflight::Unavailable {
                    reason: format!("CreateNamedPipeW 失败（error {err}）"),
                }
            }
        }
    }

    /// 次实例路径：连管道、握手、等回执。`connect_budget` 是「等主实例空出
    /// 一个实例位」的上限。
    pub fn activate_at(dir: &Path, connect_budget_ms: u32) -> Activated {
        let pipe = crate::single_instance::pipe_name(dir);
        let wide = to_wide(&pipe);
        let deadline = Instant::now() + Duration::from_millis(u64::from(connect_budget_ms));

        loop {
            // SAFETY: wide 在本函数内有效；句柄由我们关闭。
            let client = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    null(),
                    OPEN_EXISTING,
                    0,
                    null_mut(),
                )
            };
            if client != INVALID_HANDLE_VALUE {
                let outcome = handshake(client);
                // SAFETY: client 是本函数刚打开的句柄。
                unsafe { CloseHandle(client) };
                return outcome;
            }

            // SAFETY: 紧随 CreateFileW 失败之后取错误码。
            let err = unsafe { GetLastError() };
            if err == ERROR_FILE_NOT_FOUND {
                return Activated::Absent;
            }
            if err == ERROR_PIPE_BUSY && Instant::now() < deadline {
                // 主实例正在 Disconnect→Connect 的间隙：等一个实例位空出来。
                // SAFETY: wide 有效；超时只影响等待时长。
                unsafe { WaitNamedPipeW(wide.as_ptr(), 200) };
                continue;
            }
            if err == ERROR_PIPE_BUSY {
                return Activated::NoResponse(
                    "已有实例，但 2s 内没有空出连接（可能被挂起）".to_owned(),
                );
            }
            return Activated::NoResponse(format!("连接激活管道失败（error {err}）"));
        }
    }

    /// client 侧握手：拿主实例 PID 交给 `AllowSetForegroundWindow`（否则主实例
    /// 即使收到请求也常抢不到前台），写请求，等回执。
    fn handshake(client: HANDLE) -> Activated {
        let mut server_pid = 0u32;
        // SAFETY: client 是已连接的管道句柄，server 正在等请求。
        let have_server = unsafe { GetNamedPipeServerProcessId(client, &mut server_pid) } != 0;
        if have_server {
            // 把「允许设置前台窗口」的权利让给主实例：这是 cross-process 前台
            // 化的必要一步，缺了它 SetForegroundWindow 基本必然失败。
            // SAFETY: 只传 PID，没有指针。
            unsafe { AllowSetForegroundWindow(server_pid) };
        }

        let request = [
            crate::single_instance::MAGIC,
            crate::single_instance::PROTO_VERSION,
            crate::single_instance::ACTION_OPEN,
        ];
        let mut written = 0u32;
        // SAFETY: request/client 在本函数内有效。
        let wrote = unsafe {
            WriteFile(
                client,
                request.as_ptr(),
                request.len() as u32,
                &mut written,
                null_mut(),
            )
        };
        if wrote == 0 || written as usize != request.len() {
            return Activated::NoResponse("写入激活请求失败（主实例可能已退出）".to_owned());
        }

        // 等回执：用 PeekNamedPipe 轮询而不是阻塞 ReadFile —— 主实例被挂起时
        // 阻塞读会永远挂在那里，而这里必须到点就报无响应。
        let deadline = Instant::now() + CLIENT_RESPONSE_TIMEOUT;
        loop {
            match peek(client) {
                Some(available) if available > 0 => {
                    let mut resp = 0u8;
                    let mut read = 0u32;
                    // SAFETY: client 已连接，缓冲区在本帧内有效。
                    let got = unsafe { ReadFile(client, &mut resp, 1, &mut read, null_mut()) };
                    if got == 0 || read != 1 {
                        return Activated::NoResponse("读回执失败".to_owned());
                    }
                    // 回一个 ACK：告诉服务端「回执收到了，可以断」。服务端
                    // 早断会把这一字节丢掉（`DisconnectNamedPipe` 不保证送达）。
                    let ack = [crate::single_instance::RESP_ACK];
                    let mut sent = 0u32;
                    // SAFETY: 同上，单字节缓冲区在本帧内有效。
                    unsafe { WriteFile(client, ack.as_ptr(), 1, &mut sent, null_mut()) };
                    if resp == crate::single_instance::RESP_ACCEPT {
                        return Activated::Accepted;
                    }
                    return Activated::Rejected(
                        "主实例拒绝激活：调用方不是同一个可执行文件".to_owned(),
                    );
                }
                Some(_) => {}
                None => return Activated::NoResponse("读回执失败".to_owned()),
            }
            if Instant::now() >= deadline {
                return Activated::NoResponse(
                    "已有实例，但 1.5s 内没有应答（可能被挂起）".to_owned(),
                );
            }
            std::thread::sleep(PEEK_INTERVAL);
        }
    }

    /// 自连一次主实例自己的管道：只为把卡在 `ConnectNamedPipe` 的线程叫醒。
    fn wake_pending_connect(pipe_name: &str) {
        let wide = to_wide(pipe_name);
        // SAFETY: wide 在本函数内有效；连不上就算了（线程依旧会因 stop 退出）。
        let client = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        if client != INVALID_HANDLE_VALUE {
            // SAFETY: 刚打开的句柄，立刻关掉即可（不握手、不读）。
            unsafe { CloseHandle(client) };
        }
    }

    /// 主实例的等待线程：一个 client 一个轮回，直到进程退出或 `Guard` 被丢。
    ///
    /// 在 `preflight` 之后立刻起（不等窗口）：这样次实例在开机自启/双击撞上
    /// 「主实例正在 boot」时也能马上拿到回执，而不是傻等到窗口出来。真正的
    /// 开窗由 UI 线程的任务等 `ACTIVATE` 通道推送完成（B3，不再轮询）。
    pub fn spawn_waiter(guard: &Guard) {
        /// 管道句柄不实现 `Send`（它是裸指针），包一层交出去。
        /// # Safety
        /// 句柄本身可以在任意线程用（`ConnectNamedPipe` 是线程无关的），
        /// 并且自此只由这条等待线程使用（主线程那份已置 0）。
        struct SendPipe(HANDLE);
        // SAFETY: 见上。
        unsafe impl Send for SendPipe {}
        impl SendPipe {
            /// 经方法调用拿句柄：Rust 2021 的闭包按**字段**捕获，直接写 `pipe.0`
            /// 会把裸 `*mut c_void` 捕进线程（那就不是 `Send` 了）。
            fn raw(&self) -> HANDLE {
                self.0
            }
        }

        let Some(raw) = guard.take() else {
            return;
        };
        let pipe = SendPipe(raw);
        let stop = std::sync::Arc::clone(&guard.stop);
        let thread = std::thread::Builder::new()
            .name("envboard-activate".to_owned())
            .spawn(move || {
                loop {
                    if stop.load(std::sync::atomic::Ordering::Acquire) {
                        break;
                    }
                    // SAFETY: pipe 由这条线程独占持有。
                    let connected = unsafe { ConnectNamedPipe(pipe.raw(), null_mut()) };
                    if connected == 0 {
                        // SAFETY: 紧随 ConnectNamedPipe 失败之后取错误码。
                        let err = unsafe { GetLastError() };
                        // client 抢在 ConnectNamedPipe 之前连上 ⇒ 视为已连接。
                        if err != ERROR_PIPE_CONNECTED {
                            log::warn!("单实例：等待线程退出（ConnectNamedPipe error {err}）");
                            break;
                        }
                    }
                    // 被 `Guard::drop` 的自连接叫醒的：不服务，直接收摊。
                    if stop.load(std::sync::atomic::Ordering::Acquire) {
                        // SAFETY: 已连接，断开以还给管道对象一个干净状态。
                        unsafe { DisconnectNamedPipe(pipe.raw()) };
                        break;
                    }

                    let accepted = serve_once(pipe.raw());

                    // SAFETY: pipe 已连接；断开以接受下一个 client。
                    unsafe { DisconnectNamedPipe(pipe.raw()) };

                    if accepted {
                        crate::single_instance::bump_pending();
                        log::info!("单实例：收到激活请求，交给界面");
                    }
                }
                // SAFETY: 句柄所有权已归这条线程，退出时由它关闭 —— 管道对象
                // 随之消失，下一个进程才能把自己当主实例。
                unsafe { CloseHandle(pipe.raw()) };
            });
        match thread {
            Ok(join) => {
                if let Ok(mut slot) = guard.waiter.lock() {
                    *slot = Some(join);
                }
            }
            Err(error) => log::warn!("单实例：等待线程起不来（{error}）"),
        }
    }

    /// 服务一个 client：读请求 → 校验调用方 → 回执。`true` = 接受。
    fn serve_once(pipe: HANDLE) -> bool {
        let mut request = [0u8; 3];
        if !read_exact_with_timeout(pipe, &mut request, SERVER_REQUEST_TIMEOUT) {
            log::warn!("单实例：client 没在预算内把请求写完，断开");
            return false;
        }
        if request[0] != crate::single_instance::MAGIC
            || request[1] < crate::single_instance::PROTO_VERSION
        {
            log::warn!(
                "单实例：不是认识的请求（magic={} version={}），断开",
                request[0],
                request[1]
            );
            return false;
        }

        let accepted = verify_client(pipe);
        let resp = if accepted {
            crate::single_instance::RESP_ACCEPT
        } else {
            crate::single_instance::RESP_REJECT
        };
        let mut written = 0u32;
        // SAFETY: pipe 已连接，单字节缓冲区在本帧内有效。
        let wrote = unsafe { WriteFile(pipe, &resp, 1, &mut written, null_mut()) };
        if wrote == 0 || written != 1 {
            log::warn!("单实例：回执写失败");
            return accepted;
        }

        // 等 client 回一个 ACK 再断开。`PeekNamedPipe` 在服务端只看「进来的
        // 字节」，看不出「发出去的回执有没有被读走」；client 读到回执才回 ACK，
        // 所以拿到 ACK 就等于拿到了「回执已送达」的凭据 —— 少了这一步，
        // `DisconnectNamedPipe` 会把还没读走的回执直接丢掉（实测：client 收到的
        // 是 `ERROR_PIPE_NOT_CONNECTED`）。
        let mut ack = [0u8; 1];
        if !read_exact_with_timeout(pipe, &mut ack, SERVER_DRAIN_TIMEOUT) {
            log::warn!("单实例：没等到 client 的 ACK，照常断开");
        }
        accepted
    }

    /// 校验调用方：取 client 进程 exe 路径，与本程序 canonicalize 后比对。
    /// 任一步失败都拒绝（fail-closed）。
    fn verify_client(pipe: HANDLE) -> bool {
        let mut pid = 0u32;
        // SAFETY: pipe 已连接，client 正在等回执，进程仍活着。
        if unsafe { GetNamedPipeClientProcessId(pipe, &mut pid) } == 0 {
            return false;
        }
        let (Some(own), Some(caller)) = (own_exe_path(), process_image_path(pid)) else {
            return false;
        };
        let same = crate::single_instance::same_executable(&own, &caller);
        if !same {
            log::warn!(
                "单实例：拒绝激活——调用方 {} 不是本程序 {}",
                caller.display(),
                own.display()
            );
        }
        same
    }

    fn own_exe_path() -> Option<PathBuf> {
        std::env::current_exe()
            .ok()
            .and_then(|p| std::fs::canonicalize(p).ok())
    }

    fn process_image_path(pid: u32) -> Option<PathBuf> {
        // SAFETY: OpenProcess 拿查询句柄，QueryFullProcessImageNameW 读路径，
        // 句柄用 CloseHandle 关闭；缓冲区在本函数内有效。
        unsafe {
            let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if proc.is_null() {
                return None;
            }
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let ok =
                QueryFullProcessImageNameW(proc, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
            CloseHandle(proc);
            if ok == 0 {
                return None;
            }
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            std::fs::canonicalize(path).ok()
        }
    }

    /// 非阻塞读：按 `PEEK_INTERVAL` 轮询，到点返回 false。
    fn read_exact_with_timeout(pipe: HANDLE, out: &mut [u8], budget: Duration) -> bool {
        let deadline = Instant::now() + budget;
        let mut filled = 0usize;
        while filled < out.len() {
            match peek(pipe) {
                Some(available) if available > 0 => {
                    let want = out.len() - filled;
                    let mut read = 0u32;
                    // SAFETY: pipe 已连接，out 剩余区间有效。
                    let got = unsafe {
                        ReadFile(
                            pipe,
                            out.as_mut_ptr().add(filled),
                            want as u32,
                            &mut read,
                            null_mut(),
                        )
                    };
                    if got == 0 || read == 0 {
                        return false;
                    }
                    filled += read as usize;
                }
                Some(_) => {}
                None => return false,
            }
            if filled == out.len() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(PEEK_INTERVAL);
        }
        true
    }

    /// 管道里有多少字节可读；`None` = 读失败（管道断了）。
    fn peek(pipe: HANDLE) -> Option<u32> {
        let mut available = 0u32;
        // SAFETY: pipe 已连接；只读计数器，不需要数据缓冲区。
        let ok =
            unsafe { PeekNamedPipe(pipe, null_mut(), 0, null_mut(), &mut available, null_mut()) };
        (ok != 0).then_some(available)
    }

    /// 建主实例管道：`FIRST_PIPE_INSTANCE` 防多主，owner-only DACL 挡跨用户
    /// （调用方校验在等待线程里按 exe 路径做，这里是纵深防御）。
    ///
    /// # Safety
    /// `wide` 必须是以 NUL 结尾的宽字符串，且在本调用期间有效。
    unsafe fn create_owner_pipe(wide: &[u16]) -> HANDLE {
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        let mut sa = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null_mut(),
            bInheritHandle: 0,
        };
        let sddl = to_wide("D:P(A;;GA;;;OW)");
        // SAFETY: 见函数级 Safety；SDDL 与出参都有效，描述符稍后 LocalFree。
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                null_mut(),
            )
        } != 0
        {
            sa.lpSecurityDescriptor = sd;
        }
        // SAFETY: 见函数级 Safety；SA 指针要么为空要么指向本帧内的结构。
        let pipe = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                64,
                64,
                0,
                if sa.lpSecurityDescriptor.is_null() {
                    null()
                } else {
                    &sa
                },
            )
        };
        if !sd.is_null() {
            // SAFETY: sd 由上面的 Convert 分配，所有权归本函数。
            unsafe { LocalFree(sd) };
        }
        pipe
    }

    /// 「叫不到前台」那句话：被拒（调用方不是本程序）与无应答（主实例被挂起）
    /// 共用。写清「已在运行」与下一步动作，别让人以为是程序没起来。
    pub fn activation_failed_message(why: &str) -> String {
        format!(
            "envboard 已在运行，但这次没能把它的窗口叫到前台。\n\n\
             {why}\n\n\
             请从任务栏或托盘打开已有窗口；若它已经不正常，先结束它再启动。"
        )
    }

    /// 锁被**别的可执行文件**持有时的提示（管道拦不到这种情况：desktop 用的是
    /// 另一条管道名）。迁移期这条比裸 `conflict` 有用得多。
    pub fn lock_held_message(detail: &str) -> String {
        format!(
            "envboard 的 state-dir 已被另一个进程占用，本进程退出。\n\n\
             detail: {detail}\n\n\
             最常见的原因是旧版 envboard（Tauri 版）还在跑。请先关闭它再启动本版。"
        )
    }

    /// 给用户一个看得见的结论。本程序是 `windows_subsystem = "windows"`，
    /// 没有控制台，日志也只在 `RUST_LOG` 下去文件/调试器里看得到。
    ///
    /// `NG_NO_PROMPT=1`（仅 harness 构建）跳过弹窗，让自动化能直接读退出码 ——
    /// 与 `NG_*` 同一族纪律：发布物里这个变量无效。
    pub fn prompt(message: &str) {
        log::warn!("{message}");
        #[cfg(feature = "harness")]
        if crate::harness_flag("NG_NO_PROMPT") {
            return;
        }
        let text = to_wide(message);
        let caption = to_wide(crate::brand::NAME);
        // SAFETY: 两个宽字符串都在本调用期间有效；返回值不需要处理。
        unsafe {
            MessageBoxW(
                null_mut(),
                text.as_ptr(),
                caption.as_ptr(),
                MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
            );
        }
    }

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::{Path, PathBuf};

    /// 非 Windows：没有命名管道，整条 N 线退化为「锁被占就报错退出」。
    pub struct Guard;

    pub enum Preflight {
        Owner(Guard),
        Activated,
        Rejected(String),
        NoResponse(String),
        Unavailable { reason: String },
    }

    #[derive(Debug)]
    pub enum Activated {
        Absent,
        Accepted,
        Rejected(String),
        NoResponse(String),
    }

    pub fn state_dir() -> Option<PathBuf> {
        None
    }

    pub fn preflight() -> Preflight {
        Preflight::Unavailable {
            reason: "非 Windows 平台没有命名管道".to_owned(),
        }
    }

    pub fn spawn_waiter(_guard: &Guard) {}

    pub fn activation_failed_message(why: &str) -> String {
        format!("envboard 已在运行，但这次没能把它的窗口叫到前台：{why}")
    }

    pub fn lock_held_message(detail: &str) -> String {
        format!("envboard 的 state-dir 已被另一个进程占用：{detail}")
    }

    pub fn prompt(message: &str) {
        log::warn!("{message}");
    }
}

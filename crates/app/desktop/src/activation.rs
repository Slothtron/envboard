//! 二次启动激活（ADR-012）+ 调用方校验（SOP §6）：同 state-dir 的第二个进程
//! 不抢锁、不写状态，只把已有实例拉回前台然后退出 0。
//!
//! 现状是「关窗常驻」引入的死胡同：用户双击 exe，撞 `InstanceLock` 弹
//! 「another envboard process holds …」再退出，应用明明在跑却打不开窗口。
//!
//! 机制（Windows 命名管道，作用域 `Local\` = 当前登录会话）：
//! - 名字 = `\\.\pipe\envboard-activate-<FNV1a(normalize(state_dir))>`，按 state-dir
//!   派生（两个便携副本各带各的 state-dir，互不误伤；形态隔离自动继承）；
//! - 第一个实例在 **Tauri builder 之前**以 `FILE_FLAG_FIRST_PIPE_INSTANCE` +
//!   owner-only DACL 创建管道（句柄随进程消失，不留死管道），setup 后起等待线程；
//! - 第二个实例在 clap 解析之后、Tauri 初始化之前连管道握手：被接受 ⇒ 退出 0；
//!   被拒绝 ⇒ 退出非 0；无管道 ⇒ 正常启动。
//!
//! **为什么是管道而不是内核事件**：事件不携带调用方身份，同会话任意进程都能
//! `SetEvent` 触发开窗（SOP §6 反模式）。管道可在服务端用
//! `GetNamedPipeClientProcessId` + `QueryFullProcessImageNameW` 取调用方 exe 路径，
//! 与本程序比对，**只有同一可执行文件才开窗**；owner-only DACL 再挡跨用户。
//! 握手是双向的：client 写 magic 后等响应（保证服务端取 PID 时 client 仍连着），
//! 服务端写响应后等 client 关闭再 `DisconnectNamedPipe`（避免响应被丢弃）。
//!
//! 非 Windows 上整个模块退化为 `Unavailable`（无托盘/激活，关窗即退出）。

#[cfg(windows)]
pub use imp::{Preflight, preflight, spawn_waiter};

#[cfg(not(windows))]
pub use stub::{Preflight, preflight, spawn_waiter};

#[cfg(windows)]
mod imp {
    use std::mem::size_of;
    use std::path::{Path, PathBuf};
    use std::ptr::{null, null_mut};
    use std::sync::atomic::Ordering;

    use envboard_host::HostOptions;
    use tauri::{AppHandle, Manager};
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
        PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };

    /// client → server 的激活请求字节；server → client 的应答。
    const MAGIC: u8 = 0x4f;
    const RESP_ACCEPT: u8 = 1;
    const RESP_REJECT: u8 = 0;
    /// owner 断开后重连的间隙里 client 会撞 PIPE_BUSY，有限重试。
    const CONNECT_ATTEMPTS: usize = 5;

    /// 第一个实例的管道句柄（必须活着，否则管道对象消失）。
    pub struct ActivationHandle(HANDLE);

    // SAFETY: 管道句柄可在任意线程使用；ConnectNamedPipe 是线程无关的。
    unsafe impl Send for ActivationHandle {}

    impl ActivationHandle {
        /// 经方法调用让等待线程闭包捕获**整个句柄**（Send），而不是把裸
        /// `*mut c_void` 字段捕获进线程（Rust 2021 按字段捕获，那会不 Send）。
        fn raw(&self) -> HANDLE {
            self.0
        }
    }

    pub enum Preflight {
        /// 已有实例在跑且接受了激活：调用方应立即退出 0。
        Activated,
        /// 本进程是第一个实例：持有管道句柄，setup 后起等待线程。
        Owner(ActivationHandle),
        /// 有实例在跑，但调用方不是本可执行文件：拒绝激活，调用方退出非 0。
        Rejected,
        /// 机制不可用（解析不出 state-dir / 创建管道失败）：照常启动。
        Unavailable,
    }

    pub fn preflight(options: &HostOptions) -> Preflight {
        let Ok(state_dir) = options.resolve_state_dir() else {
            return Preflight::Unavailable;
        };
        let name = pipe_name(&state_dir);
        let wide = to_wide(&name);

        for _ in 0..CONNECT_ATTEMPTS {
            // SAFETY: wide 在本函数内有效；句柄由我们自己关闭。
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
                let result = client_handshake(client);
                // SAFETY: client 是本函数刚打开的句柄。
                unsafe { CloseHandle(client) };
                return result;
            }
            // SAFETY: 紧随 CreateFileW 失败之后取错误码。
            let err = unsafe { GetLastError() };
            if err == ERROR_FILE_NOT_FOUND {
                // 还没有 owner：尝试自己成为 owner。
                let owner = create_owner_pipe(&wide);
                if owner != INVALID_HANDLE_VALUE {
                    return Preflight::Owner(ActivationHandle(owner));
                }
                // SAFETY: 紧随 CreateNamedPipeW 失败之后取错误码。
                let err = unsafe { GetLastError() };
                if err == ERROR_ACCESS_DENIED {
                    // 竞态：另一个实例刚好先创建了管道 → 回到 client 重试。
                    continue;
                }
                return Preflight::Unavailable;
            }
            if err == ERROR_PIPE_BUSY {
                // owner 在 Disconnect→Connect 间隙：等一个实例空出来再试。
                // SAFETY: wide 有效；超时只影响等待时长。
                unsafe { WaitNamedPipeW(wide.as_ptr(), 1000) };
                continue;
            }
            return Preflight::Unavailable;
        }
        // 一直 PIPE_BUSY：管道存在即说明有实例在跑，按已激活处理。
        Preflight::Activated
    }

    /// client 握手：写 magic → 读应答。服务端取 PID 期间本端保持连接。
    fn client_handshake(pipe: HANDLE) -> Preflight {
        // SAFETY: pipe 是已连接的客户端句柄；缓冲区在本函数内有效。
        unsafe {
            let mut written = 0u32;
            if WriteFile(pipe, &MAGIC, 1, &mut written, null_mut()) == 0 {
                return Preflight::Unavailable;
            }
            let mut resp = 0u8;
            let mut read = 0u32;
            if ReadFile(pipe, &mut resp, 1, &mut read, null_mut()) == 0 || read != 1 {
                return Preflight::Unavailable;
            }
            if resp == RESP_ACCEPT {
                Preflight::Activated
            } else {
                Preflight::Rejected
            }
        }
    }

    /// 创建 owner 管道：`FILE_FLAG_FIRST_PIPE_INSTANCE` 防多 owner，owner-only
    /// DACL 挡跨用户（纵深防御；真正的调用方校验在 waiter 里按 exe 路径做）。
    fn create_owner_pipe(wide: &[u16]) -> HANDLE {
        // SAFETY: SDDL 宽字符串与 SECURITY_ATTRIBUTES 在本函数内有效；
        // 分配出的安全描述符用 LocalFree 释放。
        unsafe {
            let sddl = to_wide("D:P(A;;GA;;;OW)");
            let mut sd: PSECURITY_DESCRIPTOR = null_mut();
            let mut sa = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: null_mut(),
                bInheritHandle: 0,
            };
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                null_mut(),
            ) != 0
            {
                sa.lpSecurityDescriptor = sd;
            }
            let pipe = CreateNamedPipeW(
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
            );
            if !sd.is_null() {
                LocalFree(sd);
            }
            pipe
        }
    }

    /// 起等待线程：每次有 client 连上就校验调用方，通过才回前台，直到进程退出。
    ///
    /// 就绪门（基建 P0）：setup 没完成时窗口/核心还没就位，直接开窗会落空 ——
    /// 只记 `pending_activation`，由 setup 收尾统一开。存完再查一次 ready，
    /// 堵住「store 恰好落在 setup 的 swap 之后」的缝（两边都开也由去抖吸收）。
    pub fn spawn_waiter(app: AppHandle, handle: ActivationHandle) {
        std::thread::spawn(move || {
            let pipe = handle.raw();
            loop {
                // SAFETY: pipe 由本线程独占持有，直到进程退出。
                let connected = unsafe { ConnectNamedPipe(pipe, null_mut()) };
                if connected == 0 {
                    // SAFETY: 紧随 ConnectNamedPipe 失败之后取错误码。
                    let err = unsafe { GetLastError() };
                    // client 抢在 ConnectNamedPipe 之前连上 ⇒ 视为已连接。
                    if err != ERROR_PIPE_CONNECTED {
                        return;
                    }
                }

                // 读 magic（client 此刻保持连接，等待应答）。
                let mut magic = 0u8;
                let mut read = 0u32;
                // SAFETY: pipe 已连接；缓冲区在本函数内有效。
                let ok = unsafe { ReadFile(pipe, &mut magic, 1, &mut read, null_mut()) };
                if ok == 0 || read != 1 || magic != MAGIC {
                    // SAFETY: pipe 已连接，断开以接受下一个 client。
                    unsafe { DisconnectNamedPipe(pipe) };
                    continue;
                }

                let accepted = verify_client(pipe);

                // 应答 client（接受/拒绝），随后等 client 关闭再断开，
                // 确保 client 收到应答（DisconnectNamedPipe 会丢弃未读数据）。
                let resp = if accepted { RESP_ACCEPT } else { RESP_REJECT };
                let mut written = 0u32;
                // SAFETY: pipe 已连接；缓冲区在本函数内有效。
                unsafe {
                    let _ = WriteFile(pipe, &resp, 1, &mut written, null_mut());
                    let mut byte = 0u8;
                    let mut n = 0u32;
                    let _ = ReadFile(pipe, &mut byte, 1, &mut n, null_mut());
                    DisconnectNamedPipe(pipe);
                }

                if !accepted {
                    crate::diagnostics::note("activation rejected: caller is not this executable");
                    continue;
                }

                let Some(shell) = app.try_state::<crate::Shell>() else {
                    return;
                };
                if shell.is_exiting.load(Ordering::Relaxed) {
                    return;
                }
                if !shell.ready.load(Ordering::Acquire) {
                    shell.pending_activation.store(true, Ordering::Release);
                    crate::diagnostics::note("activation received before ready; pending");
                    if !shell.ready.load(Ordering::Acquire) {
                        continue;
                    }
                }
                let app_for_main = app.clone();
                let _ = app.run_on_main_thread(move || crate::tray::open_workbench(&app_for_main));
            }
        });
    }

    /// 校验调用方：取 client 进程 exe 路径，与本程序 canonicalize 后比对。
    /// 任一步失败都拒绝（fail-closed）。
    fn verify_client(pipe: HANDLE) -> bool {
        let mut pid = 0u32;
        // SAFETY: pipe 已连接，client 正在等待应答，进程仍活着。
        if unsafe { GetNamedPipeClientProcessId(pipe, &mut pid) } == 0 {
            return false;
        }
        let Some(own) = own_exe_path() else {
            return false;
        };
        let Some(caller) = process_image_path(pid) else {
            return false;
        };
        // Windows 路径大小写不敏感；canonicalize 后双方同为 \\?\ 扩展形式。
        own.to_string_lossy().to_lowercase() == caller.to_string_lossy().to_lowercase()
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

    fn pipe_name(state_dir: &Path) -> String {
        // 路径规则与组合根一致：大小写在 Windows 上不敏感，先折成小写；
        // 再哈希成十六进制，避免路径里的 `\` 被当成管道名分隔符。
        let normalized = state_dir.to_string_lossy().to_lowercase();
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in normalized.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        format!("\\\\.\\pipe\\envboard-activate-{hash:016x}")
    }

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(not(windows))]
mod stub {
    use envboard_host::HostOptions;
    use tauri::AppHandle;

    pub struct ActivationHandle;

    pub enum Preflight {
        Activated,
        Owner(ActivationHandle),
        Rejected,
        Unavailable,
    }

    pub fn preflight(_options: &HostOptions) -> Preflight {
        Preflight::Unavailable
    }

    pub fn spawn_waiter(_app: AppHandle, _handle: ActivationHandle) {}
}

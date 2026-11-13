//! 二次启动激活（ADR-012）：同 state-dir 的第二个进程不抢锁、不写状态，
//! 只把已有实例拉回前台然后退出 0。
//!
//! 现状是「关窗常驻」引入的死胡同：用户双击 exe，撞 `InstanceLock` 弹
//! 「another envboard process holds …」再退出，应用明明在跑却打不开窗口。
//!
//! 机制（Windows 命名内核事件，作用域 `Local\` = 当前登录会话）：
//! - 名字 = `Local\envboard-activate-<FNV1a(normalize(state_dir))>`，按 state-dir
//!   派生子目录语义（两个便携副本各带各的 state-dir，互不误伤）；
//! - 第一个实例在 **Tauri builder 之前**创建 auto-reset 事件（句柄随进程消失，
//!   不会留死信号），setup 后起等待线程；被 signal ⇒ 主线程 `open_workbench`；
//! - 第二个实例在 clap 解析之后、Tauri 初始化之前 `OpenEventW`：命中 ⇒ `SetEvent`
//!   并退出 0；未命中 ⇒ 正常启动。
//!
//! 非 Windows 上整个模块退化为 `Unavailable`（无托盘/激活，关窗即退出）。

#[cfg(windows)]
pub use imp::{Preflight, preflight, spawn_waiter};

#[cfg(not(windows))]
pub use stub::{Preflight, preflight, spawn_waiter};

#[cfg(windows)]
mod imp {
    use std::path::Path;

    use envboard_host::HostOptions;
    use tauri::AppHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, HANDLE};
    use windows_sys::Win32::System::Threading::{
        CreateEventW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent, WaitForSingleObject,
    };

    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;

    /// 第一个实例的事件句柄（必须活着，否则事件对象消失）。
    pub struct ActivationHandle(HANDLE);

    // SAFETY: 内核事件句柄可在任意线程使用；WaitForSingleObject 是线程无关的。
    unsafe impl Send for ActivationHandle {}

    impl ActivationHandle {
        /// 阻塞等待一次 signal。方法调用让闭包捕获整个句柄（Send），
        /// 而不是把裸 `*mut c_void` 字段捕获进线程（那会不 Send）。
        fn wait(&self) -> u32 {
            // SAFETY: 句柄由本线程独占持有，直到进程退出。
            unsafe { WaitForSingleObject(self.0, INFINITE) }
        }
    }

    pub enum Preflight {
        /// 已有实例在跑：事件已 signal，调用方应立即退出 0。
        Activated,
        /// 本进程是第一个实例：持有事件句柄，setup 后起等待线程。
        Owner(ActivationHandle),
        /// 机制不可用（解析不出 state-dir / 创建事件失败）：照常启动。
        Unavailable,
    }

    pub fn preflight(options: &HostOptions) -> Preflight {
        let Ok(state_dir) = options.resolve_state_dir() else {
            return Preflight::Unavailable;
        };
        let name = event_name(&state_dir);
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();

        // SAFETY: name 的宽字符串在本函数内一直有效；句柄由我们自己关闭。
        unsafe {
            let existing = OpenEventW(EVENT_MODIFY_STATE, 0, wide.as_ptr());
            if !existing.is_null() {
                let _ = SetEvent(existing);
                let _ = CloseHandle(existing);
                return Preflight::Activated;
            }
            if GetLastError() != ERROR_FILE_NOT_FOUND {
                return Preflight::Unavailable;
            }
            let created = CreateEventW(std::ptr::null(), 0, 0, wide.as_ptr());
            if !created.is_null() {
                return Preflight::Owner(ActivationHandle(created));
            }
            // 竞态：另一个实例刚好先创建了事件 → 重试一次 OpenEvent。
            let raced = OpenEventW(EVENT_MODIFY_STATE, 0, wide.as_ptr());
            if !raced.is_null() {
                let _ = SetEvent(raced);
                let _ = CloseHandle(raced);
                return Preflight::Activated;
            }
            Preflight::Unavailable
        }
    }

    /// 起等待线程：每次被 signal 就回前台，直到进程退出。
    pub fn spawn_waiter(app: AppHandle, handle: ActivationHandle) {
        std::thread::spawn(move || loop {
            if handle.wait() != WAIT_OBJECT_0 {
                return;
            }
            let app_for_main = app.clone();
            let _ = app.run_on_main_thread(move || crate::tray::open_workbench(&app_for_main));
        });
    }

    fn event_name(state_dir: &Path) -> String {
        // 路径规则与组合根一致：大小写在 Windows 上不敏感，先折成小写；
        // 再哈希成十六进制，避免路径里的 `\` 被当成内核对象命名空间分隔符。
        let normalized = state_dir.to_string_lossy().to_lowercase();
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in normalized.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        format!("Local\\envboard-activate-{hash:016x}")
    }

    use windows_sys::Win32::Foundation::GetLastError;
}

#[cfg(not(windows))]
mod stub {
    use envboard_host::HostOptions;
    use tauri::AppHandle;

    pub struct ActivationHandle;

    pub enum Preflight {
        Activated,
        Owner(ActivationHandle),
        Unavailable,
    }

    pub fn preflight(_options: &HostOptions) -> Preflight {
        Preflight::Unavailable
    }

    pub fn spawn_waiter(_app: AppHandle, _handle: ActivationHandle) {}
}

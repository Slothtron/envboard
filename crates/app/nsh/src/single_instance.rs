//! N1：单实例 preflight（方案 §5.3）。同一 state-dir 只允许一个写者，第二个进程
//! 在 `boot` **之前**就给出结论 —— 而不是先撞 `InstanceLock`，把一句 `conflict`
//! 甩给一个还没有窗口的进程（用户看到的就是「双击没反应」）。
//!
//! 机制（借 desktop `activation.rs` 已验证的形状，只借形状不借代码）：按
//! `state_dir` 派生 `\\.\pipe\envboard-nsh-activate-<FNV1a>`，以
//! `FILE_FLAG_FIRST_PIPE_INSTANCE` + owner-only DACL 建同名管道：
//!
//! - 建成 = 我是主实例 → 句柄挂到进程结束（句柄随进程消失，不留死管道）；
//! - `ERROR_ACCESS_DENIED` = 同 state-dir 已有实例 → 提示后 `exit 2`；
//! - 其它错误 / 解析不出 state-dir = 机制不可用 → 照常启动，交给 `InstanceLock`。
//!
//! 前缀故意带 `nsh`：迁移期 desktop 与 NSH 可能同机共存，desktop 用
//! `envboard-activate-`，两个实例互不冒充。
//!
//! **N1 不做握手**（不连管道、不校验调用方、不拉前台）—— 那是 N2 的事。这里只把
//! 「谁是主」定下来；`FIRST_PIPE_INSTANCE` 而不是「先 boot 再看锁」，避开了
//! 「主实例正在 boot、管道还没建好」的窗口期。

use std::path::Path;

pub use imp::{Guard, Preflight, lock_held_message, occupied_message, preflight, prompt};

/// Windows 上的真实管道走一遍（`preflight_at`），其它平台没有这条路径。
#[cfg(windows)]
pub use imp::preflight_at;

/// 管道名前缀。与 desktop 的 `envboard-activate-` 刻意不同名。
const PIPE_PREFIX: &str = "envboard-nsh-activate-";

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
        assert!(name.starts_with(r"\\.\pipe\envboard-nsh-activate-"), "{name}");
        // 十六位小写十六进制，且不含路径分隔符（`\` 会破坏管道名的层级）。
        let suffix = name.trim_start_matches(r"\\.\pipe\envboard-nsh-activate-");
        assert_eq!(suffix.len(), 16, "{name}");
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn occupied_message_names_the_state_dir() {
        let message = super::occupied_message(Path::new(r"C:\Users\x\envboard-dev"));
        assert!(message.contains(r"C:\Users\x\envboard-dev"), "{message}");
        assert!(message.contains("已在运行"), "{message}");
    }

    /// 真实管道走一遍：同一个 state-dir 的第二次 claim 必须是 `Occupied`，
    /// 异 dir 互不影响，主实例句柄一关管道随即消失（不留死管道）。
    #[cfg(windows)]
    #[test]
    fn second_claim_is_occupied_and_a_dropped_pipe_is_free_again() {
        use super::{Preflight, preflight_at};

        let stamp = std::process::id();
        let dir = std::env::temp_dir().join(format!("eb-n1-pipe-{stamp}-a"));
        let other_dir = std::env::temp_dir().join(format!("eb-n1-pipe-{stamp}-b"));

        let first = preflight_at(&dir);
        assert!(
            matches!(first, Preflight::Owner(_)),
            "首次 claim 应当是主实例"
        );

        let second = preflight_at(&dir);
        assert!(
            matches!(second, Preflight::Occupied { .. }),
            "同 dir 的第二次 claim 必须被拦成 Occupied"
        );

        let elsewhere = preflight_at(&other_dir);
        assert!(
            matches!(elsewhere, Preflight::Owner(_)),
            "不同 state-dir 必须能各自主"
        );

        // 主实例退出 = 句柄关闭 = 管道对象消失：同一 dir 立刻能重新当主。
        drop(first);
        let again = preflight_at(&dir);
        assert!(
            matches!(again, Preflight::Owner(_)),
            "句柄关闭后不应留下死管道"
        );
        drop(again);
        drop(second);
        drop(elsewhere);
    }
}

#[cfg(windows)]
mod imp {
    use std::mem::size_of;
    use std::path::{Path, PathBuf};
    use std::ptr::{null, null_mut};

    use envboard_host::HostOptions;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, ERROR_PIPE_BUSY, GetLastError, HANDLE,
        INVALID_HANDLE_VALUE, LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
    use windows_sys::Win32::System::Pipes::{
        CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MessageBoxW,
    };

    /// 主实例的管道句柄。**必须活到进程结束**：句柄一关，管道对象就消失，
    /// 下一个进程会把自己当成主实例。`Drop` 只负责让「提前退出」不留尸。
    pub struct Guard(HANDLE);

    impl Drop for Guard {
        fn drop(&mut self) {
            // SAFETY: 句柄由本结构独占，且只在 Drop 里关闭一次。
            unsafe { CloseHandle(self.0) };
        }
    }

    impl Guard {
        /// 给测试与 N2 的等待线程用：把整个句柄（而不是裸指针）交出去。
        #[allow(dead_code)]
        pub(crate) fn raw(&self) -> HANDLE {
            self.0
        }
    }

    pub enum Preflight {
        /// 本进程是主实例：句柄要一直活到 `run_app` 结束。
        Owner(Guard),
        /// 同 state-dir 已有实例（或它的管道不可用）：给提示、退出 2。
        Occupied { pipe: String, state_dir: PathBuf },
        /// 机制不可用（解析不出 state-dir、建管道报别的错、非 Windows）：
        /// 照常启动，互斥仍由 `InstanceLock` 兜底。
        Unavailable { reason: String },
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

    /// 给定 state-dir 走一次 preflight。拆出来是为了让单测能用临时目录，
    /// 不必抢真实 state-dir 的管道（同进程两次调用即覆盖「已有实例」分支）。
    pub fn preflight_at(dir: &Path) -> Preflight {
        let pipe = crate::single_instance::pipe_name(dir);
        let wide = to_wide(&pipe);

        // SAFETY: wide 在本函数内有效；失败时句柄无效，成功时交给 Guard 持有。
        let handle = unsafe { create_owner_pipe(&wide) };
        if handle != INVALID_HANDLE_VALUE {
            return Preflight::Owner(Guard(handle));
        }
        // SAFETY: 紧随 CreateNamedPipeW 失败之后取错误码。
        let err = unsafe { GetLastError() };
        // 两种都算「已被占」：跨进程撞名字是 `ERROR_ACCESS_DENIED`
        // （`FIRST_PIPE_INSTANCE` 的语义），同进程再建一次是 `ERROR_PIPE_BUSY`
        // （`nMaxInstances = 1` 已用满，实测 error 231）。名字由 state-dir 派生、
        // 前缀独有，出现在这条名字上的管道只可能是 envboard 自己。
        if err == ERROR_ACCESS_DENIED || err == ERROR_PIPE_BUSY {
            // 同名管道的第一个实例已被占：同 state-dir 有进程在跑。
            Preflight::Occupied {
                pipe,
                state_dir: dir.to_path_buf(),
            }
        } else {
            Preflight::Unavailable {
                reason: format!("CreateNamedPipeW 失败（error {err}）"),
            }
        }
    }

    /// 建主实例管道：`FIRST_PIPE_INSTANCE` 防多主，owner-only DACL 挡跨用户
    /// （真正的调用方校验在 N2 的等待线程里按 exe 路径做，这里是纵深防御）。
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

    /// 「已有实例」那句话。N1 阶段还没有唤起机制，所以要让用户知道
    /// 该去哪里找窗口，以及迁移期最容易踩的混跑情形。
    pub fn occupied_message(state_dir: &Path) -> String {
        format!(
            "envboard 已在运行（state-dir: {}）。\n\n\
             请从已有窗口或托盘继续使用，不要再启动第二个。\n\
             若刚才关的是旧版 envboard（Tauri 版），请先确认它已退出——\
             两版共用同一份 state-dir，第二个写者会把账本写乱。",
            state_dir.display()
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
        Occupied { pipe: String, state_dir: PathBuf },
        Unavailable { reason: String },
    }

    pub fn state_dir() -> Option<PathBuf> {
        None
    }

    pub fn preflight() -> Preflight {
        Preflight::Unavailable {
            reason: "非 Windows 平台没有命名管道".to_owned(),
        }
    }

    pub fn occupied_message(state_dir: &Path) -> String {
        format!(
            "envboard 已在运行（state-dir: {}）。",
            state_dir.display()
        )
    }

    pub fn lock_held_message(detail: &str) -> String {
        format!("envboard 的 state-dir 已被另一个进程占用：{detail}")
    }

    pub fn prompt(message: &str) {
        log::warn!("{message}");
    }
}

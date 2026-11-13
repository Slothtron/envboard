//! 提权卸载（ADR-010）：只用于「证书也装在**本机** ROOT」时的一次性提权。
//!
//! **安装不在这里** —— 安装走系统证书向导（`ops::open_cert_install_wizard`），
//! 装到当前用户还是计算机由用户在向导里选，本进程从不请求管理员。
//!
//! 两半：
//! * 父进程：`run_elevated_uninstall` 用 `ShellExecuteW("runas")` 拉起同一二进制，
//!   参数是私有 helper 形态（`--machine-trust uninstall <pem>`）；UAC 由系统弹。
//! * 子进程（提权后）：`try_run_helper` 在 `Cli::parse()` **之前**拦截，读 PEM → DER →
//!   按字节完全相等从本机 ROOT 卸载，然后退出；不初始化 Tauri/WebView，也不拿
//!   单实例锁——它和主界面不是同一个「工作台实例」。
//!
//! FFI 照 `envboard_host::trust_store` 的 crypt32 先例手写（`#[link]` + `extern "system"`），
//! 不为此引 windows-sys 直接依赖。

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::path::Path;

    use envboard_engine::{Error, ErrorCode};

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteW(
            hwnd: *mut c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show: i32,
        ) -> isize;
    }

    /// `SW_HIDE`；提权子进程没有可见窗口可显示。
    const SW_HIDE: i32 = 0;
    /// ShellExecuteW 返回 ≤ 32 即失败（含 `ERROR_CANCELLED = 1223`）。
    const SHELL_SUCCESS_FLOOR: isize = 32;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 拉起提权助手卸载本机 ROOT 里的这张证书。UAC 被取消时回 store_failure 信封。
    pub fn run_elevated_uninstall(pem: &Path) -> Result<(), Error> {
        let exe = std::env::current_exe().map_err(|error| {
            Error::new(
                ErrorCode::InternalError,
                format!("cannot resolve the current exe: {error}"),
            )
        })?;
        let exe = exe.to_string_lossy().into_owned();
        // 路径一律带引号：状态目录可能含空格。
        let params = format!("--machine-trust uninstall \"{}\"", pem.display());
        let operation = wide("runas");
        let file = wide(&exe);
        let parameters = wide(&params);
        // SAFETY: 只传本函数持有的只读宽字符串；返回值只用于判成败。
        let rc = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                operation.as_ptr(),
                file.as_ptr(),
                parameters.as_ptr(),
                std::ptr::null(),
                SW_HIDE,
            )
        };
        if rc <= SHELL_SUCCESS_FLOOR {
            return Err(Error::new(
                ErrorCode::StoreFailure,
                format!(
                    "cannot elevate the helper (ShellExecuteW returned {rc}); the machine ROOT was not touched — UAC may have been cancelled"
                ),
            ));
        }
        Ok(())
    }

    /// helper 形态：`--machine-trust uninstall <pem>`。命中返回退出码，否则 `None`。
    pub fn try_run_helper(
        mut args: impl Iterator<Item = std::ffi::OsString>,
    ) -> Option<std::process::ExitCode> {
        let _exe = args.next();
        let rest: Vec<std::ffi::OsString> = args.collect();
        if rest.first().and_then(|arg| arg.to_str()) != Some("--machine-trust") {
            return None;
        }
        let Some(mode) = rest.get(1).and_then(|arg| arg.to_str()) else {
            eprintln!("envboard: --machine-trust needs a mode (uninstall)");
            return Some(std::process::ExitCode::from(2));
        };
        if mode != "uninstall" {
            eprintln!("envboard: unknown --machine-trust mode {mode:?} (only uninstall)");
            return Some(std::process::ExitCode::from(2));
        }
        let Some(pem) = rest.get(2).map(Path::new) else {
            eprintln!("envboard: --machine-trust uninstall needs a certificate PEM path");
            return Some(std::process::ExitCode::from(2));
        };
        Some(run_machine_uninstall(pem))
    }

    fn run_machine_uninstall(pem: &Path) -> std::process::ExitCode {
        let pem_bytes = match std::fs::read(pem) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("envboard: cannot read {}: {error}", pem.display());
                return std::process::ExitCode::from(1);
            }
        };
        let der = match rustls_pemfile::certs(&mut pem_bytes.as_slice()).next() {
            Some(Ok(cert)) => cert.as_ref().to_vec(),
            Some(Err(error)) => {
                eprintln!("envboard: cannot parse the certificate PEM: {error}");
                return std::process::ExitCode::from(1);
            }
            None => {
                eprintln!("envboard: no certificate in {}", pem.display());
                return std::process::ExitCode::from(1);
            }
        };
        match envboard_host::trust_store::uninstall_machine(&der) {
            Ok(removed) => {
                eprintln!("envboard: machine ROOT uninstall finished (removed={removed})");
                std::process::ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!(
                    "envboard: cannot uninstall from the machine ROOT: {}",
                    error.message
                );
                std::process::ExitCode::from(1)
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::Path;

    pub fn run_elevated_uninstall(_pem: &Path) -> Result<(), envboard_engine::Error> {
        Err(envboard_engine::Error::new(
            envboard_engine::ErrorCode::StoreFailure,
            "machine ROOT uninstall is implemented on Windows only",
        ))
    }

    pub fn try_run_helper(
        _args: impl Iterator<Item = std::ffi::OsString>,
    ) -> Option<std::process::ExitCode> {
        None
    }
}

pub use imp::{run_elevated_uninstall, try_run_helper};
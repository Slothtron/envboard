//! 系统信任库：这张 CA 到底装没装到「受信任的根证书颁发机构」里，以及
//! **用户主动发起**的安装/卸载（ADR-007 反转了早期「只探测不代装」的政策）。
//!
//! 边界：**写只碰当前用户 ROOT 存储**（免管理员提权）；本机存储始终只读探测。
//! 安装/卸载是用户在工作台里显式确认过的决定，不是后台静默行为；卸载可逆
//! （再装回来即可），安装也不从系统存储移除任何其它证书。
//!
//! 探测口径："当前用户 / 本机的 ROOT 存储里有没有与这张 CA **字节完全相同**的证书"。
//!
//! 为什么不是"按指纹查"：`CertFindCertificateInStore` 的哈希查找要 SHA-1，而本仓
//! 只带了 SHA-256；枚举全部证书再逐字节比对同样准确（ROOT 存储通常几十到几百张，
//! 一次全量比较是微秒级），且不需要为此引一个哈希实现。卸载定位复用同一条循环。
//!
//! 非 Windows 平台：探测一律 [`StorePresence::Unknown`]，写操作响亮报错 ——
//! 宁可说"不知道/不支持"，也不要拿猜出来的结果误导用户。

/// 一张证书在某个存储里的存在性。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorePresence {
    Installed,
    Missing,
    /// 探测不了（平台不支持、或系统 API 调用失败）。
    Unknown,
}

/// 两个存储的探测结果 + 一句人话说明（`Unknown` 时用来解释原因）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustProbe {
    /// 当前用户 \ 受信任的根证书颁发机构。
    pub current_user: StorePresence,
    /// 本机 \ 受信任的根证书颁发机构。
    pub machine: StorePresence,
    /// 探测失败/不支持时的原因；一切正常时为 `None`。
    pub detail: Option<String>,
}

impl TrustProbe {
    pub fn unknown(detail: impl Into<String>) -> Self {
        Self {
            current_user: StorePresence::Unknown,
            machine: StorePresence::Unknown,
            detail: Some(detail.into()),
        }
    }

    /// 任一存储装了就算子：客户端信任的是"这张证书在不在它的信任链里"。
    pub fn is_installed(&self) -> bool {
        matches!(
            (self.current_user, self.machine),
            (StorePresence::Installed, _) | (_, StorePresence::Installed)
        )
    }
}

/// 探测 `cert_der` 是否已导入系统根证书存储。
pub fn probe(cert_der: &[u8]) -> TrustProbe {
    imp::probe(cert_der)
}

/// 写操作的目标存储（ADR-010）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // 非 Windows 构建里两个变体都不被读取。
pub enum StoreScope {
    /// 当前用户 ROOT：免提权。安装入口已改为系统证书向导，写入函数只作宿主能力保留。
    CurrentUser,
    /// 本机 ROOT：写需要管理员，只允许提权助手调用。
    LocalMachine,
}

/// 把 `cert_der` 安装进**当前用户** ROOT 存储（幂等：已存在则替换为同一张）。
///
/// 安装入口是系统证书导入向导（ADR-010）；本函数保留为宿主能力，界面不再直接调用。
pub fn install(cert_der: &[u8]) -> Result<(), envboard_engine::Error> {
    imp::install(StoreScope::CurrentUser, cert_der)
}

/// 从**当前用户** ROOT 存储卸载与 `cert_der` 字节相同的证书。
///
/// 返回是否真的删了一张（没装 = `false`，幂等成功）。
pub fn uninstall(cert_der: &[u8]) -> Result<bool, envboard_engine::Error> {
    imp::uninstall(StoreScope::CurrentUser, cert_der)
}

/// 从**本机** ROOT 存储卸载与 `cert_der` 字节相同的证书（ADR-010：提权助手专用）。
///
/// 返回是否真的删了一张（没装 = `false`，幂等成功）。
pub fn uninstall_machine(cert_der: &[u8]) -> Result<bool, envboard_engine::Error> {
    imp::uninstall(StoreScope::LocalMachine, cert_der)
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    use super::{StorePresence, StoreScope, TrustProbe};

    // kernel-less 的 crypt32 声明：只用到六个符号，不为此引 windows-sys 直接依赖。
    #[link(name = "crypt32")]
    unsafe extern "system" {
        fn CertOpenStore(
            store_provider: *const i8,
            encoding: u32,
            crypt_prov: usize,
            flags: u32,
            para: *const u16,
        ) -> *mut c_void;
        fn CertEnumCertificatesInStore(
            store: *mut c_void,
            prev: *const CertContext,
        ) -> *const CertContext;
        fn CertFreeCertificateContext(context: *const CertContext) -> i32;
        fn CertCloseStore(store: *mut c_void, flags: u32) -> i32;
        fn CertAddEncodedCertificateToStore(
            store: *mut c_void,
            cert_encoding_type: u32,
            cert_encoded: *const u8,
            cert_encoded_len: u32,
            add_disposition: u32,
            cert_context: *mut *const CertContext,
        ) -> i32;
        fn CertDeleteCertificateFromStore(context: *const CertContext) -> i32;
    }

    /// Win32 的 `CERT_CONTEXT`（只取本模块需要的字段；字段顺序与对齐必须照抄）。
    #[repr(C)]
    struct CertContext {
        cert_encoding_type: u32,
        cert_encoded: *const u8,
        cert_encoded_len: u32,
        cert_info: *const c_void,
        cert_store: *mut c_void,
    }

    /// `CERT_STORE_PROV_SYSTEM_W`（win32 头里的 `(LPCSTR)10`）。
    const STORE_PROV_SYSTEM_W: *const i8 = 10 as *const i8;
    const STORE_CURRENT_USER: u32 = 0x0001_0000;
    const STORE_LOCAL_MACHINE: u32 = 0x0002_0000;
    /// `CERT_STORE_ADD_REPLACE_EXISTING`：同名/同证书已在则替换，安装幂等。
    const ADD_REPLACE_EXISTING: u32 = 3;
    /// `X509_ASN_ENCODING`：CertAddEncodedCertificateToStore 的编码类型**必须是 1**
    /// （传 0 = E_INVALIDARG，真机走查抓过：os error -2147024809）。
    const X509_ASN_ENCODING: u32 = 1;
    /// `CERT_STORE_READONLY_FLAG`：探测必须**只读**开存储。不带它时非管理员
    /// 以读写开本机存储必被拒（os error 5）——真机「探测不了」的真因（ADR-008）。
    const STORE_READONLY: u32 = 0x0000_8000;

    /// 写操作失败的统一信封：store_failure + 系统错误原文；os error 5 按存储分档提权指路。
    fn write_error(scope: StoreScope, action: &str, error: std::io::Error) -> envboard_engine::Error {
        let (label, field) = match scope {
            StoreScope::CurrentUser => ("current user", "system_trust_store.current_user"),
            StoreScope::LocalMachine => ("local machine", "system_trust_store.local_machine"),
        };
        let mut message = format!("cannot {action} the {label} ROOT store: {error}");
        if error.raw_os_error() == Some(5) {
            match scope {
                StoreScope::CurrentUser => message.push_str(
                    " (access denied; the current user store should not need elevation — check policy/ACL)",
                ),
                StoreScope::LocalMachine => message.push_str(
                    " (access denied; writing the machine store requires administrator rights — the elevated helper must run as admin)",
                ),
            }
        }
        envboard_engine::Error {
            code: envboard_engine::ErrorCode::StoreFailure,
            message,
            field: Some(field.to_string()),
        }
    }

    /// 打开目标 scope 的 ROOT 存储（读写）。失败回写操作信封。
    fn open_root(scope: StoreScope, action: &str) -> Result<*mut c_void, envboard_engine::Error> {
        let flags = match scope {
            StoreScope::CurrentUser => STORE_CURRENT_USER,
            StoreScope::LocalMachine => STORE_LOCAL_MACHINE,
        };
        let root = wide("ROOT");
        // SAFETY: 只传常量与自持有缓冲区；句柄由调用方负责 CertCloseStore。
        let store = unsafe { CertOpenStore(STORE_PROV_SYSTEM_W, 0, 0, flags, root.as_ptr()) };
        if store.is_null() {
            return Err(write_error(scope, action, std::io::Error::last_os_error()));
        }
        Ok(store)
    }

    pub fn install(scope: StoreScope, cert_der: &[u8]) -> Result<(), envboard_engine::Error> {
        let store = open_root(scope, "install into")?;
        // SAFETY: store 有效；der 由调用方持有且长度匹配；out context 传 null
        // （不需要拿回上下文，crypt32 自行管理）。
        let ok = unsafe {
            CertAddEncodedCertificateToStore(
                store,
                X509_ASN_ENCODING,
                cert_der.as_ptr(),
                cert_der.len() as u32,
                ADD_REPLACE_EXISTING,
                std::ptr::null_mut(),
            )
        };
        let error = std::io::Error::last_os_error();
        unsafe { CertCloseStore(store, 0) };
        if ok == 0 {
            return Err(write_error(scope, "install into", error));
        }
        Ok(())
    }

    /// 在指定 scope 的 ROOT 存储里按字节完全相等卸载（ADR-010：本机只经提权助手）。
    pub fn uninstall(scope: StoreScope, cert_der: &[u8]) -> Result<bool, envboard_engine::Error> {
        let store = open_root(scope, "uninstall from")?;
        let mut removed = false;
        let mut context: *const CertContext = std::ptr::null();
        loop {
            // SAFETY: store 有效；传回上一个 context 时 crypt32 释放它（枚举契约）。
            context = unsafe { CertEnumCertificatesInStore(store, context) };
            if context.is_null() {
                break;
            }
            // SAFETY: 非空 context 指向有效 CERT_CONTEXT（枚举契约）。
            let (bytes, len) = unsafe {
                (
                    (*context).cert_encoded,
                    (*context).cert_encoded_len as usize,
                )
            };
            if bytes.is_null() {
                continue;
            }
            // SAFETY: 证书编码由 crypt32 持有，长度为上面读到的字节数。
            let encoded = unsafe { std::slice::from_raw_parts(bytes, len) };
            if encoded == cert_der {
                // SAFETY: CertDeleteCertificateFromStore **消费**该 context
                // （成功或失败都释放），故这里不再 CertFreeCertificateContext。
                let ok = unsafe { CertDeleteCertificateFromStore(context) };
                let error = std::io::Error::last_os_error();
                unsafe { CertCloseStore(store, 0) };
                if ok == 0 {
                    return Err(write_error(scope, "uninstall from", error));
                }
                removed = true;
                break;
            }
        }
        if !removed {
            unsafe { CertCloseStore(store, 0) };
        }
        Ok(removed)
    }

    pub fn probe(cert_der: &[u8]) -> TrustProbe {
        let mut detail = None;
        let current_user = probe_store(cert_der, STORE_CURRENT_USER, &mut detail);
        let machine = probe_store(cert_der, STORE_LOCAL_MACHINE, &mut detail);
        TrustProbe {
            current_user,
            machine,
            detail,
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn probe_store(cert_der: &[u8], flags: u32, detail: &mut Option<String>) -> StorePresence {
        let root = wide("ROOT");
        // SAFETY: 只传常量与自持有缓冲区；返回的句柄在函数末尾关闭。
        let store =
            unsafe { CertOpenStore(STORE_PROV_SYSTEM_W, 0, 0, flags | STORE_READONLY, root.as_ptr()) };
        if store.is_null() {
            let code = std::io::Error::last_os_error();
            detail.get_or_insert_with(|| format!("cannot open the system ROOT store: {code}"));
            return StorePresence::Unknown;
        }

        let mut found = false;
        let mut context: *const CertContext = std::ptr::null();
        loop {
            // SAFETY: `store` 由 CertOpenStore 给出且未关闭；传回上一个 context
            // 时 crypt32 会释放它（枚举契约）。
            context = unsafe { CertEnumCertificatesInStore(store, context) };
            if context.is_null() {
                break;
            }
            // SAFETY: 非空 context 指向一个有效的 CERT_CONTEXT（枚举契约）。
            let (bytes, len) = unsafe {
                (
                    (*context).cert_encoded,
                    (*context).cert_encoded_len as usize,
                )
            };
            if !bytes.is_null() {
                // SAFETY: 证书编码由 crypt32 持有，长度为上面读到的字节数。
                let encoded = unsafe { std::slice::from_raw_parts(bytes, len) };
                if encoded == cert_der {
                    found = true;
                    break;
                }
            }
        }
        if !context.is_null() {
            // 提前 break 时最后一个 context 还没被枚举释放。
            unsafe { CertFreeCertificateContext(context) };
        }
        unsafe { CertCloseStore(store, 0) };

        if found {
            StorePresence::Installed
        } else {
            StorePresence::Missing
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{StoreScope, TrustProbe};

    pub fn probe(_cert_der: &[u8]) -> TrustProbe {
        TrustProbe::unknown("system trust store probing is implemented on Windows only")
    }

    pub fn install(_scope: StoreScope, _cert_der: &[u8]) -> Result<(), envboard_engine::Error> {
        Err(envboard_engine::Error::new(
            envboard_engine::ErrorCode::StoreFailure,
            "system trust store writes are implemented on Windows only",
        ))
    }

    pub fn uninstall(_scope: StoreScope, _cert_der: &[u8]) -> Result<bool, envboard_engine::Error> {
        Err(envboard_engine::Error::new(
            envboard_engine::ErrorCode::StoreFailure,
            "system trust store writes are implemented on Windows only",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::StorePresence;

    #[test]
    fn unknown_counts_as_not_installed_and_carries_a_reason() {
        let probe = super::TrustProbe::unknown("no platform support");
        assert!(!probe.is_installed());
        assert_eq!(probe.current_user, StorePresence::Unknown);
        assert!(probe.detail.is_some());
    }

    #[test]
    fn any_store_counts_as_installed() {
        let probe = super::TrustProbe {
            current_user: StorePresence::Missing,
            machine: StorePresence::Installed,
            detail: None,
        };
        assert!(probe.is_installed(), "本机装了就够客户端校验");
    }
}

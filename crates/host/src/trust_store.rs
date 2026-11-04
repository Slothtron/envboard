//! 系统信任库探测：这张 CA 到底装没装到「受信任的根证书颁发机构」里。
//!
//! **只读**。安装/卸载要写系统存储、还会弹一次安全警告，那是用户的决定；
//! 本模块只回答一个问题："现在这台机器上，当前用户 / 本机的 ROOT 存储里有没有
//! 与这张 CA **字节完全相同**的证书"。
//!
//! 为什么不是"按指纹查"：`CertFindCertificateInStore` 的哈希查找要 SHA-1，而本仓
//! 只带了 SHA-256；枚举全部证书再逐字节比对同样准确（ROOT 存储通常几十到几百张，
//! 一次全量比较是微秒级），且不需要为此引一个哈希实现。
//!
//! 非 Windows 平台目前一律返回 [`StorePresence::Unknown`]：宁可说"不知道"，也不要
//! 拿一个猜出来的"未安装"去误导用户。

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

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    use super::{StorePresence, TrustProbe};

    // kernel-less 的 crypt32 声明：只用到四个符号，不为此引 windows-sys 直接依赖。
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
        let store = unsafe { CertOpenStore(STORE_PROV_SYSTEM_W, 0, 0, flags, root.as_ptr()) };
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
    use super::TrustProbe;

    pub fn probe(_cert_der: &[u8]) -> TrustProbe {
        TrustProbe::unknown("system trust store probing is implemented on Windows only")
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

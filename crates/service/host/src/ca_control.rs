//! 共享 CA 的**可操作面**：看（摘要/PEM）、重新签发、按域名签发、系统信任库探测。
//!
//! 分工：
//!
//! * 材料的生成与解析在 `envboard_engine::ca`（路径规则也只有那一份）；
//! * 本模块只把它们接起来，并且**保证旋转对正在跑的实例立即生效**
//!   （`CaStore` 是引擎按连接读取的当前值，换入即生效，不需要重启环境）；
//! * 系统信任库在 [`crate::trust_store`]：探测只读；**安装走系统证书向导**
//!   （位置由用户选），**卸载优先当前用户直删、本机存储经提权助手**（ADR-010）。
//!
//! 界面形态通过 [`crate::Prepared::ca`] 拿到本模块；它**不含私钥**——CA 私钥只在
//! 引擎进程内存与状态目录的 `envboard-ca.pem` 里。

use std::path::PathBuf;
use std::sync::Arc;

use envboard_engine::ca::{CaStore, SharedCa, cert_path};
use envboard_engine::{CaOutcome, Error};

use crate::trust_store::{self, TrustProbe};

/// 按域名签发出来的一张叶子证书（证书 + 私钥，PEM）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeafPem {
    pub host: String,
    pub cert_pem: String,
    pub key_pem: String,
}

/// 共享 CA 的操作句柄。
#[derive(Clone)]
pub struct CaControl {
    store: CaStore,
    confdir: PathBuf,
}

impl CaControl {
    pub fn new(store: CaStore, confdir: PathBuf) -> Self {
        Self { store, confdir }
    }

    /// 当前 CA 的 SHA-256（公钥部分）指纹。
    pub fn fingerprint(&self) -> String {
        self.store.current().fingerprint().to_string()
    }

    /// 证书摘要（界面只读展示用）。结构异常时 `None`，由界面降级。
    pub fn info(&self) -> Option<serde_json::Value> {
        self.store
            .current()
            .info()
            .and_then(|info| serde_json::to_value(info).ok())
    }

    /// 证书 PEM 字节（只含证书，不含私钥）。
    pub fn pem(&self) -> Arc<Vec<u8>> {
        Arc::new(self.store.current().ca_cert_pem().into_bytes())
    }

    /// 证书 DER 字节（只含证书；ADR-010 安装向导导出 `.cer` 用）。
    pub fn cert_der(&self) -> Vec<u8> {
        self.store.current().ca_cert_der().to_vec()
    }

    /// 客户端要安装的那张证书的路径（文件名规则归引擎）。
    pub fn cert_path(&self) -> PathBuf {
        cert_path(&self.confdir)
    }

    /// **重新签发**：生成新 CA、覆盖状态目录里的文件、并让运行中的引擎立刻换用
    /// 新证书（下一次 CONNECT 生效）。
    ///
    /// 旧证书不会从系统信任库里移除 —— 那不是本工具该替用户做的决定。
    pub fn rotate(&self) -> Result<CaOutcome, Error> {
        let (ca, outcome) = SharedCa::rotate(&self.confdir)?;
        self.store.replace(ca);
        Ok(outcome)
    }

    /// 为指定域名签发一张叶子证书（证书 + 私钥 PEM）。不影响 MITM 的 SNI 缓存。
    pub fn issue_pem(&self, host: &str) -> Result<LeafPem, Error> {
        let ca = self.store.current();
        let (cert_pem, key_pem) = ca.issue_pem(host)?;
        Ok(LeafPem {
            host: host.trim().to_string(),
            cert_pem,
            key_pem,
        })
    }

    /// 这张 CA 是否已导入系统根证书存储（只读探测）。
    pub fn trust(&self) -> TrustProbe {
        trust_store::probe(self.store.current().ca_cert_der())
    }

    /// 把当前 CA 安装进**当前用户** ROOT 存储（ADR-007；安装入口已改为系统向导，
    /// 本方法保留为宿主能力，界面不再直接调用）。
    pub fn install_trust(&self) -> Result<(), Error> {
        trust_store::install(self.store.current().ca_cert_der())
    }

    /// 从**当前用户** ROOT 存储卸载当前 CA（字节比对定位）。
    /// 返回是否真删了一张；没装 = false（幂等成功）。
    pub fn uninstall_trust(&self) -> Result<bool, Error> {
        trust_store::uninstall(self.store.current().ca_cert_der())
    }
}

impl std::fmt::Debug for CaControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaControl")
            .field("fingerprint", &self.fingerprint())
            .field("confdir", &self.confdir)
            .finish_non_exhaustive()
    }
}

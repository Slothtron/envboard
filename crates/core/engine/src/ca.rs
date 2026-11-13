//! 共享 CA：加载 envboard 自己的根证书、必要时生成、按 SNI 现签叶子证书并缓存。
//!
//! **身份是自己的**：文件叫 `envboard-ca.pem` / `envboard-ca-cert.pem`，
//! Subject 是 `O=envboard, CN=envboard`。v3 早期为了"已装 mitmproxy 证书的客户端
//! 零感知"沿用了 mitmproxy 的文件名与 Subject —— 那个决定的代价是**继承别人的
//! 信任**，已废弃（见 [`LEGACY_CA_BASENAME`]）：旧文件既不加载也不删除，只在
//! 装配时上报一句"旧 CA 已不再使用，客户端需要重装"。
//!
//! 加载判据（有一对文件就要过全关）：
//!
//! 1. 从私钥提出公钥字节，与证书 SPKI 逐字节配对；
//! 2. 证书必须是 CA 且未过期；
//! 3. `-ca.pem` 内嵌的证书（若有）必须与 `-ca-cert.pem` 一致。
//!
//! 任一不满足 → 删除这一对文件并重新生成（幂等）。带着坏 CA 继续运行的症状是
//! "客户端随机证书错误"，响亮重做优于静默带病服务。文件本来就缺 → 生成新 CA
//! 并按同样的文件名写回。
//!
//! 新生成的 CA 用 ECDSA P-256：签发库的 ring 后端不支持生成 RSA 密钥
//! （rcgen 0.14 文档明示只有 aws-lc-rs 能生成），加载既有 RSA CA 不受影响。
//! 对客户端而言可见物只有证书，密钥算法无关。
//!
//! 叶子证书按 SNI 现签现缓存（有界，满则整体清空重签 —— 调试代理里重签是
//! 毫秒级，换来实现的简单）；剩余寿命不足 30 天即重签，避免长驻进程把缓存
//! 变成过期证书的来源。

use std::collections::BTreeMap;
use std::fmt;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, PKCS_RSA_SHA256, SanType,
};
use rustls::pki_types::{
    CertificateDer, PrivateKeyDer, PrivatePkcs1KeyDer, PrivatePkcs8KeyDer, PrivateSec1KeyDer,
};
use rustls::sign::CertifiedKey;
use time::OffsetDateTime;

use crate::der;
use crate::sha256;
use crate::text::parse_ip_literal;
use crate::{Error, ErrorCode};

/// CA 文件名前缀。**自己的名字**：envboard 签发自己的根证书，不再借用
/// mitmproxy 的文件名与 Subject —— 借用意味着「系统里已装 mitmproxy 证书的
/// 机器会自动信任本工具」，那是别人的信任决定，不该由我们继承。
pub const CA_BASENAME: &str = "envboard";

/// v3 之前沿用的 mitmproxy 文件名。**只读不认**：`load_or_create` 不再加载它们，
/// 也不删除（那是别人可能仍在用的材料）；发现它们时按「旧 CA 已失效」上报，
/// 让用户知道客户端需要重装证书。
pub const LEGACY_CA_BASENAME: &str = "mitmproxy";

fn key_path(confdir: &Path) -> PathBuf {
    confdir.join(format!("{CA_BASENAME}-ca.pem"))
}

/// v2/v3 早期写下的私钥文件（只用于判断"旧 CA 还在不在"）。
fn legacy_key_path(confdir: &Path) -> PathBuf {
    confdir.join(format!("{LEGACY_CA_BASENAME}-ca.pem"))
}

/// 客户端要安装的那张证书的路径（只含证书，不含私钥）。
///
/// 公开出来是为了让界面能如实告诉用户"文件在哪"：路径规则只有这一份实现，
/// 界面不许自己拼文件名。
pub fn cert_path(confdir: &Path) -> PathBuf {
    confdir.join(format!("{CA_BASENAME}-ca-cert.pem"))
}

/// v2/v3 早期写下的证书文件（同上，只用于判断）。
fn legacy_cert_path(confdir: &Path) -> PathBuf {
    confdir.join(format!("{LEGACY_CA_BASENAME}-ca-cert.pem"))
}

/// 生成路径的 CA 有效期（年）。
const CA_YEARS: i64 = 10;
/// 叶子证书有效期（天），对齐 mitmproxy 的签发窗口。
const LEAF_DAYS: i64 = 825;
/// 时钟偏移容忍：新证书从「昨天」起有效。
const BACKDATING_DAYS: i64 = 1;
/// 缓存叶子剩余寿命低于此天数即重签。
const REFRESH_MARGIN_DAYS: i64 = 30;
/// SNI 缓存容量；满时整体清空（见模块文档）。
const SNI_CACHE_CAPACITY: usize = 512;

/// 加载结论 —— 重新生成过必须让上层可见（新 CA 意味着客户端可能要重装证书），
/// 禁止静默。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaOutcome {
    Loaded { fingerprint: String },
    Regenerated { fingerprint: String, reason: String },
}

impl CaOutcome {
    pub fn fingerprint(&self) -> &str {
        match self {
            CaOutcome::Loaded { fingerprint } | CaOutcome::Regenerated { fingerprint, .. } => {
                fingerprint
            }
        }
    }
}

struct CachedLeaf {
    certified: Arc<CertifiedKey>,
    not_after_yyyymmdd: i32,
}

/// 一张共享 CA 与它的 SNI 签发缓存。多环境实例共享一个实例即可。
pub struct SharedCa {
    issuer: Issuer<'static, KeyPair>,
    ca_cert_der: Vec<u8>,
    fingerprint: String,
    cache: Mutex<SniCache>,
}

impl fmt::Debug for SharedCa {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedCa")
            .field("fingerprint", &self.fingerprint)
            .field(
                "cached_leaves",
                &self
                    .cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entries
                    .len(),
            )
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct SniCache {
    entries: BTreeMap<String, CachedLeaf>,
}

/// 共享 CA 的**可变所有权**：引擎按连接读取当前值，重新签发 = 整体换入。
///
/// 为什么不把 `Arc<SharedCa>` 直接塞进引擎：重新签发要求**正在跑的实例立刻
/// 换新证书**，否则客户端装的是新 CA、拿到的却是旧叶子证书（握手失败）。
/// 每个 CONNECT 取一次当前值，旋转就不需要重启环境 —— 代价是每条 CONNECT
/// 重建一次 `ServerConfig`（几十微秒，和 TLS 握手本身比可以忽略）。
#[derive(Clone)]
pub struct CaStore {
    inner: Arc<arc_swap::ArcSwap<SharedCa>>,
}

impl CaStore {
    pub fn new(ca: Arc<SharedCa>) -> Self {
        Self {
            inner: Arc::new(arc_swap::ArcSwap::from(ca)),
        }
    }

    /// 当前生效的 CA。拿到的是快照：调用方在一次握手里看到的是同一张证书。
    pub fn current(&self) -> Arc<SharedCa> {
        self.inner.load_full()
    }

    /// 整体换入新 CA（生效于下一次 CONNECT）。
    pub fn replace(&self, ca: Arc<SharedCa>) {
        self.inner.store(ca);
    }
}

impl fmt::Debug for CaStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CaStore").finish_non_exhaustive()
    }
}

impl SharedCa {
    /// 就绪一个共享 CA：能加载就加载，不能就重新物化（判据见模块文档）。
    ///
    /// **只认自己的文件**。旧 mitmproxy 文件存在时本次会生成新 CA，并带回一条
    /// `Regenerated` —— 已安装旧证书的客户端因此需要重装，这件事不允许静默。
    pub fn load_or_create(confdir: &Path) -> Result<(Arc<SharedCa>, CaOutcome), Error> {
        std::fs::create_dir_all(confdir)
            .map_err(|e| Error::io_at("create the CA directory", confdir, e))?;
        let key_file = key_path(confdir);
        let cert_file = cert_path(confdir);
        if key_file.exists() && cert_file.exists() {
            let key_pem = std::fs::read(&key_file)
                .map_err(|e| Error::io_at("read the CA key", &key_file, e))?;
            let cert_pem = std::fs::read(&cert_file)
                .map_err(|e| Error::io_at("read the CA certificate", &cert_file, e))?;
            match Self::load(&key_pem, &cert_pem) {
                Ok(ca) => {
                    let fingerprint = ca.fingerprint.clone();
                    return Ok((Arc::new(ca), CaOutcome::Loaded { fingerprint }));
                }
                Err(reason) => {
                    let _ = std::fs::remove_file(&key_file);
                    let _ = std::fs::remove_file(&cert_file);
                    let ca = Self::generate_into(confdir)?;
                    let fingerprint = ca.fingerprint.clone();
                    return Ok((
                        Arc::new(ca),
                        CaOutcome::Regenerated {
                            fingerprint,
                            reason,
                        },
                    ));
                }
            }
        }
        let ca = Self::generate_into(confdir)?;
        let fingerprint = ca.fingerprint.clone();
        // 旧文件还在 = 这是从"沿用 mitmproxy CA"升级上来的机器：旧证书装过的
        // 客户端从此不认我们的新叶子证书，必须让上层把话说出来。
        if legacy_key_path(confdir).exists() || legacy_cert_path(confdir).exists() {
            return Ok((
                Arc::new(ca),
                CaOutcome::Regenerated {
                    fingerprint,
                    reason: format!(
                        "legacy {LEGACY_CA_BASENAME} CA is no longer reused; envboard now issues its own CA"
                    ),
                },
            ));
        }
        Ok((Arc::new(ca), CaOutcome::Loaded { fingerprint }))
    }

    /// **重新签发**：换一张全新的自签根证书，覆盖状态目录里的两个文件。
    ///
    /// 调用方负责让客户端知道要重装（指纹变了）。旧证书不会被删除 —— 它可能还
    /// 躺在系统信任库里，本工具不替用户决定什么时候清理别人的信任。
    pub fn rotate(confdir: &Path) -> Result<(Arc<SharedCa>, CaOutcome), Error> {
        std::fs::create_dir_all(confdir)
            .map_err(|e| Error::io_at("create the CA directory", confdir, e))?;
        let ca = Self::generate_into(confdir)?;
        let fingerprint = ca.fingerprint.clone();
        Ok((
            Arc::new(ca),
            CaOutcome::Regenerated {
                fingerprint,
                reason: "rotated on request".to_string(),
            },
        ))
    }

    /// 只加载、不生成（测试与诊断入口）。输入是两份 PEM 的原始字节。
    pub fn load(key_pem: &[u8], cert_pem: &[u8]) -> Result<SharedCa, String> {
        let (key_der, embedded_cert) = split_key_and_cert(key_pem)?;
        let cert_der = single_cert(cert_pem)?;
        // mitmproxy 形态里 -ca.pem 内嵌的证书与 -ca-cert.pem 是同一张；
        // 不一致说明 confdir 混了两代 CA，按坏 CA 处理。
        if !embedded_cert.is_empty() && embedded_cert != cert_der {
            return Err(
                "mitmproxy-ca.pem embeds a different certificate than mitmproxy-ca-cert.pem"
                    .to_string(),
            );
        }
        let public_from_key = der::public_key_octets_from_private(&key_der)
            .ok_or_else(|| "private key container is not a supported RSA/EC shape".to_string())?;
        let public_from_cert = der::cert_public_key_octets(&cert_der)
            .ok_or_else(|| "certificate subjectPKInfo is not a supported shape".to_string())?;
        if public_from_key != public_from_cert {
            return Err("CA private key does not pair with the CA certificate".to_string());
        }
        if !der::cert_is_ca(&cert_der) {
            return Err(
                "the loaded certificate is not a CA (no basicConstraints CA:TRUE)".to_string(),
            );
        }
        if cert_is_expired(&cert_der) {
            return Err("the loaded CA certificate has expired".to_string());
        }
        let key_pair = key_pair_from(&key_der)?;
        let issuer = Issuer::from_ca_cert_der(&CertificateDer::from(cert_der.clone()), key_pair)
            .map_err(|e| format!("cannot construct issuer from the CA certificate: {e}"))?;
        Ok(SharedCa {
            issuer,
            ca_cert_der: cert_der,
            fingerprint: sha256::hex(&public_from_cert),
            cache: Mutex::new(SniCache::default()),
        })
    }

    /// 生成新 CA（ECDSA P-256，理由见模块文档）并写盘：私钥 pem + 证书 pem 拼进
    /// `-ca.pem`，证书 pem 单独进 `-ca-cert.pem`（沿用 mitmproxy 的拼接形态，
    /// 只为让"既有的解析路径"与手工排查习惯继续成立；文件名与 Subject 都是自己的）。
    fn generate_into(confdir: &Path) -> Result<SharedCa, Error> {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
            .map_err(|e| internal(format!("cannot generate the CA key: {e}")))?;
        let mut params = ca_params();
        params.distinguished_name = envboard_subject();
        let cert = params
            .self_signed(&key)
            .map_err(|e| internal(format!("cannot self-sign the CA certificate: {e}")))?;
        let cert_der = cert.der().to_vec();

        // 与 mitmproxy 相同的拼接顺序：先私钥后证书。私钥文件 0600。
        let mut bundle = Vec::new();
        bundle.extend_from_slice(key.serialize_pem().as_bytes());
        bundle.extend_from_slice(cert.pem().as_bytes());
        write_secret(&key_path(confdir), &bundle)?;
        let cert_only = cert.pem();
        let ca = SharedCa {
            // Issuer 需要按值持有 params；证书已序列化，重设一遍 subject 无副作用。
            issuer: Issuer::new(params, key),
            ca_cert_der: cert_der,
            fingerprint: String::new(),
            cache: Mutex::new(SniCache::default()),
        };
        let cert_file = cert_path(confdir);
        std::fs::write(&cert_file, cert_only.as_bytes())
            .map_err(|e| Error::io_at("write the CA certificate", &cert_file, e))?;
        let mut ca = ca;
        ca.fingerprint = der::cert_public_key_octets(&ca.ca_cert_der)
            .map(|public| sha256::hex(&public))
            .ok_or_else(|| {
                internal("freshly generated CA cert has an unreadable SPKI".to_string())
            })?;
        Ok(ca)
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// CA 证书 DER（信任库导入与工作台展示用；私钥永不外流）。
    pub fn ca_cert_der(&self) -> &[u8] {
        &self.ca_cert_der
    }

    /// 设置页「证书信息」卡的只读摘要。证书形状在 load/生成时都已过校验，
    /// 理论上不会是 None；万一结构异常就返回 None，由工作台降级展示。
    pub fn info(&self) -> Option<crate::der::CertInfo> {
        crate::der::cert_info(&self.ca_cert_der)
    }

    /// 证书 PEM（下载端点的载荷）。DER → base64 就地二十行，不为此引依赖。
    pub fn ca_cert_pem(&self) -> String {
        pem("CERTIFICATE", &self.ca_cert_der)
    }

    /// 为 host 产出（或复用）一张叶子证书。
    pub fn issue(&self, host: &str) -> Result<Arc<CertifiedKey>, Error> {
        let now = OffsetDateTime::now_utc();
        let today = yyyymmdd(now);
        let refresh_before = today + REFRESH_MARGIN_DAYS as i32;
        if let Some(hit) = self.cache.lock().unwrap().entries.get(host)
            && hit.not_after_yyyymmdd > refresh_before
        {
            return Ok(hit.certified.clone());
        }
        let leaf_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
            .map_err(|e| internal(format!("cannot generate the leaf key: {e}")))?;
        let params = leaf_params(host)?;
        let not_after = yyyymmdd(params.not_after);
        let cert = params
            .signed_by(&leaf_key, &self.issuer)
            .map_err(|e| internal(format!("cannot sign the leaf certificate: {e}")))?;
        let signing = leaf_signing_key(&leaf_key)?;
        let certified = Arc::new(CertifiedKey::new(
            vec![CertificateDer::from(cert.der().to_vec())],
            signing,
        ));
        let mut cache = self.cache.lock().unwrap();
        if cache.entries.len() >= SNI_CACHE_CAPACITY {
            cache.entries.clear();
        }
        cache.entries.insert(
            host.to_string(),
            CachedLeaf {
                certified: certified.clone(),
                not_after_yyyymmdd: not_after,
            },
        );
        Ok(certified)
    }

    /// 为指定域名**签发一张可导出的叶子证书**（证书 PEM + 私钥 PEM）。
    ///
    /// 与 [`SharedCa::issue`] 的分工：那条路径服务 MITM 握手、私钥永不外流；
    /// 这一条是用户显式下达的动作，产物就是交给用户的（验证签发链、装进别的
    /// 程序、手工排查）。**不复用 SNI 缓存** —— 显式签一张就该是独立的一张，
    /// 每次给新的密钥对。
    pub fn issue_pem(&self, host: &str) -> Result<(String, String), Error> {
        let host = host.trim();
        if host.is_empty() {
            return Err(Error::new(
                ErrorCode::InvalidConfig,
                "leaf host must not be empty".to_string(),
            ));
        }
        let leaf_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
            .map_err(|e| internal(format!("cannot generate the leaf key: {e}")))?;
        let params = leaf_params(host)?;
        let cert = params
            .signed_by(&leaf_key, &self.issuer)
            .map_err(|e| internal(format!("cannot sign the leaf certificate: {e}")))?;
        Ok((cert.pem(), leaf_key.serialize_pem()))
    }
}

/// 叶子证书的参数：CN=host、单个 SAN（IP 字面量走 iPAddress），服务端认证用途。
fn leaf_params(host: &str) -> Result<CertificateParams, Error> {
    let now = OffsetDateTime::now_utc();
    let mut params = CertificateParams::default();
    params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, host);
        dn
    };
    params.subject_alt_names = vec![san_for(host)?];
    params.is_ca = IsCa::NoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.not_before = now - time::Duration::days(BACKDATING_DAYS);
    params.not_after = now + time::Duration::days(LEAF_DAYS);
    Ok(params)
}

/// host → SAN：IP 字面量走 iPAddress，其余按 DNS 名（校验交给签发库）。
fn san_for(host: &str) -> Result<SanType, Error> {
    if let Some(ip) = parse_ip_literal(host) {
        return Ok(SanType::IpAddress(ip));
    }
    host.to_string()
        .try_into()
        .map(SanType::DnsName)
        .map_err(|e| {
            Error::new(
                ErrorCode::InvalidConfig,
                format!("bad leaf subject {host:?}: {e}"),
            )
        })
}

fn ca_params() -> CertificateParams {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
    ];
    params.extended_key_usages = Vec::new();
    let now = OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::days(BACKDATING_DAYS);
    params.not_after = now + time::Duration::days(CA_YEARS * 365);
    params
}

/// 自己的 Subject（O=envboard, CN=envboard）。改它等于换 CA 身份，客户端全部要重装。
fn envboard_subject() -> DistinguishedName {
    let mut dn = DistinguishedName::new();
    dn.push(DnType::OrganizationName, CA_BASENAME);
    dn.push(DnType::CommonName, CA_BASENAME);
    dn
}

/// PEM 流里的第一把私钥与第一张证书（mitmproxy-ca.pem 就是两者的拼接）。
fn split_key_and_cert(pem: &[u8]) -> Result<(der::PrivateKeyDerish, Vec<u8>), String> {
    let mut reader = BufReader::new(pem);
    let mut key: Option<der::PrivateKeyDerish> = None;
    let mut cert: Vec<u8> = Vec::new();
    while let Some(item) = rustls_pemfile::read_one(&mut reader).map_err(|e| e.to_string())? {
        match item {
            rustls_pemfile::Item::Pkcs1Key(k) => {
                if key.is_none() {
                    key = Some(der::PrivateKeyDerish::Pkcs1(k.secret_pkcs1_der().to_vec()));
                }
            }
            rustls_pemfile::Item::Pkcs8Key(k) => {
                if key.is_none() {
                    key = Some(der::PrivateKeyDerish::Pkcs8(k.secret_pkcs8_der().to_vec()));
                }
            }
            rustls_pemfile::Item::Sec1Key(k) => {
                if key.is_none() {
                    key = Some(der::PrivateKeyDerish::Sec1(k.secret_sec1_der().to_vec()));
                }
            }
            rustls_pemfile::Item::X509Certificate(c) if cert.is_empty() => {
                cert = c.to_vec();
            }
            _ => {}
        }
    }
    let key = key.ok_or_else(|| "no private key in the PEM bundle".to_string())?;
    if cert.is_empty() {
        return Err("no certificate in the PEM bundle".to_string());
    }
    Ok((key, cert))
}

fn single_cert(pem: &[u8]) -> Result<Vec<u8>, String> {
    let mut reader = BufReader::new(pem);
    while let Some(item) = rustls_pemfile::read_one(&mut reader).map_err(|e| e.to_string())? {
        if let rustls_pemfile::Item::X509Certificate(c) = item {
            return Ok(c.to_vec());
        }
    }
    Err("no certificate found".to_string())
}

/// 三形态私钥 → 签发库 KeyPair。PKCS#1 RSA 走「直喂，失败则转 PKCS#8 重喂」：
/// 签发库对 PKCS#1 的支持随版本变化，转换层把这条兼容风险关在编译期之外
/// （见模块文档的依赖纪律）。
fn key_pair_from(key: &der::PrivateKeyDerish) -> Result<KeyPair, String> {
    match key {
        der::PrivateKeyDerish::Pkcs1(bytes) => {
            let owned = bytes.clone();
            let direct = PrivateKeyDer::Pkcs1(PrivatePkcs1KeyDer::from(owned.clone()));
            match KeyPair::from_der_and_sign_algo(&direct, &PKCS_RSA_SHA256) {
                Ok(kp) => Ok(kp),
                Err(first) => {
                    let pkcs8 = der::pkcs1_to_pkcs8_rsa(&owned);
                    let wrapped = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pkcs8));
                    KeyPair::from_der_and_sign_algo(&wrapped, &PKCS_RSA_SHA256).map_err(|e| {
                        format!("both PKCS#1 and converted PKCS#8 failed: {first} / {e}")
                    })
                }
            }
        }
        der::PrivateKeyDerish::Pkcs8(bytes) => {
            let (alg_tlv, _) = der::pkcs8_split(bytes).ok_or("unparsable PKCS#8")?;
            let alg = if der::is_rsa_alg(alg_tlv) {
                &PKCS_RSA_SHA256
            } else {
                &PKCS_ECDSA_P256_SHA256
            };
            let owned = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(bytes.clone()));
            KeyPair::from_der_and_sign_algo(&owned, alg).map_err(|e| e.to_string())
        }
        der::PrivateKeyDerish::Sec1(bytes) => {
            let owned = PrivateKeyDer::Sec1(PrivateSec1KeyDer::from(bytes.clone()));
            KeyPair::from_der_and_sign_algo(&owned, &PKCS_ECDSA_P256_SHA256)
                .map_err(|e| e.to_string())
        }
    }
}

fn leaf_signing_key(key: &KeyPair) -> Result<Arc<dyn rustls::sign::SigningKey>, Error> {
    let pkcs8 = key.serialize_der();
    let der_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pkcs8));
    rustls::crypto::ring::sign::any_supported_type(&der_key)
        .map_err(|e| internal(format!("leaf key is not usable for TLS signing: {e}")))
}

/// 证书 notAfter（UTCTime/GeneralizedTime 的 ASCII 数字）→ yyyymmdd。
/// 读不出来按「已过期」处理：宁可判坏 CA 重新物化，不可带着解析不了的证书服务。
fn cert_is_expired(cert_der: &[u8]) -> bool {
    let Some(raw) = der::cert_not_after(cert_der) else {
        return true;
    };
    match not_after_yyyymmdd(raw) {
        Some(yyyymmdd) => yyyymmdd < today_yyyymmdd(),
        None => true,
    }
}

fn not_after_yyyymmdd(raw: &[u8]) -> Option<i32> {
    let text = std::str::from_utf8(raw).ok()?;
    let digits: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
    let (year, rest) = if digits.len() >= 14 {
        (digits[..4].parse::<i32>().ok()?, &digits[4..])
    } else if digits.len() >= 12 {
        (2000 + digits[..2].parse::<i32>().ok()?, &digits[2..])
    } else {
        return None;
    };
    let month: i32 = rest[..2].parse().ok()?;
    let day: i32 = rest[2..4].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    year.checked_mul(10000)?
        .checked_add(month * 100)?
        .checked_add(day)
}

fn yyyymmdd(moment: OffsetDateTime) -> i32 {
    let date = moment.date();
    date.year() * 10000 + i32::from(date.month() as u8) * 100 + i32::from(date.day())
}

fn today_yyyymmdd() -> i32 {
    yyyymmdd(OffsetDateTime::now_utc())
}

fn write_secret(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| Error::io_at("write the CA key", path, e))?;
        file.write_all(bytes)
            .map_err(|e| Error::io_at("write the CA key", path, e))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes).map_err(|e| Error::io_at("write the CA key", path, e))?;
        Ok(())
    }
}

fn internal(message: String) -> Error {
    Error::new(ErrorCode::InternalError, message)
}

// ---- PEM 编码（只服务 ca_cert_pem；RFC 7468 的固定形状子集）----

const BASE64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b1 = u32::from(chunk[0]);
        let b2 = u32::from(*chunk.get(1).unwrap_or(&0));
        let b3 = u32::from(*chunk.get(2).unwrap_or(&0));
        let n = (b1 << 16) | (b2 << 8) | b3;
        out.push(BASE64_TABLE[(n >> 18) as usize & 63] as char);
        out.push(BASE64_TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            BASE64_TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            BASE64_TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn pem(label: &str, der: &[u8]) -> String {
    let mut out = format!("-----BEGIN {label}-----\n");
    let b64 = base64(der);
    for line in b64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(line).unwrap_or(""));
        out.push('\n');
    }
    out.push_str("-----END ");
    out.push_str(label);
    out.push_str("-----\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_ca() -> SharedCa {
        let key = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/mitmproxy-compat/mitmproxy-ca.pem"
        ))
        .expect("测试资产 mitmproxy-ca.pem 不在仓库里");
        let cert = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/mitmproxy-compat/mitmproxy-ca-cert.pem"
        ))
        .expect("测试资产 mitmproxy-ca-cert.pem 不在仓库里");
        SharedCa::load(&key, &cert).expect("fixture CA 必须能加载")
    }

    /// 设置页「证书信息」的数据源钉在仓库里的假 CA 上：形状读取一旦被
    /// 上游结构变化打破，这里先红。
    #[test]
    fn cert_info_reads_the_fixture_ca() {
        let info = fixture_ca().info().expect("fixture CA 的 DER 形状必须可读");
        assert_eq!(info.version, 3);
        assert!(info.is_ca);
        assert!(!info.serial_hex.is_empty());
        assert_eq!(info.not_before.len(), "YYYY-MM-DD HH:MM:SSZ".len());
        assert_eq!(info.not_after.len(), "YYYY-MM-DD HH:MM:SSZ".len());
        assert!(!info.sig_alg.is_empty());
        assert!(!info.pubkey_alg.is_empty());
        assert!(
            !info.common_name.unwrap_or_default().is_empty(),
            "fixture CA 必须有 CN"
        );
    }

    /// 测试用临时 confdir：建在**测试二进制自己所在目录**（`target/…/deps/`）下。
    /// 不用 `temp_dir()`：Windows 上它读 `TMP`，Git Bash 传下来的是 `/tmp`
    /// （被解释成 `C:\tmp`，写它要管理员）。
    fn scratch_confdir(tag: &str) -> PathBuf {
        let exe = std::env::current_exe().expect("test binary path");
        let dir = exe
            .parent()
            .expect("test binary has a parent")
            .join(format!("ca-test-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("fixture dir");
        dir
    }

    fn legacy_fixture_bytes() -> (Vec<u8>, Vec<u8>) {
        let key = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/mitmproxy-compat/mitmproxy-ca.pem"
        ))
        .expect("测试资产 mitmproxy-ca.pem 不在仓库里");
        let cert = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/mitmproxy-compat/mitmproxy-ca-cert.pem"
        ))
        .expect("测试资产 mitmproxy-ca-cert.pem 不在仓库里");
        (key, cert)
    }

    /// 新 confdir：生成的是**自己的**文件与 Subject，不是 mitmproxy 的那一套。
    #[test]
    fn a_fresh_confdir_gets_an_envboard_named_ca() {
        let dir = scratch_confdir("fresh");
        let (ca, outcome) = SharedCa::load_or_create(&dir).expect("fresh CA");
        assert!(matches!(outcome, CaOutcome::Loaded { .. }), "{outcome:?}");
        assert!(
            dir.join("envboard-ca.pem").exists(),
            "私钥文件名必须是自己的"
        );
        assert!(dir.join("envboard-ca-cert.pem").exists());
        assert!(
            !dir.join("mitmproxy-ca.pem").exists(),
            "不该再写 mitmproxy 名字"
        );
        let info = ca.info().expect("CA 摘要");
        assert_eq!(info.common_name.as_deref(), Some("envboard"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 旧 mitmproxy 文件**不加载也不删除**，并且要上报"旧 CA 已不再使用"。
    #[test]
    fn legacy_mitmproxy_files_are_ignored_not_reused() {
        let dir = scratch_confdir("legacy");
        let (key, cert) = legacy_fixture_bytes();
        std::fs::write(dir.join("mitmproxy-ca.pem"), &key).unwrap();
        std::fs::write(dir.join("mitmproxy-ca-cert.pem"), &cert).unwrap();

        let (ca, outcome) = SharedCa::load_or_create(&dir).expect("legacy CA");
        match &outcome {
            CaOutcome::Regenerated { reason, .. } => {
                assert!(reason.contains("legacy"), "理由必须点明旧 CA：{reason}");
            }
            other => panic!("旧 CA 存在时必须如实上报重新生成，实际 {other:?}"),
        }
        assert_eq!(
            ca.info().and_then(|info| info.common_name).as_deref(),
            Some("envboard")
        );
        assert!(dir.join("mitmproxy-ca.pem").exists(), "别人的文件不许删");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 重新签发：换一张新 CA 并覆盖文件，重新加载拿到的是新的那张。
    #[test]
    fn rotate_replaces_the_ca_and_its_files() {
        let dir = scratch_confdir("rotate");
        let (first, _) = SharedCa::load_or_create(&dir).expect("first CA");
        let first_fingerprint = first.fingerprint().to_string();

        let (rotated, outcome) = SharedCa::rotate(&dir).expect("rotate");
        assert!(
            matches!(outcome, CaOutcome::Regenerated { .. }),
            "{outcome:?}"
        );
        assert_ne!(rotated.fingerprint(), first_fingerprint, "指纹必须变");

        let (reloaded, _) = SharedCa::load_or_create(&dir).expect("reload");
        assert_eq!(
            reloaded.fingerprint(),
            rotated.fingerprint(),
            "文件必须被新 CA 覆盖"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 指定域名签发：产出一张可导出的叶子证书，CN/SAN 是请求的域名，
    /// 私钥与证书配对，且不是 CA。
    #[test]
    fn issue_pem_signs_a_leaf_for_the_requested_host() {
        let dir = scratch_confdir("leaf");
        let (ca, _) = SharedCa::load_or_create(&dir).expect("CA");
        let (cert_pem, key_pem) = ca.issue_pem("api.example.com").expect("leaf");

        assert!(cert_pem.starts_with("-----BEGIN CERTIFICATE-----"));
        assert!(key_pem.contains("PRIVATE KEY"));

        let leaf_der = single_cert(cert_pem.as_bytes()).expect("leaf cert DER");
        let info = crate::der::cert_info(&leaf_der).expect("leaf info");
        // `CertInfo` 的 DN 字段取的是**颁发者**（自签 CA 上它等于主体），所以这里的
        // CN 是 envboard；域名要看 SAN。
        assert_eq!(info.common_name.as_deref(), Some("envboard"));
        assert!(
            info.san.iter().any(|name| name == "api.example.com"),
            "SAN 必须带上请求的域名：{:?}",
            info.san
        );
        assert!(!info.is_ca);

        // 配对由签发路径保证（同一把 `KeyPair` 生成证书），这里只钉"确实给了私钥"。
        assert!(
            key_pem.starts_with("-----BEGIN ") && key_pem.contains("PRIVATE KEY-----"),
            "导出必须带私钥：{}",
            key_pem.lines().next().unwrap_or("")
        );

        // IP 字面量走 iPAddress SAN，不能当成 DNS 名。
        assert!(ca.issue_pem("127.0.0.1").is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// PEM 载荷的形状：BEGIN/END 成对、正文行不超过 64 字符（RFC 7468 惯例）。
    #[test]
    fn ca_cert_pem_has_canonical_shape() {
        let pem_text = fixture_ca().ca_cert_pem();
        assert!(pem_text.starts_with("-----BEGIN CERTIFICATE-----\n"));
        assert!(pem_text.ends_with("-----END CERTIFICATE-----\n"));
        for line in pem_text.lines() {
            assert!(
                line.starts_with("-----") || line.len() <= 64,
                "PEM 正文行超长：{line}"
            );
        }
    }
}

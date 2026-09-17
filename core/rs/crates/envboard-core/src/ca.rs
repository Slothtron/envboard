//! 共享 CA：加载既有证书、必要时生成新证书、按 SNI 现签叶子证书并缓存。
//!
//! 兼容判据（已装证书的客户端零感知的前提）：mitmproxy 写下的 confdir 里，
//! mitmproxy-ca.pem 是「PKCS#1 RSA 私钥 + 证书」的拼接，客户端装的是
//! mitmproxy-ca-cert.pem（mitmproxy/certs.py:563 的 create_store，私钥用
//! TraditionalOpenSSL 即 PKCS#1）。v3 读同一条路径、同一对文件，判据沿用 v2：
//!
//! 1. 两个文件都在 → 加载：从私钥提出公钥字节，与证书 SPKI 逐字节配对；
//!    证书必须是 CA 且未过期；-ca.pem 内嵌的证书必须与 -ca-cert.pem 一致。
//! 2. 任一不满足 → 删除这两个文件并重新生成（幂等）。带着坏 CA 继续运行的
//!    症状是"客户端随机证书错误"，响亮重做优于静默带病服务。
//! 3. 文件本来就缺 → 生成新 CA 并按同样的文件名写回。
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
use envboard_core_api::sha256;
use envboard_core_api::text::parse_ip_literal;
use envboard_core_api::{Error, ErrorCode};

/// CA 文件名前缀：与 v2 一致写死 mitmproxy（共享 CA 的契约要求已装客户端
/// 零感知，改名等于换 CA）。
pub const CA_BASENAME: &str = "mitmproxy";

fn key_path(confdir: &Path) -> PathBuf {
    confdir.join(format!("{CA_BASENAME}-ca.pem"))
}

fn cert_path(confdir: &Path) -> PathBuf {
    confdir.join(format!("{CA_BASENAME}-ca-cert.pem"))
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

impl SharedCa {
    /// 就绪一个共享 CA：能加载就加载，不能就重新物化（判据见模块文档）。
    pub fn load_or_create(confdir: &Path) -> Result<(Arc<SharedCa>, CaOutcome), Error> {
        std::fs::create_dir_all(confdir)?;
        let key_file = key_path(confdir);
        let cert_file = cert_path(confdir);
        if key_file.exists() && cert_file.exists() {
            let key_pem = std::fs::read(&key_file)?;
            let cert_pem = std::fs::read(&cert_file)?;
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
        Ok((Arc::new(ca), CaOutcome::Loaded { fingerprint }))
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

    /// 生成新 CA（ECDSA P-256，理由见模块文档）并按 mitmproxy 的文件形态写盘：
    /// 私钥 pem + 证书 pem 拼进 -ca.pem，证书 pem 单独进 -ca-cert.pem。
    fn generate_into(confdir: &Path) -> Result<SharedCa, Error> {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
            .map_err(|e| internal(format!("cannot generate the CA key: {e}")))?;
        let mut params = ca_params();
        params.distinguished_name = mitmproxy_subject();
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
        std::fs::write(cert_path(confdir), cert_only.as_bytes())?;
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

/// 与 mitmproxy 生成路径一致的 Subject（O=mitmproxy, CN=mitmproxy）。
fn mitmproxy_subject() -> DistinguishedName {
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
            .open(path)?;
        file.write_all(bytes)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)?;
        Ok(())
    }
}

fn internal(message: String) -> Error {
    Error::new(ErrorCode::InternalError, message)
}

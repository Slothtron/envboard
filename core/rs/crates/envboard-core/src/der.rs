//! 极简 DER 工具：只做 CA 兼容层需要的三件事 —— 读 TLV、从私钥提出公钥字节、
//! PKCS#1 ↔ PKCS#8 的 RSA 封装互转。刻意不引入通用 ASN.1 库：这里没有"解析任意
//! DER"的需求，每一步都只走固定形状（RSA/EC 私钥的标准结构），失败即 `None`，
//! 由调用方决定响亮报错。
//!
//! 为什么手写而不是让签发库直接吃 PKCS#1：加载既有 mitmproxy CA 是硬兼容约束
//! （已装证书的客户端零感知），而 mitmproxy 写出的私钥是 PKCS#1 `RSAPrivateKey`
//! （`mitmproxy/certs.py:563` 的 `TraditionalOpenSSL`）。签发库对 PKCS#1 的支持
//! 随版本变化，中间放一层自己说了算的转换，兼容面就不被上游牵着走。

/// 一个 TLV：tag 字节 + value 切片 + **整段 TLV 的原始字节**（复制子元素时
/// 直接用它，不用重新编码长度）。
pub type Tlv<'a> = (u8, &'a [u8], &'a [u8]);

/// 一个 TLV：tag 字节 + value 切片 + 剩余输入。只接受确定长度（DER 子集）。
pub fn read_tlv(buf: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = buf.split_first()?;
    let (&first, rest) = rest.split_first()?;
    // len 与"长度字节之后的数据"必须一起定下来；先前版本把 rest 遮蔽在
    // 分支里，多字节长度的 value 切片会偏移 —— 单测 tlv_round_trips_multibyte_lengths
    // 钉住这个形状。
    let (len, body) = if first & 0x80 == 0 {
        (usize::from(first), rest)
    } else {
        let n = usize::from(first & 0x7f);
        if n == 0 || n > 4 || rest.len() < n {
            return None;
        }
        let (len_bytes, tail) = rest.split_at(n);
        let mut acc = 0usize;
        for byte in len_bytes {
            acc = (acc << 8) | usize::from(*byte);
        }
        (acc, tail)
    };
    if body.len() < len {
        return None;
    }
    Some((tag, &body[..len], &body[len..]))
}

/// 遍历一个容器（SEQUENCE 等）的**直接子 TLV**。返回 (tag, value, 整段子 TLV
/// 的原始字节) —— 复制公钥 INTEGER 时要用整段。形状不对即返回空（调用方报错）。
pub fn children(container: &[u8]) -> Vec<(u8, &[u8], &[u8])> {
    let mut out = Vec::new();
    let mut rest = container;
    while !rest.is_empty() {
        let Some((tag, value, tail)) = read_tlv(rest) else {
            return Vec::new();
        };
        out.push((tag, value, &rest[..rest.len() - tail.len()]));
        rest = tail;
    }
    out
}

/// DER：tag + 长度 + value（长度编码处理 >127 的情况）。
pub fn tlv(tag: u8, value: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    if value.len() < 0x80 {
        out.push(value.len() as u8);
    } else {
        let bytes = value.len().to_be_bytes();
        let first = bytes
            .iter()
            .position(|b| *b != 0)
            .unwrap_or(bytes.len() - 1);
        let trimmed = &bytes[first..];
        out.push(0x80 | trimmed.len() as u8);
        out.extend_from_slice(trimmed);
    }
    out.extend_from_slice(value);
    out
}

/// RSA 公钥的 DER：`SEQUENCE { modulus INTEGER, publicExponent INTEGER }`。
///
/// 输入是 PKCS#1 `RSAPrivateKey` DER —— 前三个 INTEGER 依次是
/// version / modulus / publicExponent，公钥即第 2、3 个子 TLV 原样拼接。
pub fn rsa_public_from_pkcs1(pkcs1: &[u8]) -> Option<Vec<u8>> {
    let (tag, seq, rest) = read_tlv(pkcs1)?;
    if tag != SEQUENCE || !rest.is_empty() {
        return None;
    }
    let kids = children(seq);
    if kids.len() < 3 || kids[1].0 != INTEGER || kids[2].0 != INTEGER {
        return None;
    }
    let mut body = Vec::new();
    body.extend_from_slice(kids[1].2);
    body.extend_from_slice(kids[2].2);
    Some(tlv(SEQUENCE, &body))
}

/// 从 SEC1 `ECPrivateKey` DER 提出公钥点字节（`[1] BIT STRING` 去掉首个
/// "未用位数"字节后的内容）。
pub fn ec_public_from_sec1(sec1: &[u8]) -> Option<Vec<u8>> {
    let (tag, seq, rest) = read_tlv(sec1)?;
    if tag != SEQUENCE || !rest.is_empty() {
        return None;
    }
    for (_t, value, _raw) in children(seq) {
        let kids_tag = _t;
        if kids_tag == CONTEXT_CONSTRUCTED_1 {
            let (bit_tag, bits, _) = read_tlv(value)?;
            if bit_tag != BIT_STRING {
                return None;
            }
            let (_, point) = bits.split_first()?;
            return Some(point.to_vec());
        }
    }
    None
}

/// 从 PKCS#8 `PrivateKeyInfo` 拆出 (算法标识整段 TLV, 私钥本体 value)。
pub fn pkcs8_split(der: &[u8]) -> Option<(&[u8], &[u8])> {
    let (tag, seq, rest) = read_tlv(der)?;
    if tag != SEQUENCE || !rest.is_empty() {
        return None;
    }
    let kids = children(seq);
    if kids.len() < 3 {
        return None;
    }
    Some((kids[1].2, kids[2].1))
}

/// 把 PKCS#1 RSA 私钥封装成 PKCS#8 `PrivateKeyInfo`。
pub fn pkcs1_to_pkcs8_rsa(pkcs1: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&tlv(INTEGER, &[0])); // version
    body.extend_from_slice(RSA_ALG_IDENTIFIER); // AlgorithmIdentifier(rsaEncryption, NULL)
    body.extend_from_slice(&tlv(OCTET_STRING, pkcs1)); // privateKey
    tlv(SEQUENCE, &body)
}

// ---- 标签常量（X.690）----
pub const SEQUENCE: u8 = 0x30;
pub const INTEGER: u8 = 0x02;
pub const BIT_STRING: u8 = 0x03;
pub const OCTET_STRING: u8 = 0x04;
#[allow(dead_code)]
pub const OBJECT_ID: u8 = 0x06;
const UTF8_STRING: u8 = 0x0c;
const PRINTABLE_STRING: u8 = 0x13;
const IA5_STRING: u8 = 0x16;
#[allow(dead_code)]
pub const CONTEXT_CONSTRUCTED_0: u8 = 0xa0;
pub const CONTEXT_CONSTRUCTED_1: u8 = 0xa1;
/// GeneralName 的 context 原语标签：dNSName 与 iPAddress。
const DNS_NAME: u8 = 0x82;
const IP_ADDRESS: u8 = 0x87;

/// rsaEncryption 的完整 AlgorithmIdentifier（含 NULL parameters），15 字节常量。
pub const RSA_ALG_IDENTIFIER: &[u8] = &[
    0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01, 0x05, 0x00,
];

/// 算法标识是否就是 rsaEncryption。
pub fn is_rsa_alg(alg_tlv: &[u8]) -> bool {
    alg_tlv == RSA_ALG_IDENTIFIER
}

// ---- X.509 证书结构读取（只走固定形状，理由见模块文档）----

/// TBS 里可选的 version [0]EXPLICIT 之后的固定次序子元素：
/// serial, sigAlg, issuer, validity, subject, subjectPKI[, issuerUID, subjectUID, extensions]。
fn after_version(cert_der: &[u8]) -> Option<Vec<Tlv<'_>>> {
    let (tag, cert_seq, rest) = read_tlv(cert_der)?;
    if tag != SEQUENCE || !rest.is_empty() {
        return None;
    }
    let outer = children(cert_seq);
    let (tbs_tag, tbs, _) = read_tlv(outer.first()?.2)?;
    if tbs_tag != SEQUENCE {
        return None;
    }
    let kids = children(tbs);
    let start = usize::from(kids.first()?.0 == CONTEXT_CONSTRUCTED_0);
    if kids.len() < start + 6 {
        return None;
    }
    Some(kids[start..].to_vec())
}

/// 证书 subjectPKInfo 的 BIT STRING 内容（去掉"未用位数"字节）：
/// RSA 时是 SEQUENCE{modulus, exponent} 的 DER；EC 时是未压缩点字节。
/// 与私钥侧提出的公钥字节是同一把尺，配对校验直接比字节。
pub fn cert_public_key_octets(cert_der: &[u8]) -> Option<Vec<u8>> {
    let kids = after_version(cert_der)?;
    let pki = kids.get(5)?;
    if pki.0 != SEQUENCE {
        return None;
    }
    let inner = children(pki.1);
    let bit = inner.last()?;
    if bit.0 != BIT_STRING {
        return None;
    }
    let (_, octets) = bit.1.split_first()?;
    Some(octets.to_vec())
}

/// validity 的 notAfter 原始 ASCII（UTCTime 是 YYMMDDHHMMSSZ，GeneralizedTime 是
/// YYYYMMDDHHMMSSZ）。只用到"与今天比日期"的精度，解析推迟到调用方。
pub fn cert_not_after(cert_der: &[u8]) -> Option<&[u8]> {
    let kids = after_version(cert_der)?;
    let validity = kids.get(3)?;
    if validity.0 != SEQUENCE {
        return None;
    }
    let t = children(validity.1).pop()?;
    if t.0 != UTC_TIME && t.0 != GENERALIZED_TIME {
        return None;
    }
    Some(t.1)
}

const UTC_TIME: u8 = 0x17;
const GENERALIZED_TIME: u8 = 0x18;
const CONTEXT_CONSTRUCTED_3: u8 = 0xa3;
const BOOLEAN: u8 = 0x01;
/// basicConstraints 的 OID 内容：2.5.29.19。
const OID_BASIC_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x13];
/// CN 的 OID 内容：2.5.4.3。
const OID_COMMON_NAME: &[u8] = &[0x55, 0x04, 0x03];

/// 是否存在 basicConstraints 且 CA=TRUE。
///
/// 刻意做结构遍历而不是在整张证书里搜字节模式：OID 字节可能出现在别的扩展值里，
/// 搜出一个"看着像"的组把非 CA 的证书放行，等于静默接受一张签不出有效链的 CA ——
/// 那会在客户端握手期才炸，正是响亮失败要防的形态。
pub fn cert_is_ca(cert_der: &[u8]) -> bool {
    let Some(kids) = after_version(cert_der) else {
        return false;
    };
    let Some(exts) = kids
        .iter()
        .find(|(tag, _, _)| *tag == CONTEXT_CONSTRUCTED_3)
    else {
        return false;
    };
    // extensions [3] 里包着一层 SEQUENCE，扩展条目在 SEQUENCE 内 —— 少一层
    // 遍历就会把"整串扩展"当"一个扩展"，真实证书全部误判为非 CA。
    let Some((_, ext_seq, _)) = children(exts.1)
        .into_iter()
        .find(|(tag, _, _)| *tag == SEQUENCE)
    else {
        return false;
    };
    for (_, body, _) in children(ext_seq) {
        let parts = children(body);
        let Some((oid_tag, oid, _)) = parts.first() else {
            continue;
        };
        if *oid_tag != OBJECT_ID || *oid != OID_BASIC_CONSTRAINTS {
            continue;
        }
        let Some((_, value, _)) = parts.iter().find(|(tag, _, _)| *tag == OCTET_STRING) else {
            continue;
        };
        let Some((inner_tag, inner, _)) = read_tlv(value) else {
            continue;
        };
        if inner_tag != SEQUENCE {
            continue;
        }
        if children(inner)
            .first()
            .is_some_and(|(tag, v, _)| *tag == BOOLEAN && v == &[0xff])
        {
            return true;
        }
    }
    false
}

/// Subject 的 CN（多个 RDN 时取第一个 2.5.4.3 的值；测试与诊断用）。
pub fn cert_subject_cn(cert_der: &[u8]) -> Option<String> {
    let kids = after_version(cert_der)?;
    let subject = kids.get(4)?;
    if subject.0 != SEQUENCE {
        return None;
    }
    for (_, rdn, _) in children(subject.1) {
        for (_, atv, _) in children(rdn) {
            let parts = children(atv);
            let [oid, value] = parts.as_slice() else {
                continue;
            };
            if oid.1 == OID_COMMON_NAME {
                return Some(String::from_utf8_lossy(value.1).into_owned());
            }
        }
    }
    None
}

/// 私钥容器（PKCS#1 / PKCS#8 / SEC1 都认）→ 与证书可比对的公钥字节。
pub fn public_key_octets_from_private(key: &PrivateKeyDerish) -> Option<Vec<u8>> {
    match key {
        PrivateKeyDerish::Pkcs1(bytes) => rsa_public_from_pkcs1(bytes),
        PrivateKeyDerish::Pkcs8(bytes) => {
            let (alg, inner) = pkcs8_split(bytes)?;
            if is_rsa_alg(alg) {
                rsa_public_from_pkcs1(inner)
            } else {
                ec_public_from_sec1(inner)
            }
        }
        PrivateKeyDerish::Sec1(bytes) => ec_public_from_sec1(bytes),
    }
}

/// 上述函数的输入形态：刻意不绑 rustls 的类型（其私钥包装的字段的可见性随版本
/// 变化，自有容器把风险关在编译期）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivateKeyDerish {
    Pkcs1(Vec<u8>),
    Pkcs8(Vec<u8>),
    Sec1(Vec<u8>),
}

impl PrivateKeyDerish {
    pub fn bytes(&self) -> &[u8] {
        match self {
            Self::Pkcs1(b) | Self::Pkcs8(b) | Self::Sec1(b) => b,
        }
    }
}

// ---- X.509 证书只读摘要（设置页「证书信息」的数据源）----

/// 证书的只读展示字段。全部来自固定形状的结构遍历；私钥永不参与。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CertInfo {
    /// 人读版本号（DER 里的 [0] INTEGER 是 0 基，展示时 +1）。
    pub version: u8,
    /// 序列号：大写十六进制（字节序，无分隔符）。
    pub serial_hex: String,
    /// 有效期（UTCTime/GeneralizedTime 归一成 YYYY-MM-DD HH:MM:SSZ 形态）。
    pub not_before: String,
    pub not_after: String,
    /// 签名算法的人读名；不认识的 OID 落到点分十进制。
    pub sig_alg: String,
    /// 公钥算法名（RSA / EC / …）。
    pub pubkey_alg: String,
    /// EC 公钥的曲线名（RSA 时为 None）。
    pub pubkey_curve: Option<String>,
    /// RSA 公钥的模长位数（EC 时为 None）。
    pub pubkey_bits: Option<u32>,
    /// 颁发者 DN 字段（工作台展示的是自签 CA，与主体一致）。
    pub country: Option<String>,
    pub organization: Option<String>,
    pub common_name: Option<String>,
    /// subjectAltName 里的 DNS 名与 IP（按证书顺序）。
    pub san: Vec<String>,
    pub is_ca: bool,
    /// 整张证书 DER 的 SHA-256 十六进制指纹。
    pub fingerprint: String,
}

/// OID 内容字节 → 人读名。只列本仓实际会碰到的（rcgen 签发 + mitmproxy 兼容加载），
/// 其余 OID 一律点分十进制 —— 如实显示好过编造名字。
fn oid_name(oid: &[u8]) -> String {
    match oid {
        [0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02] => "ECDSA with SHA-256".into(),
        [0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03] => "ECDSA with SHA-384".into(),
        [0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x04] => "ECDSA with SHA-512".into(),
        [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b] => "SHA-256 with RSA".into(),
        [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0c] => "SHA-384 with RSA".into(),
        [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0d] => "SHA-512 with RSA".into(),
        [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x05] => "SHA-1 with RSA".into(),
        [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01] => "RSA".into(),
        [0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01] => "EC".into(),
        _ => oid_dotted(oid),
    }
}

/// OID 内容字节 → 点分十进制（base-128 解码；首字节把前两个子标识符合编）。
fn oid_dotted(oid: &[u8]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut acc = 0u64;
    for (index, byte) in oid.iter().enumerate() {
        acc = (acc << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            if index == 0 {
                let (first, second) = if acc < 80 {
                    (acc / 40, acc % 40)
                } else {
                    (2, acc - 80)
                };
                parts.push(first.to_string());
                parts.push(second.to_string());
            } else {
                parts.push(acc.to_string());
            }
            acc = 0;
        }
    }
    parts.join(".")
}

/// Name（RDN 序列）里找指定 OID 的第一个字符串值。
fn rdn_value(name: &Tlv, oid: &[u8]) -> Option<String> {
    if name.0 != SEQUENCE {
        return None;
    }
    for (_, rdn, _) in children(name.1) {
        for (_, atv, _) in children(rdn) {
            let parts = children(atv);
            let [entry_oid, value] = parts.as_slice() else {
                continue;
            };
            if entry_oid.0 != OBJECT_ID || entry_oid.1 != oid {
                continue;
            }
            // 字符串形态随签发方变化（Printable / UTF8 / IA5）；展示统一按文本读。
            if matches!(value.0, UTF8_STRING | PRINTABLE_STRING | IA5_STRING) {
                return Some(String::from_utf8_lossy(value.1).into_owned());
            }
        }
    }
    None
}

const OID_COUNTRY: &[u8] = &[0x55, 0x04, 0x06];
const OID_ORGANIZATION: &[u8] = &[0x55, 0x04, 0x0a];
const OID_SUBJECT_ALT_NAME: &[u8] = &[0x55, 0x1d, 0x11];

/// UTCTime（YYMMDDHHMMSSZ）与 GeneralizedTime（YYYYMMDDHHMMSS[.f]Z）归一成
/// YYYY-MM-DD HH:MM:SSZ。UTCTime 的两位年份按 X.680 界定：00-49 = 20xx。
fn time_text(t: &Tlv) -> Option<String> {
    if t.0 != UTC_TIME && t.0 != GENERALIZED_TIME {
        return None;
    }
    let ascii = String::from_utf8_lossy(t.1);
    let digits: String = ascii.chars().filter(|c| c.is_ascii_digit()).collect();
    let (year, rest) = if t.0 == GENERALIZED_TIME {
        (digits.get(0..4)?.to_string(), digits.get(4..)?.to_string())
    } else {
        let yy = digits.get(0..2)?.parse::<u32>().ok()?;
        let century = if yy >= 50 { "19" } else { "20" };
        (format!("{century}{yy}"), digits.get(2..)?.to_string())
    };
    if rest.len() < 6 {
        return None;
    }
    Some(format!(
        "{year}-{}-{} {}:{}:{}Z",
        &rest[0..2],
        &rest[2..4],
        &rest[4..6],
        rest.get(6..8).unwrap_or("00"),
        rest.get(8..10).unwrap_or("00"),
    ))
}

/// subjectPKInfo 的算法标识 → (算法名, 曲线名, RSA 模长位数)。
fn pki_alg(pki: &Tlv) -> (String, Option<String>, Option<u32>) {
    let Some(alg) = children(pki.1).first().cloned() else {
        return ("未知".into(), None, None);
    };
    let parts = children(alg.1);
    let Some((tag, oid, _)) = parts.first() else {
        return ("未知".into(), None, None);
    };
    if *tag != OBJECT_ID {
        return ("未知".into(), None, None);
    }
    let name = oid_name(oid);
    if name == "RSA" {
        // RSA：模长在 BIT STRING 的 SEQUENCE{INTEGER modulus, INTEGER e} 里。
        let bits = children(pki.1)
            .last()
            .and_then(|bit| bit.1.split_first().map(|(_, octets)| octets))
            .and_then(|octets| read_tlv(octets))
            .and_then(|(inner_tag, inner, _)| (inner_tag == SEQUENCE).then_some(inner))
            .and_then(|seq| children(seq).first().cloned())
            .and_then(|modulus| rsa_modulus_bits(modulus.1));
        return (name, None, bits);
    }
    if name == "EC" {
        // EC：曲线 OID 在 AlgorithmIdentifier 的 parameters 里。
        let curve = parts.get(1).map(|param| match param.1 {
            [0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07] => "P-256".to_string(),
            [0x2b, 0x81, 0x04, 0x00, 0x22] => "P-384".to_string(),
            [0x2b, 0x81, 0x04, 0x00, 0x23] => "P-521".to_string(),
            other => oid_dotted(other),
        });
        return (name, curve, None);
    }
    (name, None, None)
}

/// RSA 模长位数：跳过符号字节 0x00 后，位数 = 字节数 × 8 − 首字节前导零。
fn rsa_modulus_bits(modulus: &[u8]) -> Option<u32> {
    let mut m = modulus;
    while m.len() > 1 && m[0] == 0 {
        m = &m[1..];
    }
    let lz = m.first()?.leading_zeros();
    Some(m.len() as u32 * 8 - lz)
}

/// subjectAltName 扩展 → ["localhost", "127.0.0.1", …]。没有该扩展即空表。
fn cert_san(cert_der: &[u8]) -> Vec<String> {
    let Some(names) = extension_value(cert_der, OID_SUBJECT_ALT_NAME) else {
        return Vec::new();
    };
    let Some((seq_tag, seq, _)) = read_tlv(&names) else {
        return Vec::new();
    };
    if seq_tag != SEQUENCE {
        return Vec::new();
    }
    children(seq)
        .into_iter()
        .filter_map(|(tag, value, _)| match tag {
            DNS_NAME => Some(String::from_utf8_lossy(value).into_owned()),
            IP_ADDRESS => Some(ip_text(value)),
            _ => None,
        })
        .collect()
}

fn ip_text(bytes: &[u8]) -> String {
    match bytes.len() {
        4 => bytes
            .iter()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join("."),
        16 => bytes
            .chunks(2)
            .map(|pair| format!("{:02x}{:02x}", pair[0], pair[1]))
            .collect::<Vec<_>>()
            .join(":"),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// 找到指定 OID 扩展的 extnValue（OCTET STRING 内容）。结构与 cert_is_ca 同款：
/// [3] → SEQUENCE → 扩展条目（OID + optional BOOLEAN critical + OCTET STRING）。
fn extension_value(cert_der: &[u8], oid: &[u8]) -> Option<Vec<u8>> {
    let kids = after_version(cert_der)?;
    let exts = kids
        .iter()
        .find(|(tag, _, _)| *tag == CONTEXT_CONSTRUCTED_3)?;
    let (_, ext_seq, _) = children(exts.1)
        .into_iter()
        .find(|(tag, _, _)| *tag == SEQUENCE)?;
    for (_, body, _) in children(ext_seq) {
        let parts = children(body);
        let Some((oid_tag, entry_oid, _)) = parts.first() else {
            continue;
        };
        if *oid_tag != OBJECT_ID || *entry_oid != oid {
            continue;
        }
        if let Some((_, value, _)) = parts.iter().find(|(tag, _, _)| *tag == OCTET_STRING) {
            return Some(value.to_vec());
        }
    }
    None
}

/// 证书只读摘要：形状不对即 None（调用方决定如何降级展示）。
pub fn cert_info(cert_der: &[u8]) -> Option<CertInfo> {
    let (tag, cert_seq, rest) = read_tlv(cert_der)?;
    if tag != SEQUENCE || !rest.is_empty() {
        return None;
    }
    let outer = children(cert_seq);
    let (tbs_tag, tbs, _) = read_tlv(outer.first()?.2)?;
    if tbs_tag != SEQUENCE {
        return None;
    }
    let kids = children(tbs);
    let has_version = kids.first()?.0 == CONTEXT_CONSTRUCTED_0;
    let version = if has_version {
        let (inner_tag, inner, _) = read_tlv(kids[0].1)?;
        (inner_tag == INTEGER).then_some(inner)?;
        inner.last()?.wrapping_add(1)
    } else {
        1
    };
    let fixed = kids.get(usize::from(has_version)..)?;
    if fixed.len() < 6 {
        return None;
    }
    let serial = fixed.first()?;
    if serial.0 != INTEGER {
        return None;
    }
    let serial_hex: String = serial.1.iter().map(|b| format!("{b:02X}")).collect();
    let sig_alg = alg_name_of(fixed.get(1)?);
    let issuer = fixed.get(2)?;
    let country = rdn_value(issuer, OID_COUNTRY);
    let organization = rdn_value(issuer, OID_ORGANIZATION);
    let common_name = rdn_value(issuer, OID_COMMON_NAME);
    let validity = fixed.get(3)?;
    if validity.0 != SEQUENCE {
        return None;
    }
    let times = children(validity.1);
    let not_before = time_text(times.first()?)?;
    let not_after = time_text(times.get(1)?)?;
    let pki = fixed.get(5)?;
    if pki.0 != SEQUENCE {
        return None;
    }
    let (pubkey_alg, pubkey_curve, pubkey_bits) = pki_alg(pki);
    Some(CertInfo {
        version,
        serial_hex,
        not_before,
        not_after,
        sig_alg,
        pubkey_alg,
        pubkey_curve,
        pubkey_bits,
        country,
        organization,
        common_name,
        san: cert_san(cert_der),
        is_ca: cert_is_ca(cert_der),
        fingerprint: envboard_core_api::sha256::hex(cert_der),
    })
}

/// signature/algorithm 这类 AlgorithmIdentifier SEQUENCE 的 OID → 人读名。
fn alg_name_of(alg: &Tlv) -> String {
    if alg.0 != SEQUENCE {
        return "未知".into();
    }
    match children(alg.1).first() {
        Some((tag, oid, _)) if *tag == OBJECT_ID => oid_name(oid),
        _ => "未知".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// version=0, modulus=0x00C1, exponent=0x010001 的手作 PKCS#1。
    fn sample_pkcs1() -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&tlv(INTEGER, &[0]));
        body.extend_from_slice(&tlv(INTEGER, &[0x00, 0xC1]));
        body.extend_from_slice(&tlv(INTEGER, &[0x01, 0x00, 0x01]));
        body.extend_from_slice(&tlv(INTEGER, &[0x02])); // d，不该被读
        tlv(SEQUENCE, &body)
    }

    #[test]
    fn tlv_round_trips_multibyte_lengths() {
        let big = vec![7u8; 300];
        let encoded = tlv(OCTET_STRING, &big);
        let (tag, value, rest) = read_tlv(&encoded).unwrap();
        assert_eq!(tag, OCTET_STRING);
        assert_eq!(value, big.as_slice());
        assert!(rest.is_empty());
    }

    #[test]
    fn rsa_public_is_first_two_integers() {
        let pkcs1 = sample_pkcs1();
        let public = rsa_public_from_pkcs1(&pkcs1).unwrap();
        let (_, seq, rest) = read_tlv(&public).unwrap();
        assert!(rest.is_empty());
        let kids = children(seq);
        assert_eq!(kids.len(), 2);
        assert_eq!(kids[0].1, &[0x00, 0xC1]);
        assert_eq!(kids[1].1, &[0x01, 0x00, 0x01]);
    }

    #[test]
    fn pkcs1_wrapped_into_pkcs8_splits_back() {
        let pkcs1 = sample_pkcs1();
        let pkcs8 = pkcs1_to_pkcs8_rsa(&pkcs1);
        let (alg, inner) = pkcs8_split(&pkcs8).unwrap();
        assert!(is_rsa_alg(alg));
        assert_eq!(inner, pkcs1.as_slice());
    }

    #[test]
    fn malformed_input_returns_none_not_panic() {
        assert!(rsa_public_from_pkcs1(&[0x30, 0x00]).is_none());
        assert!(ec_public_from_sec1(&[]).is_none());
        assert!(pkcs8_split(&[0x04, 0x01, 0x00]).is_none());
        assert!(cert_public_key_octets(&[0x30, 0x00]).is_none());
        assert!(!cert_is_ca(&[0x04, 0x01, 0x00]));
        assert_eq!(cert_subject_cn(&[0x30, 0x00]), None);
    }

    #[test]
    fn private_key_containers_route_to_the_right_extractor() {
        let pkcs1 = sample_pkcs1();
        let via_pkcs1 =
            public_key_octets_from_private(&PrivateKeyDerish::Pkcs1(pkcs1.clone())).unwrap();
        let pkcs8 = pkcs1_to_pkcs8_rsa(&pkcs1);
        let via_pkcs8 = public_key_octets_from_private(&PrivateKeyDerish::Pkcs8(pkcs8)).unwrap();
        assert_eq!(via_pkcs1, via_pkcs8);
    }
}

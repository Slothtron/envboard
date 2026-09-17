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
#[allow(dead_code)]
pub const CONTEXT_CONSTRUCTED_0: u8 = 0xa0;
pub const CONTEXT_CONSTRUCTED_1: u8 = 0xa1;

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

//! 代理访问鉴权（407 门）与它需要的最小 Base64 编解码。
//!
//! 凭据只存在于编译后的快照与内存比对里 —— 不再有 argv、不再有状态文件。
//! Base64 自带实现（标准字母表，解码容忍缺 padding）：为编解码引第二个 crate
//! 不符合依赖纪律，这里不到 80 行且有 RFC 4648 官方向量钉住。编码器服务于
//! 出向的 `Proxy-Authorization: Basic`（二级代理鉴权），解码器服务于进向 407 门。

/// \`Proxy-Authorization: Basic <b64>\` → (user, password)。不合规即 None（不 panic）。
pub fn parse_basic(value: &str) -> Option<(String, Vec<u8>)> {
    let (scheme, encoded) = value.split_once(char::is_whitespace)?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = base64_decode(encoded.trim())?;
    let text = String::from_utf8(decoded).ok()?;
    let (user, password) = text.split_once(':')?;
    Some((user.to_string(), password.as_bytes().to_vec()))
}

/// 出向 Basic 凭据值：`base64(user:password)`（发给二级代理的
/// `Proxy-Authorization: Basic <value>`）。
pub fn basic_credentials(user: &str, password: &str) -> String {
    base64_encode(format!("{user}:{password}").as_bytes())
}

/// 标准字母表 Base64 编码（含 padding，RFC 4648）。
pub fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        // 前 8 位恒 0：把 3 字节左对齐成 4 字节大端，再按 6 位切片。
        let mut bytes = [0u8; 4];
        bytes[1] = chunk[0];
        if let Some(byte) = chunk.get(1) {
            bytes[2] = *byte;
        }
        if let Some(byte) = chunk.get(2) {
            bytes[3] = *byte;
        }
        let word = u32::from_be_bytes(bytes);
        out.push(TABLE[(word >> 18 & 0x3f) as usize] as char);
        out.push(TABLE[(word >> 12 & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(word >> 6 & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(word & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// 标准字母表 Base64 解码；遇非法字符即 None，容忍缺 padding（padding 后停止）。
pub fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let input = input.trim_end();
    let mut accumulator: u32 = 0;
    let mut bits = 0u32;
    let mut out = Vec::with_capacity(input.len() / 4 * 3 + 3);
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => u32::from(byte - b'A'),
            b'a'..=b'z' => u32::from(byte - b'a') + 26,
            b'0'..=b'9' => u32::from(byte - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        accumulator = (accumulator << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
        }
    }
    Some(out)
}

/// 407 挑战（无正文；连接由调用方决定去留）。realm 固定 envboard：
/// 这是调试代理的自证身份，不是可配置项。
pub fn challenge() -> &'static [u8] {
    b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"envboard\"\r\nContent-Length: 0\r\n\r\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc4648_vectors() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("Zm9vYmFy").unwrap(), b"foobar");
        assert_eq!(base64_decode("Zg==").unwrap(), b"f");
        assert_eq!(base64_decode("Zm8=").unwrap(), b"fo");
        assert_eq!(base64_decode("Zm9v").unwrap(), b"foo");
        // 缺 padding 的宽容形态
        assert_eq!(base64_decode("aGVsbG8").unwrap(), b"hello");
        assert!(base64_decode("!!!!").is_none());
    }

    #[test]
    fn encode_round_trips_with_the_decoder_and_matches_rfc4648() {
        // RFC 4648 §10 的测试向量
        for (raw, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(raw.as_bytes()), encoded, "raw={raw:?}");
            assert_eq!(base64_decode(encoded).unwrap(), raw.as_bytes());
        }
        // 与出向凭据拼装互验：解码回 user:password
        let value = basic_credentials("alice", "s3cret");
        assert_eq!(value, "YWxpY2U6czNjcmV0");
        assert_eq!(base64_decode(&value).unwrap(), b"alice:s3cret".as_slice());
    }

    #[test]
    fn parses_basic_and_only_basic() {
        let encoded = "YWxpY2U6czNjcmV0"; // alice:s3cret
        let (user, pass) = parse_basic(&format!("Basic {encoded}")).unwrap();
        assert_eq!(user, "alice");
        assert_eq!(pass, b"s3cret".as_slice());
        assert!(parse_basic(&format!("Bearer {encoded}")).is_none());
        assert!(parse_basic("Basic").is_none());
        // scheme 大小写不敏感（RFC 9110：auth-scheme 比较 case-insensitive）
        assert!(parse_basic(&format!("basic {encoded}")).is_some());
        // 密码里的冒号只按第一个切分
        let tricky = base64_line("u:a:b");
        let (user, pass) = parse_basic(&format!("Basic {tricky}")).unwrap();
        assert_eq!(user, "u");
        assert_eq!(pass, b"a:b".as_slice());
    }

    fn base64_line(text: &str) -> String {
        // 测试辅助：手写编码器，与解码器互验（不是生产路径）。
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        let mut chunk = 0u32;
        let mut bits = 0u32;
        for byte in text.bytes() {
            chunk = (chunk << 8) | u32::from(byte);
            bits += 8;
            while bits >= 6 {
                bits -= 6;
                out.push(TABLE[((chunk >> bits) & 0x3f) as usize] as char);
            }
        }
        if bits > 0 {
            out.push(TABLE[((chunk << (6 - bits)) & 0x3f) as usize] as char);
        }
        while !out.len().is_multiple_of(4) {
            out.push('=');
        }
        out
    }
}

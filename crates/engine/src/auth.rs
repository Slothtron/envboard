//! 代理访问鉴权（407 门）与它需要的最小 Base64 解码。
//!
//! 凭据只存在于编译后的快照与内存比对里 —— 不再有 argv、不再有状态文件。
//! Base64 自带实现（标准字母表、容忍缺 padding）：为一个解码引第二个 crate
//! 不符合依赖纪律，这里不到 50 行且有 RFC 4648 官方向量钉住。

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

//! HTTP/1.x 的最小转发模型：**默认整体缓冲**（错误契约的 body 语义从这里来）。
//!
//! 三条刻意选择，各有理由：
//!
//! * 请求与响应体都进内存（上限 \`MAX_BODY_BYTES\`），转发时统一改写成
//!   Content-Length —— 改写类插件需要完整 body，流式会打乱事件序；上限即
//!   防护，超限报错而不是悄悄流式。
//! * 上游连接一问一答（本实现不向上游复用连接）：规则与名单可热改，复用连接
//!   会把"上一条请求的策略"带进下一条；单机调试代理里握手成本可接受。
//! * WebSocket/Upgrade 走**透传旁路**：101 之后没有 HTTP 语义可言，缓冲只会把
//!   连接憋死，于是两侧裸字节互抄直到断开。

use std::io;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use envboard_core_api::text::parse_ip_literal;

/// 请求/响应头部（含起始行）的字节上限；超过即 431/502。
pub const MAX_HEAD_BYTES: usize = 64 * 1024;
/// 缓冲体上限（M-P3 把它做成可配置的 max_buffered_body，先取 8 MiB）。
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Message {
    /// 起始行：请求 "METHOD target VERSION"，响应 "VERSION STATUS REASON"。
    pub first: String,
    /// header 名已小写；值保留原样。
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Message {
    pub fn method(&self) -> Option<&str> {
        self.first.split(' ').next()
    }
    pub fn uri(&self) -> Option<&str> {
        self.first.split(' ').nth(1)
    }
    pub fn status(&self) -> Option<u16> {
        self.first
            .split(' ')
            .nth(1)
            .and_then(|code| code.parse::<u16>().ok())
    }
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
    pub fn is_upgrade_request(&self) -> bool {
        self.header("upgrade").is_some()
            && self
                .header("connection")
                .is_some_and(|value| value.to_ascii_lowercase().contains("upgrade"))
    }
}

#[derive(Debug)]
pub enum HttpError {
    BadHead(String),
    HeadTooBig,
    BodyTooBig,
    BadBody(String),
    Io(String),
}

impl HttpError {
    pub fn status(&self) -> u16 {
        match self {
            HttpError::HeadTooBig => 431,
            HttpError::BodyTooBig => 413,
            HttpError::BadHead(_) | HttpError::BadBody(_) => 400,
            HttpError::Io(_) => 502,
        }
    }
    pub fn reason(&self) -> String {
        match self {
            HttpError::BadHead(detail) => format!("malformed HTTP head: {detail}"),
            HttpError::HeadTooBig => format!("HTTP head exceeds {MAX_HEAD_BYTES} bytes"),
            HttpError::BodyTooBig => format!("HTTP body exceeds {MAX_BODY_BYTES} bytes"),
            HttpError::BadBody(detail) => format!("malformed HTTP body: {detail}"),
            HttpError::Io(detail) => detail.clone(),
        }
    }
}

impl From<io::Error> for HttpError {
    fn from(error: io::Error) -> Self {
        HttpError::Io(error.to_string())
    }
}

/// 从头部字节（不含结尾空行）解析起始行与头部。
pub fn parse_head(raw: &[u8]) -> Result<Message, HttpError> {
    let mut lines = raw.split(|b| *b == b'\n');
    let first_line = lines.next().unwrap_or_default();
    let first = String::from_utf8_lossy(trim_crlf(first_line))
        .trim()
        .to_string();
    if first.is_empty() {
        return Err(HttpError::BadHead("empty request/status line".to_string()));
    }
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        let line = trim_crlf(line);
        if line.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(line).into_owned();
        if text.starts_with([' ', '\t']) {
            // RFC 7230 的 obs-fold（续行）：拼回上一条，不发明新条目。
            let Some((_, value)) = headers.last_mut() else {
                return Err(HttpError::BadHead("obs-fold before any header".to_string()));
            };
            value.push(' ');
            value.push_str(text.trim());
            continue;
        }
        let Some((name, value)) = text.split_once(':') else {
            return Err(HttpError::BadHead(format!(
                "header line without colon: {text:?}"
            )));
        };
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
    Ok(Message {
        first,
        headers,
        body: Vec::new(),
    })
}

/// 判断头字节是否已含终止边界；返回"去掉边界后的头部内容"。
fn strip_boundary(raw: &[u8]) -> Option<(&[u8], bool)> {
    let n = raw.len();
    if n >= 4 && raw.ends_with(b"\r\n\r\n") {
        Some((&raw[..n - 4], true))
    } else if n >= 2 && raw.ends_with(b"\n\n") {
        Some((&raw[..n - 2], true))
    } else {
        Some((raw, false))
    }
}

/// BufReader 路径的头部读取（内部请求、上游响应 —— HTTP 帧内多读无害）。
/// Ok(None) = 对端在起始行之前干净断开。
pub async fn read_head<R: AsyncReadExt + Unpin>(
    reader: &mut BufReader<R>,
) -> Result<Option<Message>, HttpError> {
    let mut raw: Vec<u8> = Vec::new();
    loop {
        let Some(()) = read_line(reader, &mut raw).await? else {
            if raw.is_empty() {
                return Ok(None);
            }
            return Err(HttpError::BadHead("head ended by EOF".to_string()));
        };
        if let Some((head, done)) = strip_boundary(&raw)
            && done
        {
            return Ok(Some(parse_head(head)?));
        }
    }
}

/// CONNECT 前的专用读法：逐字节读到终止边界，**绝不多吃一个字节** ——
/// 客户端发完 CONNECT 就有权立刻开始 TLS，多吞一条握手记录握手就废了。
/// 返回头部与"边界之后已经到达的字节"恒为空的保证：调用方可直接把 socket
/// 交给 TLS 层。
pub async fn read_head_exactly<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<Option<Message>, HttpError> {
    let mut raw: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let read = reader
            .read(&mut byte)
            .await
            .map_err(|e| HttpError::Io(e.to_string()))?;
        if read == 0 {
            if raw.is_empty() {
                return Ok(None);
            }
            return Err(HttpError::BadHead("head ended by EOF".to_string()));
        }
        raw.push(byte[0]);
        if raw.len() > MAX_HEAD_BYTES {
            return Err(HttpError::HeadTooBig);
        }
        if let Some((head, done)) = strip_boundary(&raw)
            && done
        {
            return Ok(Some(parse_head(head)?));
        }
    }
}

async fn read_line<R: AsyncReadExt + Unpin>(
    reader: &mut BufReader<R>,
    buf: &mut Vec<u8>,
) -> Result<Option<()>, HttpError> {
    loop {
        // fill_buf 的借用必须先落地为自有数据再 consume（同一 reader 不能双可变借用）。
        let peeked = reader
            .fill_buf()
            .await
            .map_err(|e| HttpError::Io(e.to_string()))?;
        if peeked.is_empty() {
            return Ok(None);
        }
        let newline = peeked.iter().position(|b| *b == b'\n');
        let take = newline.map_or(peeked.len(), |index| index + 1);
        let chunk = peeked[..take].to_vec();
        reader.consume(take);
        buf.extend_from_slice(&chunk);
        if buf.len() > MAX_HEAD_BYTES {
            return Err(HttpError::HeadTooBig);
        }
        if newline.is_some() {
            return Ok(Some(()));
        }
    }
}

fn trim_crlf(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && matches!(line[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    &line[..end]
}

#[derive(Debug, PartialEq, Eq)]
pub enum Framing {
    Empty,
    Length(u64),
    Chunked,
    UntilClose,
}

/// 请求体分帧判定。
pub fn request_framing(method: &str, message: &Message) -> Result<Framing, HttpError> {
    let cl = message.header("content-length");
    let te = message.header("transfer-encoding");
    if let (Some(cl), Some(te)) = (cl, te)
        && te.to_ascii_lowercase().contains("chunked")
        && !cl.trim().eq("0")
    {
        return Err(HttpError::BadHead(
            "both Content-Length and Transfer-Encoding: chunked".to_string(),
        ));
    }
    if te.is_some_and(|value| value.to_ascii_lowercase().contains("chunked")) {
        return Ok(Framing::Chunked);
    }
    if let Some(value) = cl {
        let length: u64 = value
            .trim()
            .parse()
            .map_err(|_| HttpError::BadHead(format!("bad Content-Length: {value:?}")))?;
        return Ok(if length == 0 {
            Framing::Empty
        } else {
            Framing::Length(length)
        });
    }
    if method == "CONNECT" {
        return Ok(Framing::Empty);
    }
    Ok(Framing::Empty)
}

/// 响应体分帧。
pub fn response_framing(request_method: &str, status: u16, message: &Message) -> Framing {
    if request_method == "HEAD" || (100..200).contains(&status) || matches!(status, 204 | 304) {
        return Framing::Empty;
    }
    if status == 101 || message.is_upgrade_request() {
        return Framing::Empty;
    }
    let te = message.header("transfer-encoding");
    if te.is_some_and(|value| value.to_ascii_lowercase().contains("chunked")) {
        return Framing::Chunked;
    }
    if let Some(value) = message.header("content-length") {
        let Ok(length) = value.trim().parse::<u64>() else {
            return Framing::UntilClose;
        };
        return if length == 0 {
            Framing::Empty
        } else {
            Framing::Length(length)
        };
    }
    Framing::UntilClose
}

/// 读体（含 chunked 解码）；reader 正指向体。
pub async fn read_body<R: AsyncReadExt + Unpin>(
    reader: &mut BufReader<R>,
    framing: Framing,
) -> Result<Vec<u8>, HttpError> {
    let mut body = Vec::new();
    match framing {
        Framing::Empty => {}
        Framing::Length(length) => {
            if length > MAX_BODY_BYTES as u64 {
                return Err(HttpError::BodyTooBig);
            }
            body.resize(length as usize, 0);
            reader.read_exact(&mut body).await.map_err(|e| {
                HttpError::BadBody(format!("connection ended inside the body: {e}"))
            })?;
        }
        Framing::Chunked => loop {
            let mut line = Vec::new();
            let Some(()) = read_line(reader, &mut line).await? else {
                return Err(HttpError::BadBody("chunk header missing".to_string()));
            };
            let text = String::from_utf8_lossy(trim_crlf(&line)).into_owned();
            let size_text = text
                .split(';')
                .next()
                .and_then(|value| value.split_whitespace().next())
                .unwrap_or("");
            let size = u64::from_str_radix(size_text, 16)
                .map_err(|_| HttpError::BadBody(format!("bad chunk size: {size_text:?}")))?;
            if size == 0 {
                // trailer：读到空行为止，内容丢弃（转发统一改写 Content-Length）。
                loop {
                    let mut line = Vec::new();
                    let Some(()) = read_line(reader, &mut line).await? else {
                        break;
                    };
                    if trim_crlf(&line).is_empty() {
                        break;
                    }
                }
                break;
            }
            if (body.len() as u64) + size > MAX_BODY_BYTES as u64 {
                return Err(HttpError::BodyTooBig);
            }
            let mut chunk = vec![0u8; size as usize];
            reader
                .read_exact(&mut chunk)
                .await
                .map_err(|e| HttpError::BadBody(format!("truncated chunk: {e}")))?;
            body.append(&mut chunk);
            let mut crlf = [0u8; 2];
            reader
                .read_exact(&mut crlf)
                .await
                .map_err(|e| HttpError::BadBody(format!("chunk not followed by CRLF: {e}")))?;
            if crlf != *b"\r\n" {
                return Err(HttpError::BadBody("chunk not followed by CRLF".to_string()));
            }
        },
        Framing::UntilClose => {
            let mut buf = [0u8; 64 * 1024];
            loop {
                let read = reader
                    .read(&mut buf)
                    .await
                    .map_err(|e| HttpError::Io(e.to_string()))?;
                if read == 0 {
                    break;
                }
                if (body.len() + read) > MAX_BODY_BYTES {
                    return Err(HttpError::BodyTooBig);
                }
                body.extend_from_slice(&buf[..read]);
            }
        }
    }
    Ok(body)
}

/// 渲染并写出（体一律 Content-Length；chunked 只可能来自对端，不来自我们）。
pub async fn write_message<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    first: &str,
    headers: &[(String, String)],
    body: &[u8],
    with_body_len: bool,
) -> Result<(), HttpError> {
    let mut head = format!("{first}\r\n");
    for (name, value) in headers {
        if name == "content-length" || name == "transfer-encoding" {
            continue;
        }
        head.push_str(&format!("{}: {value}\r\n", canonical_header_name(name)));
    }
    if with_body_len {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    writer.write_all(head.as_bytes()).await?;
    if !body.is_empty() {
        writer.write_all(body).await?;
    }
    writer.flush().await?;
    Ok(())
}

/// 把头名还原成常见大小写习惯（转发不改语义，只照顾人眼与旧服务器）。
fn canonical_header_name(lower: &str) -> String {
    const TABLE: &[(&str, &str)] = &[
        ("content-length", "Content-Length"),
        ("content-type", "Content-Type"),
        ("host", "Host"),
        ("user-agent", "User-Agent"),
        ("accept", "Accept"),
        ("accept-encoding", "Accept-Encoding"),
        ("connection", "Connection"),
        ("authorization", "Authorization"),
        ("cookie", "Cookie"),
    ];
    if let Some((_, name)) = TABLE.iter().find(|(key, _)| *key == lower) {
        return (*name).to_string();
    }
    let mut chars: Vec<char> = lower.chars().collect();
    if let Some(first) = chars.first_mut() {
        *first = first.to_ascii_uppercase();
    }
    String::from_iter(chars)
}

/// 转发请求时的头过滤。保留 upgrade 链（101 要透传），丢掉代理层头与分帧头。
pub fn forward_request_headers(message: &Message) -> Vec<(String, String)> {
    const DROP: &[&str] = &[
        "proxy-authorization",
        "proxy-connection",
        "te",
        "trailer",
        "transfer-encoding",
    ];
    let keep_connection = message.is_upgrade_request();
    let mut headers: Vec<(String, String)> = message
        .headers
        .iter()
        .filter(|(name, _)| {
            !DROP.contains(&name.as_str()) && (keep_connection || name != "connection")
        })
        .cloned()
        .collect();
    if !keep_connection {
        headers.retain(|(name, _)| name != "keep-alive");
    }
    headers
}

pub fn forward_response_headers(message: &Message) -> Vec<(String, String)> {
    const DROP: &[&str] = &[
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "te",
        "trailer",
        "transfer-encoding",
    ];
    let mut headers: Vec<(String, String)> = message
        .headers
        .iter()
        .filter(|(name, _)| !DROP.contains(&name.as_str()))
        .cloned()
        .collect();
    if message.status() != Some(101) {
        headers.retain(|(name, _)| name != "upgrade");
    }
    headers
}

/// "http://host[:port]/path" → (host, port(默认 80), origin-form 路径)。
/// 只认 http scheme：https 的绝对形式不接受（该走 CONNECT），与 v2 一致。
pub fn parse_absolute_http_uri(uri: &str) -> Option<(String, u16, String)> {
    let rest = uri.strip_prefix("http://")?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    let (host, port) = split_authority(authority, 80)?;
    Some((host, port, path.to_string()))
}

/// "host:port" / "[v6]:port" / "host"（取默认端口）。
pub fn split_authority(authority: &str, default_port: u16) -> Option<(String, u16)> {
    let authority = authority.trim_end_matches(':');
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        if host.is_empty() {
            return None;
        }
        let port = match tail {
            "" => default_port,
            _ => tail.strip_prefix(':')?.parse().ok()?,
        };
        return Some((host.to_string(), port));
    }
    if parse_ip_literal(authority).is_some() {
        return Some((authority.to_string(), default_port));
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().ok()?),
        None => (authority, default_port),
    };
    if host.is_empty() || port == 0 {
        return None;
    }
    Some((host.to_string(), port))
}

/// 101 之后的裸字节透传：先冲掉 BufReader 里已到达的字节，再双向互抄。
pub async fn tunnel<CL, UL>(
    client_reader: &mut BufReader<CL>,
    client_writer: &mut CL,
    upstream: &mut UL,
) -> io::Result<()>
where
    CL: AsyncReadExt + AsyncWriteExt + Unpin,
    UL: AsyncReadExt + AsyncWriteExt + Unpin,
{
    let pending = client_reader.buffer().to_vec();
    if !pending.is_empty() && upstream.write_all(&pending).await.is_err() {
        return Ok(());
    }
    let _ = tokio::io::copy_bidirectional(client_writer, upstream).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tokio::io::BufReader as TokioBufReader;

    fn msg(first: &str, headers: &[(&str, &str)]) -> Message {
        Message {
            first: first.to_string(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string().to_ascii_lowercase(), v.to_string()))
                .collect(),
            body: Vec::new(),
        }
    }

    #[tokio::test]
    async fn head_and_body_round_trip() {
        let raw =
            b"GET http://svc.test/a?x=1 HTTP/1.1\r\nHost: svc.test\r\nContent-Length: 2\r\n\r\nhi";
        let mut reader = TokioBufReader::new(Cursor::new(&raw[..]));
        let mut message = read_head(&mut reader).await.unwrap().unwrap();
        let framing = request_framing("GET", &message).unwrap();
        assert_eq!(framing, Framing::Length(2));
        message.body = read_body(&mut reader, framing).await.unwrap();
        assert_eq!(message.body, b"hi");
        assert_eq!(message.first, "GET http://svc.test/a?x=1 HTTP/1.1");
        assert_eq!(message.header("host"), Some("svc.test"));
    }

    #[tokio::test]
    async fn exact_reader_stops_at_the_boundary() {
        // 逐字节读法：读到边界即停，后面的字节一个都不许动。
        let mut raw = b"CONNECT a.test:443 HTTP/1.1\r\nHost: a.test\r\n\r\n".to_vec();
        raw.extend_from_slice(&[0x16, 0x03, 0x01]); // 紧随 CONNECT 的 TLS record 开头
        let cursor = Cursor::new(&raw[..]);
        tokio::pin!(cursor);
        let message = read_head_exactly(&mut cursor).await.unwrap().unwrap();
        assert_eq!(message.first, "CONNECT a.test:443 HTTP/1.1");
        let mut rest = Vec::new();
        cursor.read_to_end(&mut rest).await.unwrap();
        assert_eq!(rest, vec![0x16, 0x03, 0x01]);
    }

    #[tokio::test]
    async fn chunked_body_is_decoded_and_trailers_dropped() {
        let raw = b"POST /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\nX-Trail: 1\r\n\r\n";
        let mut reader = TokioBufReader::new(Cursor::new(&raw[..]));
        let message = read_head(&mut reader).await.unwrap().unwrap();
        let framing = request_framing("POST", &message).unwrap();
        assert_eq!(framing, Framing::Chunked);
        let body = read_body(&mut reader, framing).await.unwrap();
        assert_eq!(body, b"abcde");
    }

    #[tokio::test]
    async fn oversized_head_is_loud() {
        let mut raw = b"GET / HTTP/1.1\r\nX: ".to_vec();
        raw.extend_from_slice(&vec![b'a'; MAX_HEAD_BYTES]);
        let mut reader = TokioBufReader::new(Cursor::new(raw));
        let error = read_head(&mut reader).await.unwrap_err();
        assert_eq!(error.status(), 431);
    }

    #[test]
    fn absolute_uri_and_authority_forms() {
        assert_eq!(
            parse_absolute_http_uri("http://svc.test:8080/p?q=1"),
            Some(("svc.test".to_string(), 8080, "/p?q=1".to_string()))
        );
        assert_eq!(
            parse_absolute_http_uri("http://svc.test"),
            Some(("svc.test".to_string(), 80, "/".to_string()))
        );
        assert!(parse_absolute_http_uri("https://svc.test/x").is_none());
        assert_eq!(
            split_authority("[::1]:443", 80),
            Some(("::1".to_string(), 443))
        );
        assert_eq!(
            split_authority("127.0.0.1", 80),
            Some(("127.0.0.1".to_string(), 80))
        );
        assert_eq!(
            split_authority("svc.test:", 80),
            Some(("svc.test".to_string(), 80))
        );
        assert!(split_authority("svc.test:0", 80).is_none());
    }

    #[test]
    fn forwarding_filters_hop_scoped_headers() {
        let request = msg(
            "GET http://a.test/ HTTP/1.1",
            &[
                ("connection", "keep-alive"),
                ("proxy-authorization", "Basic aaa"),
                ("host", "a.test"),
            ],
        );
        let forwarded = forward_request_headers(&request);
        assert!(
            forwarded
                .iter()
                .all(|(name, _)| name != "proxy-authorization" && name != "connection")
        );
        let upgrade = msg(
            "GET / HTTP/1.1",
            &[
                ("connection", "Upgrade, HTTP2-Settings"),
                ("upgrade", "websocket"),
            ],
        );
        assert!(upgrade.is_upgrade_request());
        assert!(
            forward_request_headers(&upgrade)
                .iter()
                .any(|(name, _)| name == "connection")
        );
    }
}

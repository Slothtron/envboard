//! 抓包正文的展示决定：什么时候可以美化、美化长什么样、什么时候只给原文。
//!
//! 边界来自 ADR-8（EB-DESIGN-004 v1.4）：
//!
//! * **不做语法高亮**——着色要一层富文本原语，收益不抵成本；
//! * 美化只做「重排缩进」（JSON 2 空格、HTML/XML 逐标签换行缩进），不重写内容；
//! * **阈值 64 KiB**：超过就只给原文态，避免把万行文本压进布局；
//! * `omitted`（后端 256 KiB 硬上限）与 `base64`（二进制）没有美化态；
//! * 复制跟随当前视图：美化态复制美化文本，原文态复制原始文本。
//!
//! 全部纯函数，不碰窗口；页面只问「这一条该显示什么」。

use std::sync::Arc;

use envboard_protocol::RecordBody;

/// 美化的字节阈值：64 KiB（字节数按记录里的 `size`，不是字符数）。
pub const PRETTY_LIMIT: u64 = 64 * 1024;

/// 一次渲染的正文上限：32 KiB。超出的部分先不进控件，由「继续加载」再放一段。
///
/// 为什么不是一次全塞：一个几千行的只读多行框要一次性排版、量宽、建行索引，
/// 40–64 KiB 的整块会明显拖慢打开动作；分段把成本摊到用户真的往下看的时候。
pub const CHUNK: usize = 32 * 1024;

/// 取前 `limit` 字节，**落在字符边界上**（中文与 base64 混排也不能切出半个字）。
///
/// 不返回“是否截断”，调用方用 `text.len()` 与结果的 `len()` 比即可。
pub fn take_prefix(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// 正文的后端形状（`encode_body` 的三种 + 没有正文）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// 没有正文（记录里 body 为 None）。
    Absent,
    /// UTF-8 文本。
    Text,
    /// 二进制：内容是 base64 串。
    Binary,
    /// 超过后端上限，只留了字节数。
    Omitted,
}

/// 一条正文：形状 + 字节数 + 原始文本（`Binary` 时是 base64）。
///
/// `raw` 是 `Arc<str>` 而不是 `String`（S4）：详情面板一旦建好，`Body` 会被
/// 信号快照、页签子树、复制按钮各取一份，`String` 每份都是一次整段克隆；
/// `Arc<str>` 让之后所有持有都是 O(1) 计数。物化只发生一次
/// （`from_record` 从 `Record` 的 `String` 转过来，≤ 后端 256 KiB 上限）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Body {
    pub kind: Kind,
    pub size: u64,
    pub raw: Arc<str>,
}

impl Default for Body {
    fn default() -> Self {
        Body {
            kind: Kind::Absent,
            size: 0,
            raw: Arc::from(""),
        }
    }
}

impl Body {
    /// 从记录里的 typed 正文块读：`None` = 没有正文。
    pub fn from_record(body: Option<&RecordBody>) -> Self {
        let Some(body) = body else {
            return Body::default();
        };
        match body {
            RecordBody::Omitted { size, .. } => Body {
                kind: Kind::Omitted,
                size: *size,
                raw: Arc::from(""),
            },
            RecordBody::Present {
                encoding,
                size,
                content,
            } => Body {
                kind: if encoding == "base64" {
                    Kind::Binary
                } else {
                    Kind::Text
                },
                size: *size,
                raw: Arc::from(content.as_str()),
            },
        }
    }

    /// 这一条能不能切到「美化」态（等价于「美化是否真的成功」：不合法的 JSON
    /// 有 `application/json` 头也美不了，页面据此只留原文态）。
    pub fn can_pretty(&self, content_type: Option<&str>) -> bool {
        self.pretty(content_type).is_some()
    }

    /// 美化后的文本；不能美化返回 `None`。
    pub fn pretty(&self, content_type: Option<&str>) -> Option<String> {
        if self.kind != Kind::Text
            || self.size > PRETTY_LIMIT
            || self.raw.len() as u64 > PRETTY_LIMIT
        {
            return None;
        }
        match language_of(&self.raw, content_type)? {
            Lang::Json => pretty_json(&self.raw),
            Lang::Html | Lang::Xml => Some(indent_markup(&self.raw)),
        }
    }

    /// 不美化时的那句话（「二进制 · 12.3 KB」「正文超过 64 KiB，未记录」）。
    pub fn note(&self) -> Option<String> {
        match self.kind {
            Kind::Text | Kind::Absent => None,
            Kind::Binary => Some(format!(
                "二进制正文 · {} · 只有原文（base64）可复制",
                crate::model::capture::format_bytes(self.size)
            )),
            Kind::Omitted => Some(format!(
                "正文未记录 · {}（超过后端 256 KiB 上限）",
                crate::model::capture::format_bytes(self.size)
            )),
        }
    }
}

/// 能美化的三种语言（CSS/JS 不重排，不在内）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Json,
    Html,
    Xml,
}

/// 语言判定：content-type 优先，缺了看内容嗅探（与桌面版 `prepareBody` 同序）。
pub fn language_of(content: &str, content_type: Option<&str>) -> Option<Lang> {
    let ct = content_type.unwrap_or("").to_ascii_lowercase();
    let trimmed = content.trim_start();
    if ct.contains("json")
        || (ct.is_empty() && (trimmed.starts_with('{') || trimmed.starts_with('[')))
    {
        return Some(Lang::Json);
    }
    if ct.contains("html")
        || trimmed.to_ascii_lowercase().starts_with("<!doctype html")
        || trimmed.to_ascii_lowercase().starts_with("<html")
    {
        return Some(Lang::Html);
    }
    if ct.contains("xml") || trimmed.starts_with("<?xml") {
        return Some(Lang::Xml);
    }
    if trimmed.starts_with('<') {
        return Some(Lang::Html);
    }
    None
}

/// JSON 美化：解析成功才给缩进文本；不合法返回 `None`（页面落原文态）。
fn pretty_json(content: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(content).ok()?;
    serde_json::to_string_pretty(&value).ok()
}

// ---- HTML / XML 缩进 -------------------------------------------------------

enum Token<'a> {
    Comment(&'a str),
    Cdata(&'a str),
    Tag(&'a str),
    Text(&'a str),
}

/// 自闭合/空元素的标签名：不增缩进深度。
const VOID_TAGS: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// HTML/XML 缩进重排：每个标签独占一行、文本节点按当前深度缩进，
/// 闭合标签先降一层；注释/CDATA 整体占一行，不参与深度。
pub fn indent_markup(src: &str) -> String {
    let tokens = tokenize(src);
    let mut out = String::with_capacity(src.len() + src.len() / 2);
    let mut depth = 0usize;
    for token in tokens {
        match token {
            Token::Comment(text) | Token::Cdata(text) => {
                push_line(&mut out, depth, text);
            }
            Token::Text(text) => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    push_line(&mut out, depth, trimmed);
                }
            }
            Token::Tag(tag) => {
                if tag.starts_with("</") {
                    depth = depth.saturating_sub(1);
                    push_line(&mut out, depth, tag);
                } else {
                    push_line(&mut out, depth, tag);
                    if !is_self_closing(tag) {
                        depth += 1;
                    }
                }
            }
        }
    }
    out.trim_end().to_owned()
}

fn push_line(out: &mut String, depth: usize, text: &str) {
    if !out.is_empty() {
        out.push('\n');
    }
    for _ in 0..depth {
        out.push_str("  ");
    }
    out.push_str(text);
}

fn is_self_closing(tag: &str) -> bool {
    if tag.starts_with("<?") || tag.starts_with("<!") {
        return true;
    }
    if tag.ends_with("/>") {
        return true;
    }
    let name = tag
        .trim_start_matches('<')
        .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    VOID_TAGS.contains(&name.as_str())
}

/// 切成 注释 / CDATA / 标签 / 文本 四类；字符串里的 `>` 不当作标签结束。
fn tokenize(src: &str) -> Vec<Token<'_>> {
    let bytes = src.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'<' {
            let start = index;
            while index < bytes.len() && bytes[index] != b'<' {
                index += 1;
            }
            tokens.push(Token::Text(&src[start..index]));
            continue;
        }
        if src[index..].starts_with("<!--") {
            let end = src[index + 4..]
                .find("-->")
                .map(|offset| index + 4 + offset + 3)
                .unwrap_or(bytes.len());
            tokens.push(Token::Comment(&src[index..end]));
            index = end;
            continue;
        }
        if src[index..].starts_with("<![CDATA[") {
            let end = src[index + 9..]
                .find("]]>")
                .map(|offset| index + 9 + offset + 3)
                .unwrap_or(bytes.len());
            tokens.push(Token::Cdata(&src[index..end]));
            index = end;
            continue;
        }
        // 普通标签：扫描到不在引号里的 '>'
        let start = index;
        let mut quote: Option<u8> = None;
        while index < bytes.len() {
            let byte = bytes[index];
            match quote {
                Some(q) if byte == q => quote = None,
                Some(_) => {}
                None if byte == b'"' || byte == b'\'' => quote = Some(byte),
                None if byte == b'>' => {
                    index += 1;
                    break;
                }
                None => {}
            }
            index += 1;
        }
        tokens.push(Token::Tag(&src[start..index.min(bytes.len())]));
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    /// UTF-8 正文块（测试里最常用的一种）。
    fn utf8(size: u64, content: &str) -> RecordBody {
        RecordBody::Present {
            encoding: "utf8".to_owned(),
            size,
            content: content.to_owned(),
        }
    }

    /// 把一块正文读成展示态（`None` = 没有正文）。
    fn view(block: RecordBody) -> Body {
        Body::from_record(Some(&block))
    }

    #[test]
    fn json_pretty_uses_two_spaces_and_keeps_values() {
        let body = view(utf8(13, "{\"a\":1,\"b\":[2,3]}"));
        let pretty = body.pretty(Some("application/json")).unwrap();
        assert!(pretty.contains("\n  \"a\": 1,"), "两空格缩进：{pretty}");
        assert_eq!(body.raw.as_ref(), "{\"a\":1,\"b\":[2,3]}", "原文不动");
        // 复制跟随视图：美化态复制美化文本，原文态复制原文——都来自同一份输入。
        assert_ne!(pretty, body.raw.as_ref());
    }

    #[test]
    fn invalid_json_has_no_pretty_state() {
        let body = view(utf8(6, "{oops"));
        assert!(body.pretty(Some("application/json")).is_none());
        assert!(!body.can_pretty(Some("application/json")));
    }

    #[test]
    fn markup_indents_tags_and_keeps_text() {
        let html = "<div><p>hi</p><br><span>a</span></div>";
        let out = indent_markup(html);
        assert_eq!(
            out,
            "<div>\n  <p>\n    hi\n  </p>\n  <br>\n  <span>\n    a\n  </span>\n</div>"
        );
        let xml = "<?xml version=\"1.0\"?><root><a>1</a></root>";
        let out = indent_markup(xml);
        assert!(out.starts_with("<?xml version=\"1.0\"?>"), "{out}");
        assert!(out.contains("\n  <a>"));
    }

    #[test]
    fn binary_and_omitted_have_only_raw_or_a_note() {
        let binary = view(RecordBody::Present {
            encoding: "base64".to_owned(),
            size: 6,
            content: "AAECAw==".to_owned(),
        });
        assert_eq!(binary.kind, Kind::Binary);
        assert!(binary.pretty(None).is_none());
        assert!(binary.note().unwrap().contains("二进制"));

        let omitted = view(RecordBody::Omitted {
            omitted: true,
            size: 400_000,
        });
        assert_eq!(omitted.kind, Kind::Omitted);
        assert!(omitted.pretty(None).is_none());
        assert!(omitted.note().unwrap().contains("未记录"));

        let absent = Body::from_record(None);
        assert_eq!(absent.kind, Kind::Absent);
        assert!(absent.note().is_none());
    }

    #[test]
    fn bodies_over_64_kib_skip_pretty() {
        let big = utf8(
            PRETTY_LIMIT,
            &format!("{{\"pad\":\"{}\"}}", "x".repeat(PRETTY_LIMIT as usize)),
        );
        let body = view(big);
        assert!(
            body.pretty(Some("application/json")).is_none(),
            "超过 64 KiB 不美化"
        );
        // 边界内（64 KiB 整）仍可美化：阈值按 `size` 与内容长度的较大者算，这里是 size 超了
        assert!(
            view(utf8(1024, "{\"a\":1}"))
                .pretty(Some("application/json"))
                .is_some()
        );
    }

    #[test]
    fn plain_text_has_no_pretty_state() {
        let body = view(utf8(5, "hello"));
        assert!(body.pretty(Some("text/plain")).is_none(), "纯文本不重排");
    }

    #[test]
    fn prefix_split_stays_on_char_boundary() {
        // 中文按 3 字节：在“第 3 个字中间”切也不能切出半个字
        let text = "一二三四五";
        assert_eq!(take_prefix(text, 6), "一二");
        assert_eq!(take_prefix(text, 7), "一二");
        assert_eq!(take_prefix(text, 8), "一二");
        assert_eq!(take_prefix(text, 9), "一二三");
        // 短文本原样返回，不额外分配
        assert_eq!(take_prefix("abc", 32), "abc");
        // 全切到 0 时给空串而不是 panic
        assert_eq!(take_prefix("一", 1), "");
    }
}

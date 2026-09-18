//! JSONL 存取：首行 `{"version":1}`，随后每行一个信封。
//!
//! 读侧契约（fail-closed）：
//!
//! * 首行不是合法 header 或版本高于本实现 → 整份拒绝；
//! * 未知事件类型：未标 `ignorable: true` → 整份拒绝；标了 → 跳过该行；
//! * `seq` 不连续 → 拒绝（丢失的事件必须可见，不能当没发生）；
//! * 最后一行不是合法 JSON（进程崩溃撕裂的残片）→ 容忍丢弃；
//!   中间的坏行 → 拒绝。

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::envelope::Envelope;

/// 存储格式版本。仅当信封形状 / 首行 / 核心事件语义变化时 bump。
pub const FORMAT_VERSION: u64 = 1;

/// 首行 header。
pub fn header_line() -> String {
    format!(r#"{{"version":{FORMAT_VERSION}}}"#)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// 首行缺失或不是 `{"version":N}`。
    BadHeader(String),
    /// 版本高于本实现（新文件老读者）。
    UnsupportedVersion { found: u64, supported: u64 },
    /// 第 `line` 行（1 起，不含 header）是未知事件类型且未标 ignorable。
    UnknownType { line: usize, kind: String },
    /// 第 `line` 行解析失败（撕裂尾除外）。
    MalformedLine { line: usize, detail: String },
    /// seq 不连续（期望值 → 实际值）。
    SeqGap { expected: u64, found: u64 },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::BadHeader(line) => write!(f, "bad header line: {line}"),
            ParseError::UnsupportedVersion { found, supported } => {
                write!(f, "format version {found} > supported {supported}")
            }
            ParseError::UnknownType { line, kind } => {
                write!(
                    f,
                    "line {line}: unknown event type {kind:?} (not marked ignorable)"
                )
            }
            ParseError::MalformedLine { line, detail } => {
                write!(f, "line {line}: {detail}")
            }
            ParseError::SeqGap { expected, found } => {
                write!(f, "seq gap: expected {expected}, found {found}")
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// 解析成功的一条：信封 + 原始行号（1 起，不含 header；便于回读到文件）。
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedEvent<T> {
    pub envelope: Envelope<T>,
    /// 1 起的事件行号（header 是第 0 行）。
    pub line: usize,
}

/// 解析一份 JSONL 事件日志。撕裂尾容忍、未知类型 fail-closed、seq 连续性判定。
pub fn parse_log<T: DeserializeOwned>(text: &str) -> Result<Vec<ParsedEvent<T>>, ParseError> {
    let mut lines = text.lines();
    let header = lines
        .next()
        .ok_or_else(|| ParseError::BadHeader("log is empty (no version header)".to_string()))?;
    let version: u64 = parse_header(header)?;
    if version > FORMAT_VERSION {
        return Err(ParseError::UnsupportedVersion {
            found: version,
            supported: FORMAT_VERSION,
        });
    }

    let mut events = Vec::new();
    let mut expected_seq = 1u64;
    let mut line_no = 0usize;
    let raw_lines: Vec<&str> = lines.collect();
    let last_index = raw_lines.len().saturating_sub(1);
    for (index, raw) in raw_lines.iter().enumerate() {
        line_no += 1;
        if raw.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(raw) {
            Ok(value) => value,
            // 只有最后一行允许是崩溃撕裂的残片；中间的坏行必须可见。
            Err(error) if index == last_index => {
                let _ = error;
                break;
            }
            Err(error) => {
                return Err(ParseError::MalformedLine {
                    line: line_no,
                    detail: error.to_string(),
                });
            }
        };
        // 未知类型 fail-closed：先看 type 与 ignorable，再做正常反序列化。
        let kind = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let ignorable = value
            .get("ignorable")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let known = known_kind::<T>(kind);
        if !known && !ignorable {
            return Err(ParseError::UnknownType {
                line: line_no,
                kind: kind.to_string(),
            });
        }
        if !known {
            continue;
        }
        let envelope: Envelope<T> =
            serde_json::from_value(value).map_err(|error| ParseError::MalformedLine {
                line: line_no,
                detail: error.to_string(),
            })?;
        if envelope.seq != expected_seq {
            return Err(ParseError::SeqGap {
                expected: expected_seq,
                found: envelope.seq,
            });
        }
        expected_seq += 1;
        events.push(ParsedEvent {
            envelope,
            line: line_no,
        });
    }
    Ok(events)
}

/// 追加一行（写入方负责顺序与 seq 连续性；这里只做无损 JSON 防线）。
pub fn append_line<T: Serialize>(
    buffer: &mut String,
    envelope: &Envelope<T>,
) -> Result<(), String> {
    let line = serde_json::to_string(envelope)
        .map_err(|error| format!("event is not lossless JSON: {error}"))?;
    buffer.push_str(&line);
    buffer.push('\n');
    Ok(())
}

fn parse_header(header: &str) -> Result<u64, ParseError> {
    let value: serde_json::Value =
        serde_json::from_str(header).map_err(|error| ParseError::BadHeader(error.to_string()))?;
    let version = value
        .get("version")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| ParseError::BadHeader(header.to_string()))?;
    Ok(version)
}

/// 类型层面的已知类型判定：`ControlEvent` / `DataEvent` 的 kind 集合。
/// 这里用反序列化探针：已知类型能通过 serde 的 tag 匹配（值为空对象时
/// 缺字段也会报"missing field"，但那已经是"类型认识"的范畴）。
fn known_kind<T: DeserializeOwned>(kind: &str) -> bool {
    if kind.is_empty() {
        return false;
    }
    let probe = serde_json::json!({ "type": kind, "data": {} });
    match serde_json::from_value::<T>(probe) {
        Ok(_) => true,
        Err(error) => !error.to_string().contains("unknown variant"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::ControlEvent;
    use crate::data::DataEvent;

    fn sample(seq: u64) -> Envelope<ControlEvent> {
        Envelope::new(
            seq,
            1_700_000_000_000 + seq,
            ControlEvent::EnvironmentDeleted { name: "dev".into() },
        )
    }

    #[test]
    fn header_and_roundtrip() {
        let mut buffer = header_line();
        buffer.push('\n');
        append_line(&mut buffer, &sample(1)).unwrap();
        append_line(&mut buffer, &sample(2)).unwrap();
        let events = parse_log::<ControlEvent>(&buffer).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].envelope.seq, 1);
        assert_eq!(events[1].line, 2);
    }

    #[test]
    fn unknown_type_is_rejected_unless_ignorable() {
        let mut buffer = header_line();
        buffer.push('\n');
        buffer.push_str(r#"{"seq":1,"time":1,"type":"future/thing","data":{"x":1}}"#);
        let error = parse_log::<ControlEvent>(&buffer).unwrap_err();
        assert!(matches!(error, ParseError::UnknownType { kind, .. } if kind == "future/thing"));

        let mut buffer = header_line();
        buffer.push('\n');
        buffer.push_str(
            r#"{"seq":1,"time":1,"type":"future/thing","data":{"x":1},"ignorable":true}"#,
        );
        // 未登记的新类型即使标了 ignorable 也会被跳过（serde 不认识 tag），
        // 这里记录的是"跳过"而非"拒绝"。
        assert_eq!(parse_log::<ControlEvent>(&buffer).unwrap().len(), 0);
    }

    #[test]
    fn seq_gap_is_visible() {
        let mut buffer = header_line();
        buffer.push('\n');
        append_line(&mut buffer, &sample(1)).unwrap();
        append_line(&mut buffer, &sample(3)).unwrap();
        let error = parse_log::<ControlEvent>(&buffer).unwrap_err();
        assert!(matches!(
            error,
            ParseError::SeqGap {
                expected: 2,
                found: 3
            }
        ));
    }

    #[test]
    fn torn_tail_is_tolerated_but_broken_middle_is_not() {
        let mut buffer = header_line();
        buffer.push('\n');
        append_line(&mut buffer, &sample(1)).unwrap();
        buffer.push_str(r#"{"seq":2,"time":1,"type":"environment/de"#);
        assert_eq!(parse_log::<ControlEvent>(&buffer).unwrap().len(), 1);

        let mut buffer = header_line();
        buffer.push('\n');
        buffer.push_str("not json at all\n");
        append_line(&mut buffer, &sample(1)).unwrap();
        assert!(matches!(
            parse_log::<ControlEvent>(&buffer),
            Err(ParseError::MalformedLine { line: 1, .. })
        ));
    }

    #[test]
    fn empty_and_bad_headers_are_rejected() {
        assert!(matches!(
            parse_log::<DataEvent>(""),
            Err(ParseError::BadHeader(_))
        ));
        assert!(matches!(
            parse_log::<DataEvent>("not a header\n"),
            Err(ParseError::BadHeader(_))
        ));
        assert!(matches!(
            parse_log::<DataEvent>("{\"version\":99}\n"),
            Err(ParseError::UnsupportedVersion { found: 99, .. })
        ));
    }

    #[test]
    fn data_log_roundtrip() {
        let mut buffer = header_line();
        buffer.push('\n');
        append_line(
            &mut buffer,
            &Envelope::new(
                1,
                1,
                DataEvent::RequestStart {
                    request_id: 1,
                    method: "GET".into(),
                    authority: "svc.a:80".into(),
                    path: "/".into(),
                    sni: None,
                    insecure: false,
                },
            ),
        )
        .unwrap();
        let events = parse_log::<DataEvent>(&buffer).unwrap();
        assert_eq!(events[0].envelope.event.kind(), "request/start");
    }
}

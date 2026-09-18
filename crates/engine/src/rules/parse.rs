//! hosts 风格输入的解析 —— 契约见 `spec/rules.md`。
//!
//! **与 Python 实现逐字节对齐**是这个模块的核心要求（不是"大致一样"）：
//! 同一批 `spec/fixtures/rules/*.json`，Rust 与 Python 两份实现都要跑，
//! 渲染输出必须逐字节一致。所以这里刻意照着 `rules.md` 的 BNF 与
//! 跳过原因码写，而不是"写个更漂亮的解析器"。

use std::collections::BTreeMap;

use crate::{is_ip_literal, parse_ip_literal, strip_brackets};

use super::host::validate_host;

/// 跳过原因码 —— 稳定字面量，UI 与 fixture 都按它断言，不要改字。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkipReason {
    TooFewTokens,
    NoIpLiteral,
    InvalidHost,
}

impl SkipReason {
    pub fn as_str(self) -> &'static str {
        match self {
            SkipReason::TooFewTokens => "too-few-tokens",
            SkipReason::NoIpLiteral => "no-ip-literal",
            SkipReason::InvalidHost => "invalid-host",
        }
    }
}

/// 被忽略的一行（或一行里的一个 token）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedItem {
    /// 行号，从 1 开始。
    pub line: usize,
    /// 该行**截断注释并 strip 后**的 body。
    pub text: String,
    pub reason: SkipReason,
    pub detail: String,
}

/// 同一个 host 被映射到不同 ip：`kept` 是最终生效的（后出现者）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostConflict {
    pub host: String,
    pub dropped: String,
    pub kept: String,
}

/// 一次解析的完整结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RulesImport {
    /// 归一化 host → ip 文本（后出现者胜）。
    pub entries: BTreeMap<String, String>,
    pub skipped: Vec<SkippedItem>,
    pub conflicts: Vec<HostConflict>,
    pub total_lines: usize,
    pub blank_lines: usize,
    pub comment_lines: usize,
    pub data_lines: usize,
}

impl RulesImport {
    pub fn accepted(&self) -> usize {
        self.entries.len()
    }

    /// 去重后的 ip 数。
    pub fn ips(&self) -> usize {
        self.entries
            .values()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    }

    /// 契约里的结果载荷（`spec/rules.md` §7），用于契约测试与 REST 面。
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "entries": self.entries,
            "accepted": self.accepted(),
            "ips": self.ips(),
            "conflicts": self
                .conflicts
                .iter()
                .map(|c| serde_json::json!([c.host, c.dropped, c.kept]))
                .collect::<Vec<_>>(),
            "skipped": self
                .skipped
                .iter()
                .map(|s| serde_json::json!({
                    "line": s.line,
                    "text": s.text,
                    "reason": s.reason.as_str(),
                    "detail": s.detail,
                }))
                .collect::<Vec<_>>(),
            "stats": self.stats_json(),
        })
    }

    pub fn stats_json(&self) -> serde_json::Value {
        serde_json::json!({
            "total_lines": self.total_lines,
            "blank_lines": self.blank_lines,
            "comment_lines": self.comment_lines,
            "data_lines": self.data_lines,
            "accepted": self.accepted(),
            "skipped": self.skipped.len(),
            "conflicts": self.conflicts.len(),
        })
    }
}

/// 解析 hosts 风格文本。**不抛异常**：非法内容记进 `skipped`。
pub fn parse_hosts_text(text: &str) -> RulesImport {
    let mut result = RulesImport::default();

    for (index, raw) in split_lines(text).into_iter().enumerate() {
        result.total_lines += 1;
        let body: String = raw.split('#').next().unwrap_or("").trim().to_string();
        if body.is_empty() {
            // 注意：注释行与空行的区分依据是**原文** strip 后是否以 `#` 开头，
            // 不是截断结果的形状。
            if raw.trim_start().starts_with('#') {
                result.comment_lines += 1;
            } else {
                result.blank_lines += 1;
            }
            continue;
        }

        result.data_lines += 1;
        let tokens: Vec<String> = body
            .split_whitespace()
            .map(|token| token.to_string())
            .collect();
        let line_number = index + 1;

        if tokens.len() < 2 {
            result.skipped.push(SkippedItem {
                line: line_number,
                text: body,
                reason: SkipReason::TooFewTokens,
                detail: tokens.first().cloned().unwrap_or_default(),
            });
            continue;
        }

        // 兼容两种写法：ip 在前是 hosts 文件惯例，ip 在后是不少团队的手写习惯。
        let (ip_token, host_tokens): (&str, &[String]) = if is_ip_literal(&tokens[0]) {
            (&tokens[0], &tokens[1..])
        } else if is_ip_literal(&tokens[tokens.len() - 1]) {
            (&tokens[tokens.len() - 1], &tokens[..tokens.len() - 1])
        } else {
            result.skipped.push(SkippedItem {
                line: line_number,
                text: body,
                reason: SkipReason::NoIpLiteral,
                detail: String::new(),
            });
            continue;
        };

        // 防御分支：`is_ip_literal` 已经解析成功过，这里不会失败；但保留它，
        // 好让"某个 token 看着像 IP 却过不了校验"（未来的解析器差异）走同一条
        // 响亮记录，而不是 panic。
        if parse_ip_literal(ip_token).is_none() {
            result.skipped.push(SkippedItem {
                line: line_number,
                text: body,
                reason: SkipReason::NoIpLiteral,
                detail: ip_token.to_string(),
            });
            continue;
        }
        // 保留作者写的拼法（`spec/rules.md` §3.2「拼法保留」）：只剥方括号，不做 IP 规范化。
        let ip_text = strip_brackets(ip_token).to_string();

        for token in host_tokens {
            if is_ip_literal(token) {
                // 例：`1.2.3.4 5.6.7.8` —— 多出来的 IP 不是 host
                result.skipped.push(SkippedItem {
                    line: line_number,
                    text: body.clone(),
                    reason: SkipReason::InvalidHost,
                    detail: token.clone(),
                });
                continue;
            }
            let Some(host) = validate_host(token) else {
                result.skipped.push(SkippedItem {
                    line: line_number,
                    text: body.clone(),
                    reason: SkipReason::InvalidHost,
                    detail: token.clone(),
                });
                continue;
            };

            if let Some(previous) = result.entries.get(&host)
                && previous != &ip_text
            {
                result.conflicts.push(HostConflict {
                    host: host.clone(),
                    dropped: previous.clone(),
                    kept: ip_text.clone(),
                });
            }
            result.entries.insert(host, ip_text.clone()); // 后出现者胜
        }
    }

    result
}

/// 按契约承认的三种行终止符切分：`\n`、`\r\n`、`\r`。
///
/// **不能**用 `str::lines()`（它只认 `\n` 与 `\r\n`，会把孤立的 `\r` 留在行内），
/// 也不能用 Python 的 `splitlines()`（它还会在 `\v`/`\f`/`\x1c`–`\x1e`/`\x85`/
/// `\u2028`/`\u2029` 断行，那属于 v1 的实现细节，不在 v2 契约内）。
///
/// 行为对齐 Python 的 `splitlines()`：**末尾的终止符不产生空行**。
fn split_lines(text: &str) -> Vec<String> {
    // 行首 BOM（可能多个）一并剥掉；不剥的话第一行会解析失败。
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '\n' => lines.push(std::mem::take(&mut current)),
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                lines.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    // 空文本 → 零行（与 Python `"".splitlines()` 一致）
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_contract_line_terminators_only() {
        assert_eq!(split_lines(""), Vec::<String>::new());
        assert_eq!(split_lines("a"), vec!["a"]);
        assert_eq!(split_lines("a\n"), vec!["a"]);
        assert_eq!(split_lines("a\r\nb"), vec!["a", "b"]);
        assert_eq!(split_lines("a\rb"), vec!["a", "b"]);
        // 契约外的分隔符不是行终止符
        assert_eq!(split_lines("a\u{0b}b"), vec!["a\u{0b}b"]);
        // BOM 剥掉（可多个）
        assert_eq!(
            split_lines("\u{feff}\u{feff}10.0.0.1 a.example.com"),
            vec!["10.0.0.1 a.example.com"]
        );
    }

    #[test]
    fn accepts_both_ip_first_and_host_first() {
        let import = parse_hosts_text("10.0.0.1 a.example.com\nb.example.com 10.0.0.2\n");
        assert_eq!(
            import.entries.get("a.example.com").map(String::as_str),
            Some("10.0.0.1")
        );
        assert_eq!(
            import.entries.get("b.example.com").map(String::as_str),
            Some("10.0.0.2")
        );
        assert_eq!(import.accepted(), 2);
        assert!(import.skipped.is_empty());
    }

    #[test]
    fn one_bad_host_does_not_drop_its_line_neighbours() {
        let import = parse_hosts_text("10.0.0.6 good.example.com -bad.example.com\n");
        assert_eq!(import.entries.len(), 1);
        assert_eq!(import.skipped.len(), 1);
        assert_eq!(import.skipped[0].reason, SkipReason::InvalidHost);
        assert_eq!(import.skipped[0].detail, "-bad.example.com");
    }

    #[test]
    fn line_without_any_ip_is_skipped_as_a_whole() {
        let import = parse_hosts_text("300.1.2.3 out-of-range.example.com\n");
        assert!(import.entries.is_empty());
        assert_eq!(import.skipped[0].reason, SkipReason::NoIpLiteral);
        assert_eq!(import.skipped[0].detail, "");
    }

    #[test]
    fn last_wins_records_a_conflict_but_same_ip_does_not() {
        let import = parse_hosts_text(
            "10.0.0.10 c.example.com\n10.0.0.11 c.example.com\n10.0.0.12 d.example.com\n10.0.0.12 d.example.com\n",
        );
        assert_eq!(
            import.entries.get("c.example.com").map(String::as_str),
            Some("10.0.0.11")
        );
        assert_eq!(import.conflicts.len(), 1, "same ip twice is not a conflict");
        assert_eq!(import.conflicts[0].dropped, "10.0.0.10");
        assert_eq!(import.conflicts[0].kept, "10.0.0.11");
    }

    #[test]
    fn comment_and_blank_lines_are_counted_separately() {
        let import = parse_hosts_text(
            "\n# full line\n   10.0.0.4  indented.example.com   # trailing\n\n#10.0.0.9 commented.out\n",
        );
        assert_eq!(import.total_lines, 5);
        assert_eq!(import.blank_lines, 2);
        assert_eq!(import.comment_lines, 2);
        assert_eq!(import.data_lines, 1);
        assert_eq!(import.accepted(), 1);
    }

    #[test]
    fn ip_text_keeps_the_authors_spelling() {
        let import =
            parse_hosts_text("2001:0db8::1 v6.example.com\n[2001:db8::2] v6b.example.com\n");
        assert_eq!(
            import.entries.get("v6.example.com").map(String::as_str),
            Some("2001:0db8::1")
        );
        assert_eq!(
            import.entries.get("v6b.example.com").map(String::as_str),
            Some("2001:db8::2")
        );
    }
}

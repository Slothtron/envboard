//! host 归一化与校验 —— 契约见 `core/spec/rules.md` §3.1。
//!
//! 这是全仓**唯一**的 host 归一化定义：注册表查表、规则文件的键、`*.` 前缀处理
//! 都走它。任何一处漏掉归一化，都会造成"配置写了却永不命中"的静默失效。

/// host 的规范形式：去空白 → 转小写 → 去**尾部所有**根点 → 剥掉**一个**前导 `*.`。
///
/// 注意 `*.` 只是被剥掉，**不做通配后缀匹配**：`*.wild.example.com` 的语义是
/// "wild.example.com 这一个域名"，不是"该后缀下所有域名"。
pub fn normalize_host(value: &str) -> String {
    let lowered = value.trim().to_lowercase();
    let without_root = lowered.trim_end_matches('.');
    without_root
        .strip_prefix("*.")
        .unwrap_or(without_root)
        .to_string()
}

/// 校验归一化后的 host（`validate_host` 的第二步）。
///
/// 规则：非空、总长 ≤ 253、按 `.` 切分后每个 label 匹配
/// `^(?!-)[A-Za-z0-9_-]{1,63}(?<!-)$`（长度 1–63，不能以 `-` 开头或结尾，允许 `_`）。
pub fn validate_normalized_host(host: &str) -> bool {
    if host.is_empty() || host.chars().count() > 253 {
        return false;
    }
    host.split('.').all(is_valid_label)
}

fn is_valid_label(label: &str) -> bool {
    let count = label.chars().count();
    if count == 0 || count > 63 {
        return false;
    }
    if label.starts_with('-') || label.ends_with('-') {
        return false;
    }
    label
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// 归一化 + 校验一步到位；不合法返回 `None`。
pub fn validate_host(value: &str) -> Option<String> {
    let host = normalize_host(value);
    validate_normalized_host(&host).then_some(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_case_root_dot_and_wildcard_prefix() {
        assert_eq!(normalize_host("API.Example.COM."), "api.example.com");
        assert_eq!(normalize_host("*.wild.example.com"), "wild.example.com");
        assert_eq!(
            normalize_host("  spaced.example.com  "),
            "spaced.example.com"
        );
        // 只剥一次，且只在开头
        assert_eq!(normalize_host("*.*.a.example.com"), "*.a.example.com");
    }

    #[test]
    fn rejects_empty_labels_and_bad_edges() {
        for bad in ["", "-lead.example.com", "trail-.example.com", "a..b", "a b"] {
            assert!(validate_host(bad).is_none(), "{bad:?} must be rejected");
        }
        // `_` 是允许的（v1 的 LABEL_RE 如此），数字开头也允许
        assert!(validate_host("_svc.9beta.example.com").is_some());
    }

    #[test]
    fn label_length_is_capped_at_63() {
        let label = "a".repeat(64);
        assert!(validate_host(&format!("{label}.example.com")).is_none());
        let ok = "a".repeat(63);
        assert!(validate_host(&format!("{ok}.example.com")).is_some());
    }
}

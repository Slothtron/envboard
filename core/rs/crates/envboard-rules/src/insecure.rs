//! `insecure_hosts` 的归一化与匹配 —— 契约见 `core/spec/capabilities.md` 的
//! 「insecure_hosts（按域名放宽上游校验）」一节。
//!
//! 与 [`crate::host`] 的规则归一化**刻意不同**的两点，写在这里免得被"顺手统一"掉：
//!
//! * **不剥 `*.`**：出现 `*` 或 `?` 一律判非法（契约要求逐条列出完整域名，
//!   把 `*.example.com` 偷偷存成 `example.com` 会让"没生效"变成"生效范围比你以为的大"）；
//! * 匹配是**完全相等**：没有子域继承，也没有后缀匹配
//!   （`example.com` 既不匹配 `app.example.com`，也不匹配 `example.com.evil`）。

/// 列表长度上限。放宽上游校验是安全控制，不是配置项，所以有硬上限。
pub const INSECURE_HOSTS_MAX: usize = 200;

/// 归一化：trim → 小写 → 去掉**尾部所有**根点。
///
/// 刻意不调用 [`crate::host::normalize_host`]：那个函数会剥掉前导 `*.`。
pub fn normalize_insecure_host(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .trim_end_matches('.')
        .to_string()
}

/// 是否含通配符（`*` 或 `?`）—— 写入时拒绝，运行时不解释。
pub fn has_wildcard(value: &str) -> bool {
    value.contains('*') || value.contains('?')
}

/// 归一化 + 校验一步到位；非法（含通配符，或 label 不合法）返回 `None`。
pub fn validate_insecure_host(value: &str) -> Option<String> {
    let host = normalize_insecure_host(value);
    if has_wildcard(&host) {
        return None;
    }
    crate::host::validate_normalized_host(&host).then_some(host)
}

/// 命中判定：归一化后的 SNI **完全等于**集合里的某个元素。
///
/// 没有 SNI（`None` 或空串）时退回上连地址（IP 字面量）—— 这正是
/// "规则把域名改写到内网 IP，客户端又不发 SNI"那条路径。
/// SNI 存在但不在集合里**不**退回地址：退回会让一次命名失配变成一次静默放行。
pub fn matches(hosts: &[String], sni: Option<&str>, address: Option<&str>) -> bool {
    let candidate = sni
        .map(normalize_insecure_host)
        .filter(|value| !value.is_empty())
        .or_else(|| address.map(normalize_insecure_host));
    let Some(candidate) = candidate else {
        return false;
    };
    hosts
        .iter()
        .any(|host| normalize_insecure_host(host) == candidate)
}

/// 归一化 + 去重 + 排序 —— 落盘形状由这里决定，同一份输入永远得到同一份列表。
pub fn normalize_insecure_hosts(values: &[String]) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for value in values {
        let host = normalize_insecure_host(value);
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    hosts.sort();
    hosts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn normalizes_case_and_root_dots_without_touching_wildcards() {
        assert_eq!(
            normalize_insecure_host("  App.Example.COM.  "),
            "app.example.com"
        );
        // 与 rules 的 normalize_host 相反：不剥 `*.`，交由校验拒绝
        assert_eq!(normalize_insecure_host("*.example.com"), "*.example.com");
        assert!(validate_insecure_host("*.example.com").is_none());
        assert!(validate_insecure_host("a?.example.com").is_none());
    }

    #[test]
    fn rejects_invalid_labels_but_accepts_ip_literals() {
        assert!(validate_insecure_host("-lead.example.com").is_none());
        assert!(validate_insecure_host("a..b").is_none());
        assert!(validate_insecure_host("10.0.0.8").is_some());
        assert!(validate_insecure_host("app.example.com").is_some());
    }

    #[test]
    fn matching_is_exact_and_never_inherits_subdomains() {
        let hosts = list(&["app.example.com"]);
        assert!(matches(&hosts, Some("app.example.com"), None));
        assert!(matches(&hosts, Some("App.Example.COM."), None));
        assert!(!matches(&hosts, Some("web.example.net"), None));
        // 不做子域继承，也不做后缀匹配
        assert!(!matches(
            &list(&["example.com"]),
            Some("app.example.com"),
            None
        ));
        assert!(!matches(
            &list(&["example.com"]),
            Some("example.com.evil"),
            None
        ));
        assert!(!matches(&[], Some("app.example.com"), None));
    }

    #[test]
    fn falls_back_to_the_address_only_without_sni() {
        let hosts = list(&["10.0.0.8"]);
        assert!(matches(&hosts, None, Some("10.0.0.8")));
        assert!(matches(&hosts, Some(""), Some("10.0.0.8")));
        // SNI 存在但没命中：不退回地址
        assert!(!matches(&hosts, Some("app.example.com"), Some("10.0.0.8")));
        // 两边都没有 → 不放行
        assert!(!matches(&hosts, None, None));
    }

    #[test]
    fn normalization_is_deduped_and_sorted() {
        assert_eq!(
            normalize_insecure_hosts(&list(&[
                " B.example.com ",
                "a.example.com",
                "B.EXAMPLE.COM."
            ])),
            list(&["a.example.com", "b.example.com"])
        );
    }
}

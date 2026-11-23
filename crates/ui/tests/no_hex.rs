//! D1 的守门人：`crates/ui/src` 里不许出现颜色字面量。
//!
//! 判据是**任何** `"#rgb"` / `"#rrggbb"` / `"#rrggbbaa"` 形态的字符串，
//! 不管它被谁用。颜色只有两个合法来源：主题 JSON 的键，或 `AdaptiveColor::TRANSPARENT`。
//! 这条测试不启用正则依赖：八个字符的滑窗就够，也不必和 regex 的方言对齐。

const SOURCES: [(&str, &str); 4] = [
    ("font.rs", include_str!("../src/font.rs")),
    ("metrics.rs", include_str!("../src/metrics.rs")),
    ("primitives.rs", include_str!("../src/primitives.rs")),
    ("tokens.rs", include_str!("../src/tokens.rs")),
];

/// 找出 `"#` 后紧跟 3/6 位十六进制的字符串字面量。
fn hex_literals(src: &str) -> Vec<String> {
    let bytes = src.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i + 4 < bytes.len() {
        if bytes[i] == b'"' && bytes[i + 1] == b'#' {
            // 6 位优先判（8 位带 alpha 的前 6 位也在这里被抓到）
            let six = i + 8 <= bytes.len()
                && bytes[i + 2..i + 8].iter().all(u8::is_ascii_hexdigit)
                && (i + 8 == bytes.len() || bytes[i + 8] == b'"');
            let three = !six
                && i + 5 <= bytes.len()
                && bytes[i + 2..i + 5].iter().all(u8::is_ascii_hexdigit)
                && (i + 5 == bytes.len() || bytes[i + 5] == b'"');
            if six || three {
                let end = if six { i + 8 } else { i + 5 };
                found.push(src[i + 1..end].to_string());
                i = end + 1;
                continue;
            }
        }
        i += 1;
    }
    found
}

#[test]
fn no_colour_literals_in_sources() {
    let mut violations = Vec::new();
    for (name, src) in SOURCES {
        for lit in hex_literals(src) {
            violations.push(format!("{name}: {lit}"));
        }
    }
    assert!(
        violations.is_empty(),
        "颜色字面量必须换成 tokens.rs 的具名角色：\n{}",
        violations.join("\n")
    );
}

#[test]
fn scanner_actually_catches_a_literal() {
    // 扫描器自己也要有一条测试：不然它可能一直在“通过”。
    assert_eq!(hex_literals(r##"bg("#9d2933")"##), vec!["#9d2933"]);
    assert_eq!(hex_literals(r##"border("#fff")"##), vec!["#fff"]);
    assert!(hex_literals(r#"text("env #1 bad")"#).is_empty());
    assert!(hex_literals(r#"t::accent()"#).is_empty());
}

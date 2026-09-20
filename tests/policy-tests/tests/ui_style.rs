//! UI 样式契约门禁：执行 `spec/ui.md` 机检档的 UI-1…UI-8。
//!
//! 为什么需要它：工作台是零构建链的原生三资产（index.html / app.css / app.js），
//! 没有任何前端工具链能在构建期拦住样式漂移 —— 唯一能拦住它的位置就是 policy 门禁。
//! 规则条文全部在契约文件里（机检档），本文件只做执行器；两者靠 `UI-n` 锚点双向
//! 对账（UI-8）：契约缺锚点、或门禁实现了契约没有的规则号，都判红。
//!
//! 判据与实现口径：
//!
//! * **令牌区**：app.css 里选择器为 `:root` 或 `[data-theme=…]` 的块。它是颜色
//!   字面量（UI-1）、字号档位（UI-2）等规则的唯一豁免域 —— 令牌定义本身就是字面量。
//! * **注释先剥掉再查**：注释是给人看的文档，不是渲染结果；不剥会让「禁止纯黑
//!   #000」这类说理文字把自己判红。CSS 剥 `/* */`，JS 剥 `//` 与 `/* */`，
//!   HTML 剥 `<!-- -->`。剥注释是**保守方向**：只会放过写在注释里的违规，不会
//!   制造冤案。
//! * **手写扫描不用正则回溯**：与 `doc_scope.rs` 同一手法 —— 定位锚点子串，然后
//!   逐字符吃掉合法成分，保证输出稳定的 `文件:行号`。
//!
//! 本文件自己的源码不受本门禁扫描（它只扫三个资产与契约文件）；它受 doc_scope
//! 与 clippy 等其余门禁约束。

mod common;

use std::collections::BTreeSet;

use common::{read_text, report, workspace_root};

/// 门禁内建的规则清单（UI-8 的对账基准，顺序即报告顺序）。
const RULE_IDS: &[&str] = &[
    "UI-1", "UI-2", "UI-3", "UI-4", "UI-5", "UI-6", "UI-7", "UI-8",
];

const APP_CSS: &str = "crates/server/assets/app.css";
const APP_JS: &str = "crates/server/assets/app.js";
const INDEX_HTML: &str = "crates/server/assets/index.html";
const SPEC_MD: &str = "spec/ui.md";

/// UI-7：内联事件属性名单（CSP 严格，事件一律 `addEventListener`）。
const INLINE_EVENT_ATTRS: &[&str] = &[
    "onclick=",
    "onload=",
    "onerror=",
    "onchange=",
    "onsubmit=",
    "oninput=",
    "onkeydown=",
    "onkeyup=",
    "onmouseover=",
];

/// UI-5：受 4px 基数约束的间距属性。子属性必须显式列出 —— 搜 `padding:` 匹配不到
/// `padding-left:`，而裸搜 `padding` 会命中无关词。
const SPACING_PROPS: &[&str] = &[
    "padding",
    "padding-top",
    "padding-right",
    "padding-bottom",
    "padding-left",
    "margin",
    "margin-top",
    "margin-right",
    "margin-bottom",
    "margin-left",
    "gap",
    "row-gap",
    "column-gap",
    "grid-gap",
];

/// UI-5 的登记例外：发丝 / 微调档。超过它的裸 px 一律判红。
const SPACING_PX_ALLOWED: &[i64] = &[1, 2];

// --------------------------------------------------------------------------- #
// 文本预处理：按行剥注释（保守方向：注释不是渲染结果）
// --------------------------------------------------------------------------- #

fn count_char(text: &str, needle: char) -> i32 {
    text.chars().filter(|c| *c == needle).count() as i32
}

/// CSS：跨行剥 `/* */`。返回与输入行号一一对应的干净行。
fn strip_block_comments(lines: &[&str]) -> Vec<String> {
    let mut cleaned = Vec::with_capacity(lines.len());
    let mut in_comment = false;
    for line in lines {
        let mut kept = String::with_capacity(line.len());
        let mut rest = *line;
        loop {
            if in_comment {
                match rest.find("*/") {
                    Some(end) => {
                        in_comment = false;
                        rest = &rest[end + 2..];
                    }
                    None => break,
                }
            } else {
                match rest.find("/*") {
                    Some(start) => {
                        kept.push_str(&rest[..start]);
                        in_comment = true;
                        rest = &rest[start + 2..];
                    }
                    None => {
                        kept.push_str(rest);
                        break;
                    }
                }
            }
        }
        cleaned.push(kept);
    }
    cleaned
}

/// JS：在 [`strip_block_comments`] 之上再剥行注释 `//`。字符串里的 `https://`
/// 会被误切 —— 只影响本门禁的可见范围（保守方向），不影响仓库本身。
fn strip_js_comments(lines: &[&str]) -> Vec<String> {
    strip_block_comments(lines)
        .into_iter()
        .map(|line| match line.find("//") {
            Some(index) => line[..index].to_string(),
            None => line,
        })
        .collect()
}

/// HTML：剥 `<!-- -->`（按行处理即可，资产里没有跨行注释；出现跨行注释时只漏检
/// 注释里的违规，方向保守）。
fn strip_html_comments(lines: &[&str]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            let mut kept = String::with_capacity(line.len());
            let mut rest = *line;
            while let Some(start) = rest.find("<!--") {
                match rest[start..].find("-->") {
                    Some(end) => {
                        kept.push_str(&rest[..start]);
                        rest = &rest[start + end + 3..];
                    }
                    None => {
                        kept.push_str(&rest[..start]);
                        rest = "";
                        break;
                    }
                }
            }
            kept.push_str(rest);
            kept
        })
        .collect()
}

// --------------------------------------------------------------------------- #
// 令牌区界定与令牌清单
// --------------------------------------------------------------------------- //

fn is_token_selector(line: &str) -> bool {
    let trimmed = line.trim_start();
    (trimmed.starts_with(":root") || trimmed.contains("[data-theme")) && line.contains('{')
}

/// 逐行标注是否处于令牌区。行号 1 起，与文件行号一致。
fn token_zone_flags(lines: &[String]) -> Vec<bool> {
    let mut flags = Vec::with_capacity(lines.len());
    let mut in_zone = false;
    let mut depth = 0;
    for line in lines {
        if !in_zone && is_token_selector(line) {
            depth = count_char(line, '{') - count_char(line, '}');
            flags.push(true);
            in_zone = depth > 0;
        } else if in_zone {
            depth += count_char(line, '{') - count_char(line, '}');
            flags.push(true);
            if depth <= 0 {
                in_zone = false;
            }
        } else {
            flags.push(false);
        }
    }
    flags
}

/// 令牌区里的 `--name:` 定义清单。剥过注释，所以只看得到真定义。
fn defined_tokens(zone_lines: &[&str]) -> BTreeSet<String> {
    let mut tokens = BTreeSet::new();
    for line in zone_lines {
        let bytes = line.as_bytes();
        let mut at = 0;
        while at + 1 < bytes.len() {
            if bytes[at] == b'-' && bytes[at + 1] == b'-' {
                let mut end = at + 2;
                while end < bytes.len()
                    && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'-')
                {
                    end += 1;
                }
                let name = &line[at + 2..end];
                if !name.is_empty() && !name.ends_with('-') && bytes.get(end) == Some(&b':') {
                    tokens.insert(name.to_string());
                }
                at = end.max(at + 2);
            } else {
                at += 1;
            }
        }
    }
    tokens
}

/// 全文件收集 `var(--name)` 引用（`文件:行号` 一并返回，报错用）。
fn var_references(file: &str, lines: &[String]) -> Vec<(String, usize, String)> {
    let mut found = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let bytes = line.as_bytes();
        let mut at = 0;
        while at + 5 < bytes.len() {
            if &bytes[at..at + 5] != b"var(--" {
                at += 1;
                continue;
            }
            let mut end = at + 5;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'-') {
                end += 1;
            }
            let name = &line[at + 5..end];
            if !name.is_empty() && !name.ends_with('-') {
                found.push((file.to_string(), index + 1, name.to_string()));
            }
            at = end.max(at + 5);
        }
    }
    found
}

// --------------------------------------------------------------------------- //
// 行内属性值提取
// --------------------------------------------------------------------------- //

/// 取 `prop:` 的值（到 `;` 为止），返回 `(冒号后起点, 值文本)` 列表。
/// 词边界：`prop` 前一个字符不得是字母 / 数字 / `-`（否则 `gap:` 会命中 `grid-gap:`）。
fn property_values<'a>(line: &'a str, prop: &str) -> Vec<(usize, &'a str)> {
    let mut found = Vec::new();
    let needle = format!("{prop}:");
    let mut from = 0;
    while let Some(offset) = line[from..].find(&needle) {
        let start = from + offset;
        let before_ok = line[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '-'));
        if before_ok {
            let value_start = start + needle.len();
            let value_end = line[value_start..]
                .find(';')
                .map(|end| value_start + end)
                .unwrap_or(line.len());
            found.push((value_start, &line[value_start..value_end]));
        }
        from = start + needle.len();
    }
    found
}

fn is_hex_digit(c: u8) -> bool {
    c.is_ascii_digit() || (b'a'..=b'f').contains(&c) || (b'A'..=b'F').contains(&c)
}

/// UI-1：一行里的颜色字面量（hex 3/4/6/8 位，rgb/rgba/hsl/hsla 函数）。
fn color_literals(line: &str) -> bool {
    let bytes = line.as_bytes();
    for func in ["rgba(", "rgb(", "hsla(", "hsl("] {
        if line.contains(func) {
            return true;
        }
    }
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] != b'#' {
            at += 1;
            continue;
        }
        let mut end = at + 1;
        while end < bytes.len() && is_hex_digit(bytes[end]) {
            end += 1;
        }
        let run = end - at - 1;
        let followed_by_hex = end < bytes.len() && is_hex_digit(bytes[end]);
        if !followed_by_hex && matches!(run, 3 | 4 | 6 | 8) {
            return true;
        }
        at = end.max(at + 1);
    }
    false
}

/// UI-5：值文本里的裸 px 数值（含负号）；`var(--space-3)` 里的 `3` 后面不是 px，
/// 不会误报。
fn raw_spacing_px(value: &str) -> Vec<i64> {
    let bytes = value.as_bytes();
    let mut numbers = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let (negative, digit_start) = match bytes[at] {
            b'-' if at + 1 < bytes.len() && bytes[at + 1].is_ascii_digit() => (true, at + 1),
            b'0'..=b'9' => (false, at),
            _ => {
                at += 1;
                continue;
            }
        };
        let mut end = digit_start;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        let followed_by_px = end + 2 <= bytes.len() && &bytes[end..end + 2] == b"px";
        if followed_by_px {
            let magnitude: i64 = value[digit_start..end].parse().unwrap_or(i64::MAX);
            numbers.push(if negative { -magnitude } else { magnitude });
        }
        at = end.max(at + 1);
    }
    numbers
}

fn squash_spaces(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// UI-4 的纯黑判定：hex 前三位全零（`#000` / `#000f` / `#000000` / `#00000000`
/// 都是黑系），或 rgb/rgba 首分量整体为零。`#0000ff` 这类非黑不误伤。
fn pure_black(line: &str) -> bool {
    let squashed = squash_spaces(line);
    if squashed.contains("rgba(0,") || squashed.contains("rgb(0,") {
        return true;
    }
    let bytes = line.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] != b'#' {
            at += 1;
            continue;
        }
        let mut end = at + 1;
        while end < bytes.len() && is_hex_digit(bytes[end]) {
            end += 1;
        }
        let run = &bytes[at + 1..end];
        if run.len() >= 3 && run[..3].iter().all(|c| *c == b'0') {
            return true;
        }
        at = end.max(at + 1);
    }
    false
}

// --------------------------------------------------------------------------- //
// 门禁主体
// --------------------------------------------------------------------------- //

#[test]
fn ui_contract_holds() {
    let root = workspace_root();
    let read = |path: &str| {
        read_text(&root.join(path)).unwrap_or_else(|| panic!("missing required file {path}"))
    };
    let css_text = read(APP_CSS);
    let js_text = read(APP_JS);
    let html_text = read(INDEX_HTML);
    let css_lines: Vec<&str> = css_text.lines().collect();
    let js_lines: Vec<&str> = js_text.lines().collect();
    let html_lines: Vec<&str> = html_text.lines().collect();
    let css = strip_block_comments(&css_lines);
    let js = strip_js_comments(&js_lines);
    let html = strip_html_comments(&html_lines);

    let mut problems = Vec::new();

    // 令牌区与令牌清单（UI-1/2/3/4/5 的豁免域；UI-6 的定义侧）。
    let zone = token_zone_flags(&css);
    let zone_lines: Vec<&str> = css
        .iter()
        .enumerate()
        .filter(|(index, _)| zone[*index])
        .map(|(_, line)| line.as_str())
        .collect();
    let tokens = defined_tokens(&zone_lines);

    // UI-1：色值只许在令牌区；html/js 一律禁止。
    for (index, line) in css.iter().enumerate() {
        if !zone[index] && color_literals(line) {
            problems.push(format!(
                "UI-1 {}:{} 颜色字面量出现在令牌区之外（组件层一律 var(--token)）",
                APP_CSS,
                index + 1
            ));
        }
    }
    for (file, lines) in [(APP_JS, &js), (INDEX_HTML, &html)] {
        for (index, line) in lines.iter().enumerate() {
            if color_literals(line) {
                problems.push(format!(
                    "UI-1 {file}:{} 资产脚本/结构层出现颜色字面量（一律 class + var(--token)）",
                    index + 1
                ));
            }
        }
    }

    // UI-2 / UI-3 / UI-4 / UI-5：令牌区之外的属性形态约束。
    for (index, line) in css.iter().enumerate() {
        if zone[index] {
            continue;
        }
        let line_no = index + 1;
        for (start, value) in property_values(line, "font-size") {
            if !value.trim_start().starts_with("var(--text-") {
                problems.push(format!(
                    "UI-2 {APP_CSS}:{line_no} font-size 第 {} 列不是 var(--text-*)：{:?}",
                    start + 1,
                    value.trim()
                ));
            }
        }
        for (_start, value) in property_values(line, "border-radius") {
            let trimmed = value.trim();
            if !trimmed.starts_with("var(--radius") && trimmed != "50%" {
                problems.push(format!(
                    "UI-3 {APP_CSS}:{line_no} border-radius 取值 {:?} 不在圆角五档与 50% 之内",
                    trimmed
                ));
            }
        }
        for (_start, value) in property_values(line, "box-shadow") {
            let squashed = squash_spaces(value);
            let focus_ring =
                squashed == "0003pxvar(--accent-bg)" || squashed == "0003pxvar(--bad-bg)";
            if !squashed.starts_with("var(--shadow") && !focus_ring {
                problems.push(format!("UI-4 {APP_CSS}:{line_no} box-shadow 取值 {:?} 只允许 var(--shadow*) 或登记的焦点环形态", value.trim()));
            }
        }
        for prop in SPACING_PROPS {
            for (_start, value) in property_values(line, prop) {
                for number in raw_spacing_px(value) {
                    if !SPACING_PX_ALLOWED.contains(&number.abs()) {
                        problems.push(format!(
                            "UI-5 {APP_CSS}:{line_no} {prop} 含裸 px 值 {number}px（4px 基数，例外仅 1px/2px 发丝档）：{value:?}"
                        ));
                    }
                }
            }
        }
    }

    // UI-4 的纯黑禁令：全文件逐行（令牌区含内）。
    for (index, line) in css.iter().enumerate() {
        if pure_black(line) {
            problems.push(format!(
                "UI-4 {APP_CSS}:{} 出现纯黑（禁令不分令牌区内外；遮罩用 --overlay）",
                index + 1
            ));
        }
    }

    // UI-6：引用的令牌必须有定义。
    for (file, line_no, name) in var_references(APP_CSS, &css)
        .into_iter()
        .chain(var_references(APP_JS, &js))
        .chain(var_references(INDEX_HTML, &html))
    {
        if !tokens.contains(&name) {
            problems.push(format!(
                "UI-6 {file}:{line_no} var(--{name}) 未在令牌区定义（拼错或死引用）"
            ));
        }
    }

    // UI-7：交互形态。
    for (index, line) in js.iter().enumerate() {
        for banned in ["window.alert(", "window.confirm(", "window.prompt("] {
            if line.contains(banned) {
                problems.push(format!("UI-7 {APP_JS}:{} 禁用原生弹窗 {banned:?}（反馈走 toast/loading，确认走模态窗）", index + 1));
            }
        }
    }
    for (index, line) in html.iter().enumerate() {
        let line_no = index + 1;
        if line.contains("<script") && !line.contains("src=") {
            problems.push(format!(
                "UI-7 {INDEX_HTML}:{line_no} 内联 <script>（CSP 严格：只允许带 src= 的外链脚本）"
            ));
        }
        for attr in INLINE_EVENT_ATTRS {
            if line.contains(attr) {
                problems.push(format!(
                    "UI-7 {INDEX_HTML}:{line_no} 内联事件属性 {attr:?}（事件一律 addEventListener）"
                ));
            }
        }
    }

    // UI-8：契约锚点与门禁规则双向对账。
    let spec = read(SPEC_MD);
    let found_ids = rule_ids_in(&spec);
    for expected in RULE_IDS {
        if !found_ids.contains(*expected) {
            problems.push(format!(
                "UI-8 {SPEC_MD} 缺锚点 {expected}（契约与门禁必须同提交同步）"
            ));
        }
    }
    for found in &found_ids {
        if !RULE_IDS.contains(&found.as_str()) {
            problems.push(format!(
                "UI-8 {SPEC_MD} 的锚点 {found} 没有对应的门禁规则（契约与门禁必须同提交同步）"
            ));
        }
    }

    report(
        "ui-contract",
        problems,
        format!("UI-1…UI-8 × 3 资产 + {SPEC_MD}；令牌 {} 项", tokens.len()),
    );
}

/// 从契约文本里提取 `UI-n` 锚点（去重排序；非规则号引用如 UI-1…UI-8 里的两个端点
/// 也会被收进来，对账按集合语义，无碍）。
fn rule_ids_in(text: &str) -> BTreeSet<String> {
    let bytes = text.as_bytes();
    let mut ids = BTreeSet::new();
    let mut at = 0;
    while at + 3 < bytes.len() {
        if &bytes[at..at + 3] != b"UI-" {
            at += 1;
            continue;
        }
        let mut end = at + 3;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end > at + 3 {
            ids.insert(text[at..end].to_string());
        }
        at = end.max(at + 3);
    }
    ids
}

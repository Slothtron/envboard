//! UI 样式契约门禁：执行 `spec/ui.md` 机检档的 UI-1…UI-6。
//!
//! 为什么需要它：工作台前端是 Vite 工程（HeroUI + Tailwind），构建链本身不拦
//! 样式漂移 —— 语义类纪律（零色值、零内联样式、零深色分支）必须由 policy 门禁
//! 在提交期拦住。规则条文全部在契约文件里（机检档），本文件只做执行器；两者靠
//! `UI-n` 锚点双向对账（UI-6）：契约缺锚点、或门禁实现了契约没有的规则号，都判红。
//!
//! 判据与实现口径（v2.0，对象从三件套资产改为 frontend 源码）：
//!
//! * **扫描域**：`frontend/src/**` 的 `.ts` / `.tsx` / `.css` + `frontend/index.html`。
//!   构建产物 `frontend/dist/` 不扫 —— 它是压缩拼接物，色值以编译结果存在，
//!   漂移由 src ↔ dist 成对判定（toolchain 门禁 + frontend 层 drift）兜住。
//! * **注释先剥掉再查**：注释是给人看的文档，不剥会让「禁止 dark:」这类说理文字
//!   把自己判红。TS/TSX 剥 `//` 与 `/* */`，CSS 剥 `/* */`，HTML 剥 `<!-- -->`。
//!   剥注释是**保守方向**：只会放过写在注释里的违规，不会制造冤案。
//! * **手写扫描不用正则回溯**：与 `doc_scope.rs` 同一手法 —— 定位锚点子串，
//!   逐字符吃掉合法成分，保证输出稳定的 `文件:行号`。
//! * **globals.css 是登记处**：UI-4 的裸 px 豁免只给它（自定义样式的落点），
//!   UI-1 色值零容忍对它同样成立（BEM 类只 `@apply` 语义类）。

mod common;

use std::collections::BTreeSet;

use common::{read_text, report, walk_relative, workspace_root};

/// 门禁内建的规则清单（UI-6 的对账基准，顺序即报告顺序）。
const RULE_IDS: &[&str] = &["UI-1", "UI-2", "UI-3", "UI-4", "UI-5", "UI-6"];

const SPEC_MD: &str = "spec/ui.md";
const FRONTEND_INDEX: &str = "frontend/index.html";

/// UI-5：index.html 内联事件属性名单（CSP 严格；JSX 的 onPress/onChange 不在此列）。
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

/// UI-5：禁止的浏览器原生弹窗调用。
const NATIVE_DIALOGS: &[&str] = &["window.alert", "window.confirm", "window.prompt"];

fn is_scanned_source(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("frontend/src/") else {
        return false;
    };
    rest.ends_with(".ts") || rest.ends_with(".tsx") || rest.ends_with(".css")
}

fn strip_block_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        if let Some(end) = rest.find("*/") {
            rest = &rest[end + 2..];
        } else {
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

fn strip_line_and_block(text: &str) -> String {
    let stripped = strip_block_comments(text);
    let mut out = String::with_capacity(stripped.len());
    for line in stripped.lines() {
        // `//` 行注释：只在不处于字符串语义时剥 —— 保守起见按"行内首个 // 且
        // 前一个字符不是 : 或 /"处理；URL（https://）误剥无害（不产生误报，
        // 只会少扫内容，方向保守）。
        let cut = match line.find("//") {
            Some(index) if !line[..index].ends_with(':') => &line[..index],
            _ => line,
        };
        out.push_str(cut);
        out.push('\n');
    }
    out
}

fn strip_html_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        if let Some(end) = rest.find("-->") {
            rest = &rest[end + 3..];
        } else {
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

fn is_hex_digit(c: char) -> bool {
    c.is_ascii_digit() || ('a'..='f').contains(&c) || ('A'..='F').contains(&c)
}

/// UI-1：行内是否出现颜色字面量。hex 要求 `#` 后跟 3/4/6/8 位十六进制且
/// 下一位不是十六进制字符（避开 markdown 锚点等误报）；函数形态查 `rgb(`/`rgba(`/
/// `hsl(`/`hwb(`。
fn color_literal(line: &str) -> bool {
    let chars: Vec<char> = line.chars().collect();
    for (index, c) in chars.iter().enumerate() {
        if *c != '#' {
            continue;
        }
        let mut run = 0;
        let mut at = index + 1;
        while at < chars.len() && is_hex_digit(chars[at]) {
            run += 1;
            at += 1;
        }
        let next_is_hex = at < chars.len() && is_hex_digit(chars[at]);
        if !next_is_hex && matches!(run, 3 | 4 | 6 | 8) {
            return true;
        }
    }
    for func in ["rgb(", "rgba(", "hsl(", "hwb("] {
        if line.contains(func) {
            return true;
        }
    }
    false
}

/// UI-2：内联样式（`style={{`，允许中间空白）。
fn has_inline_style(line: &str) -> bool {
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains("style={{")
}

/// UI-4：Tailwind 任意值裸 px（`[…px]`）。
fn arbitrary_px(line: &str) -> bool {
    let chars: Vec<char> = line.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '[' {
            let mut at = index + 1;
            let mut digits = 0;
            while at < chars.len() && chars[at].is_ascii_digit() {
                digits += 1;
                at += 1;
            }
            if digits > 0
                && at + 2 < chars.len()
                && chars[at] == 'p'
                && chars[at + 1] == 'x'
                && chars[at + 2] == ']'
            {
                return true;
            }
        }
        index += 1;
    }
    false
}

/// UI-5：原生弹窗。`window.alert` 等直接子串；裸 `alert(`/`confirm(`/`prompt(`
/// 要求前导不是标识符字符或点（`onConfirm(` 不误判）。
fn native_dialog(line: &str) -> bool {
    if NATIVE_DIALOGS.iter().any(|banned| line.contains(banned)) {
        return true;
    }
    for bare in ["alert(", "confirm(", "prompt("] {
        if let Some(offset) = line.find(bare) {
            let before = if offset == 0 {
                None
            } else {
                line[..offset].chars().next_back()
            };
            let identifier_edge =
                before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.');
            if !identifier_edge {
                return true;
            }
        }
    }
    false
}

/// UI-5：index.html 的内联 `<script>`（无 src=）与 on* 事件属性。
fn html_inline_script_or_handler(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    if lower.contains("<script") && !lower.contains("src=") {
        return true;
    }
    INLINE_EVENT_ATTRS.iter().any(|attr| lower.contains(attr))
}

fn read_stripped(root: &std::path::Path, relative: &str) -> Vec<String> {
    let text = read_text(&root.join(relative)).unwrap_or_default();
    let stripped = if relative.ends_with(".html") {
        strip_html_comments(&text)
    } else if relative.ends_with(".css") {
        strip_block_comments(&text)
    } else {
        strip_line_and_block(&text)
    };
    stripped.lines().map(String::from).collect()
}

#[test]
fn ui_contract_holds() {
    let root = workspace_root();
    let sources: Vec<String> = walk_relative(&root)
        .into_iter()
        .filter(|path| is_scanned_source(path))
        .collect();

    let mut problems: Vec<String> = Vec::new();

    for file in &sources {
        let lines = read_stripped(&root, file);
        let is_script = file.ends_with(".ts") || file.ends_with(".tsx");
        for (number, line) in lines.iter().enumerate() {
            let at = format!("{file}:{}", number + 1);

            // UI-1 色值出没域（globals.css 同样零容忍）。
            if color_literal(line) {
                problems.push(format!(
                    "UI-1 {at} 颜色字面量只允许 HeroUI 语义类，源码零容忍"
                ));
            }
            // UI-2 内联样式禁令。
            if is_script && has_inline_style(line) {
                problems.push(format!(
                    "UI-2 {at} 内联 style（自定义样式走 globals.css BEM）"
                ));
            }
            // UI-3 深色覆盖禁令。
            if line.contains("dark:") {
                problems.push(format!(
                    "UI-3 {at} 手写 dark: 覆盖（暗色只靠 html.dark 整体切换）"
                ));
            }
            // UI-4 裸 px 任意值（只判脚本；globals.css 是登记处，不判）。
            if is_script && file != "frontend/src/globals.css" && arbitrary_px(line) {
                problems.push(format!("UI-4 {at} Tailwind 任意值裸 px（尺寸走刻度）"));
            }
            // UI-5 原生弹窗。
            if is_script && native_dialog(line) {
                problems.push(format!(
                    "UI-5 {at} 原生弹窗（反馈走 role=status Alert，确认走 AlertDialog）"
                ));
            }
        }
    }

    let index_lines = read_stripped(&root, FRONTEND_INDEX);
    for (number, line) in index_lines.iter().enumerate() {
        // UI-5 index.html：内联 script 与 on* 属性。
        if html_inline_script_or_handler(line) {
            problems.push(format!(
                "UI-5 {FRONTEND_INDEX}:{} 内联脚本或事件属性（CSP 严格，只允许外链）",
                number + 1
            ));
        }
        // UI-1 index.html：色值（data-URI 豁免）。
        if color_literal(line) && !line.contains("data:") {
            problems.push(format!(
                "UI-1 {FRONTEND_INDEX}:{} 颜色字面量（仅 data-URI 豁免）",
                number + 1
            ));
        }
    }

    // UI-6 契约锚点双向对账。
    let spec_text = read_text(&root.join(SPEC_MD)).unwrap_or_default();
    let spec_rules = rule_ids_in(&spec_text);
    let gate_rules: BTreeSet<String> = RULE_IDS.iter().map(|id| (*id).to_string()).collect();
    for expected in &gate_rules {
        if !spec_rules.contains(expected) {
            problems.push(format!(
                "UI-6 {SPEC_MD} 缺锚点 {expected}（契约与门禁必须同提交同步）"
            ));
        }
    }
    for found in &spec_rules {
        if !gate_rules.contains(found) {
            problems.push(format!(
                "UI-6 {SPEC_MD} 的锚点 {found} 没有对应的门禁规则（契约与门禁必须同提交同步）"
            ));
        }
    }

    report(
        "ui_style",
        problems,
        format!(
            "UI-1…UI-6 × {} 个源文件 + {FRONTEND_INDEX} + {SPEC_MD}",
            sources.len()
        ),
    );
}

/// 从契约文本里收集 `UI-n` 锚点（只认 `### UI-n 标题` 形态，正文引用不误伤）。
fn rule_ids_in(text: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("### ") else {
            continue;
        };
        let Some(rest) = rest.strip_prefix("UI-") else {
            continue;
        };
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            found.insert(format!("UI-{digits}"));
        }
    }
    found
}

#[test]
fn anchor_scanning_is_robust() {
    // 锚点收集只认标题形态。
    let sample = "### UI-1 色值\n见 UI-9 与 UI-2。\n### UI-3 深色";
    let ids = rule_ids_in(sample);
    assert!(
        ids.contains("UI-1") && ids.contains("UI-3") && !ids.contains("UI-9"),
        "{ids:?}"
    );
    // 色值判定不误伤 markdown 锚点与百分比。
    assert!(!color_literal("let x = width; // 50% #section"));
    assert!(color_literal("color: #ff0000;"));
    assert!(color_literal("bg = rgb(1, 2, 3)"));
    // onConfirm 不误判为原生弹窗。
    assert!(!native_dialog("onConfirm={() => close()}"));
    assert!(native_dialog("window.confirm('删除？')"));
    // data-theme 不含 dark:，不误判。
    assert!(!color_literal("data-theme=\"dark\""));
    // 任意值 px 命中与豁免。
    assert!(arbitrary_px("className=\"text-[13px]\""));
    assert!(!arbitrary_px("className=\"text-sm\""));
}

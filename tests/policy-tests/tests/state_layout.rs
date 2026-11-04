//! 状态目录布局门禁：**路径字面量只能出现在一处**。
//!
//! 为什么需要它：状态目录的布局（`state.json` / `events.jsonl` / `rules/` /
//! `logs/` / `trajectories/` / `shared/confdir/` …）决定了"东西落在哪儿"。
//! 只要**两份实现**，就会漂移 —— 改了一处忘了另一处，症状是"界面读的是 A、
//! 管理器写的是 B"，从外部几乎无法归因（文件在、路径对，就是读不出来）。
//!
//! 这条纪律在规范化之前是**零门禁**的：`state.rs` 派生一套，`ui_prefs.rs` 又
//! 自己拼了一套 `"ui.json"`，两处独立演化。本门禁把布局表钉在
//! `crates/manager/src/state.rs` 一处，其他任何文件都不得再自己拼路径。
//!
//! 判据（两条）：
//!
//! 1. **布局字面量只在拼路径的地方受管**：`.join("ui.json")` 这类**造路径**
//!    的写法只允许出现在 `state.rs`；而 `"rules"` 这种**领域字段名**
//!    （环境绑定的规则集名）到处都用，它不是路径 —— 判据必须区分这两者，
//!    否则门禁会红在毫无关系的地方（红在无关处，门禁就会被当成噪声忽略）。
//! 2. **布局表本身登记齐全** —— `state.rs` 的 `layout` 模块必须含有全部约定项，
//!    少一项即红（防止"新加一个目录忘了登记"，那等于给布局开了个没锁的口子）。
//!
//! 行内豁免：`state-layout-lint: allow`（与仓库既有的 `doc-scope-lint: allow`
//! 同一手法）。它留给**确实无法引用布局表**的地方 —— 例如引擎层（`engine`
//! 在依赖方向上**低于** `manager`，它不知道状态目录长什么样）。

mod common;

use common::{read_text, report, walk_relative, workspace_root};

/// 布局表的唯一合法出处。
const LAYOUT_OWNER: &str = "crates/manager/src/state.rs";

/// 状态目录里的**目录名**字面量。
///
/// 目录名比文件名危险：多一个或少一个就是另一个目录，而路径拼错**不会报错**
/// —— 它只是让那个目录凭空多出来（"日志怎么跑到这儿来了"）。
const DIR_LITERALS: &[&str] = &[
    "rules",
    "logs",
    "trajectories",
    "shared",
    "confdir",
    "agent",
];

/// 状态目录里的**文件名**字面量。
const FILE_LITERALS: &[&str] = &["state.json", "events.jsonl", "ui.json", "lock"];

/// 布局表必须登记齐全的项（缺一项即红）。
///
/// 这张表是"规范本身"：代码改了布局而忘了改这张表，门禁就会把新布局漏出去；
/// 反过来，门禁改了而代码没改，红的是代码 —— 两个方向都要有人管。
const REQUIRED_LAYOUT_KEYS: &[&str] = &[
    "STATE_FILE",
    "UI_PREFS_FILE",
    "EVENTS_FILE",
    "LOCK_FILE",
    "RULES_DIR",
    "LOGS_DIR",
    "TRAJECTORIES_DIR",
    "SHARED_DIR",
    "CONFDIR_DIR",
    "LEGACY_AGENT_DIR",
];

/// 行内豁免标记。
const ALLOW_MARKER: &str = "state-layout-lint: allow";

#[test]
fn the_state_layout_literals_live_in_exactly_one_place() {
    let root = workspace_root();
    let mut problems = Vec::new();
    let mut checked = 0usize;

    for file in walk_relative(&root) {
        if file == LAYOUT_OWNER || file.ends_with("state_layout.rs") {
            continue;
        }
        // 只扫 Rust 源码：布局字面量是代码事实，文档里描述布局是**应该**的
        // （README 的目录树、spec 的路径说明）。
        if !file.ends_with(".rs") {
            continue;
        }
        let Some(text) = read_text(&root.join(&file)) else {
            continue;
        };
        checked += 1;

        for (number, line) in text.lines().enumerate() {
            let code = line.trim_start();
            // 注释不是实现 —— 注释里说明布局是应该的（"轨迹在 trajectories 下"）。
            if code.starts_with("//") || code.contains(ALLOW_MARKER) {
                continue;
            }
            for literal in DIR_LITERALS.iter().chain(FILE_LITERALS.iter()) {
                if builds_a_path(code, literal) {
                    problems.push(format!(
                        "{file}:{}: 布局字面量 {literal:?} 只允许出现在 {LAYOUT_OWNER} —— \
                         路径规则有两份实现就必然漂移（这一行：{}）",
                        number + 1,
                        code.trim()
                    ));
                }
            }
        }
    }

    report(
        "state-layout",
        problems,
        format!("{checked} 个 Rust 文件已扫，布局字面量只存在于 {LAYOUT_OWNER}"),
    );
}

#[test]
fn the_layout_table_registers_every_agreed_entry() {
    let root = workspace_root();
    let owner = root.join(LAYOUT_OWNER);
    let text = read_text(&owner).unwrap_or_default();
    let mut problems = Vec::new();

    for key in REQUIRED_LAYOUT_KEYS {
        if !text.contains(&format!("pub const {key}")) {
            problems.push(format!(
                "{LAYOUT_OWNER}: 布局表缺少 {key} —— 布局项必须登记齐全"
            ));
        }
    }

    report(
        "state-layout-table",
        problems,
        format!("{} 个布局项登记齐全", REQUIRED_LAYOUT_KEYS.len()),
    );
}

/// 这一行是否在**造路径**时用到了这个字面量。
///
/// 判据不是"字符串出现了"而是"**它被拼进了路径**"：只看 `.join("…")` 这类
/// 路径构造。这样 `"rules"`（领域的字段名：环境绑定的规则集）不会被误判，
/// 而 `state_dir.join("rules")`（造目录）会被抓住 —— 前者是数据，后者是布局。
///
/// 做法：找每个 `.join(` 的调用，在它的括号里取**第一个**字符串字面量
/// （路径拼接的第一个成分就是目录/文件名本身），比对是否命中。
fn builds_a_path(code_line: &str, literal: &str) -> bool {
    let mut rest = code_line;
    while let Some(start) = rest.find(".join(") {
        let after = &rest[start + ".join(".len()..];
        let Some(end) = after.find(')') else {
            return false;
        };
        let argument = &after[..end];
        if first_string_literal(argument) == Some(literal) {
            return true;
        }
        rest = &after[end + 1..];
    }
    false
}

/// 取一段文本里**第一个**字符串字面量的内容。
fn first_string_literal(text: &str) -> Option<&str> {
    let start = text.find('"')?;
    let after = &text[start + 1..];
    let end = after.find('"')?;
    Some(&after[..end])
}

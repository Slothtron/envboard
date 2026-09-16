//! 宿主适配器门禁（静态部分）：**适配器角落必须仍是"单文件、纯标准库"的形态**。
//!
//! 适配器是仓库里唯一的非 Rust 角落（宿主 mitmproxy 用 `-s` 加载这个脚本），所以它的
//! 形态需要被守住 —— 一旦它开始 import 本仓其他东西、或长成多文件包，那个边界就没了。
//!
//! 断言：
//!
//! 1. `adapters/mitmproxy/` 下**恰好一个** `.py`，且名字是约定的那个；
//! 2. 它的 import 只允许标准库白名单 + `mitmproxy`（`envboard` / `mitmproxy_rs` 这类
//!    会直接失败，消息里点出模块名）；
//! 3. 没有 `from __future__ import annotations`（模块文档里记着为什么：`-s` 加载时
//!    模块不进 `sys.modules`，`@dataclass` 处理字符串注解会崩）；
//! 4. 存在 `--render --fixture` 入口（跨语言对拍靠它，见 S3 的 `dual`）。
//!
//! **语法错误**不在这里判：Rust 解析不了 Python，而 `dual` 会真的加载这个脚本，
//! 语法错误在那一步就暴露（原先的 Python 语法门禁同样需要解释器，依赖面没有变化）。

mod common;

use common::{read_text, report, walk_relative, workspace_root};

const ADAPTER_DIR: &str = "adapters/mitmproxy";
const INJECTOR_NAME: &str = "envboard_mitmproxy.py";

/// 允许 import 的模块：标准库白名单（本脚本只用这些）+ 宿主。
const ALLOWED_IMPORTS: &[&str] = &[
    "argparse",
    "dataclasses",
    "ipaddress",
    "json",
    "os",
    "re",
    "sys",
    "threading",
    "time",
    "typing",
    "mitmproxy",
];

/// 被点名的越界模块（出现在这里时错误消息更直白）。
const FORBIDDEN_IMPORTS: &[&str] = &["envboard", "mitmproxy_rs"];

/// 去掉行尾注释。`#` 在字符串字面量里不算注释（这个脚本里有 `"# envboard rules file"`）。
fn strip_comment(line: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    for (index, c) in line.char_indices() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single && !in_double => return &line[..index],
            _ => {}
        }
    }
    line
}

/// 取出一行的 import 根模块名（`import a.b` / `from a.b import c` 都算 `a`）。
fn imported_module(line: &str) -> Option<String> {
    let code = strip_comment(line).trim();
    let rest = code
        .strip_prefix("import ")
        .or_else(|| code.strip_prefix("from "))?;
    let module = rest
        .split(|c: char| c.is_whitespace() || c == ',')
        .next()
        .unwrap_or("")
        .trim();
    let root = module.split('.').next().unwrap_or("").to_string();
    if root.is_empty() { None } else { Some(root) }
}

#[test]
fn the_adapter_stays_a_single_stdlib_script() {
    let root = workspace_root();
    let mut problems = Vec::new();

    // 1. 恰好一个脚本，且是约定的那个。
    let scripts: Vec<String> = walk_relative(&root)
        .into_iter()
        .filter(|file| file.starts_with(ADAPTER_DIR) && file.ends_with(".py"))
        .collect();
    if scripts != vec![format!("{ADAPTER_DIR}/{INJECTOR_NAME}")] {
        problems.push(format!(
            "{ADAPTER_DIR}/ 下必须恰好一个 `{INJECTOR_NAME}`（宿主用 -s 加载的单文件产品代码）；\
             实际：{scripts:?}"
        ));
        report("adapter", problems, String::new());
        return;
    }

    let injector = format!("{ADAPTER_DIR}/{INJECTOR_NAME}");
    let Some(text) = read_text(&root.join(&injector)) else {
        report("adapter", vec![format!("{injector} 读不到")], String::new());
        return;
    };

    // 2. import 白名单。
    for (number, line) in text.lines().enumerate() {
        let Some(module) = imported_module(line) else {
            continue;
        };
        if FORBIDDEN_IMPORTS.contains(&module.as_str()) {
            problems.push(format!(
                "{injector}:{}: 不许 import {module}（适配器必须自包含：它被内嵌进二进制）",
                number + 1
            ));
        } else if !ALLOWED_IMPORTS.contains(&module.as_str()) {
            problems.push(format!(
                "{injector}:{}: 只允许标准库白名单 + mitmproxy，出现了 {module:?}",
                number + 1
            ));
        }

        // 3. `from __future__ import annotations` 会让 `-s` 加载直接崩。
        let code = strip_comment(line).trim();
        if code.starts_with("from __future__ import") && code.contains("annotations") {
            problems.push(format!(
                "{injector}:{}: 不许 `from __future__ import annotations` —— mitmdump 用 -s \
                 加载时模块不进 sys.modules，`@dataclass` 处理字符串注解会崩",
                number + 1
            ));
        }
    }

    // 4. 跨语言对拍的入口。
    for flag in ["--render", "--fixture"] {
        if !text.contains(flag) {
            problems.push(format!("{injector}: 缺 `{flag}` 入口（跨语言对拍靠它）"));
        }
    }

    report(
        "adapter",
        problems,
        format!("{injector}：单文件、import 只用白名单、`--render --fixture` 入口在位"),
    );
}

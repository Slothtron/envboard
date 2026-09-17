//! 跨语言对拍用的一次性入口：读一个规则 fixture，把规范化文本打到 stdout。
//!
//! 存在的理由：Rust 与 Python 有两份 hosts 解析实现（刻意的重复，见
//! `core/spec/capabilities.md`「多语言实现的仲裁规则」），必须能对同一批 fixture
//! **逐字节**比较输出，而"起两个代理比流量"既慢又脆。
//!
//! 用法（`ci/verify.sh` 的 contract-dual 步骤就是这么调的）：
//!
//! ```text
//! cargo run -q -p envboard-rules --bin render_fixture -- <fixture.json>
//! ```
//!
//! 这个入口是规则解析/渲染的唯一实现：golden fixture 的期望正文由它重算，契约测试
//! 逐例消费（v2 的"与注入器 `--render` 逐字节对拍"随第二份实现一起退场）。

use std::collections::BTreeMap;
use std::path::PathBuf;

use envboard_rules::{parse_hosts_text, render_rules};

fn main() -> std::process::ExitCode {
    let Some(path) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: render_fixture <fixture.json>");
        return std::process::ExitCode::from(2);
    };

    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) => {
            eprintln!("cannot read {}: {error}", path.display());
            return std::process::ExitCode::from(2);
        }
    };
    let fixture: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{}: invalid JSON: {error}", path.display());
            return std::process::ExitCode::from(2);
        }
    };

    let text = fixture
        .get("input")
        .and_then(|input| input.get("text"))
        .and_then(|text| text.as_str())
        .unwrap_or_default();
    let source = fixture
        .get("source")
        .and_then(|source| source.as_str())
        .unwrap_or_default();

    let parsed = parse_hosts_text(text);
    // entries 需要规范化后的键；渲染器要求"传入前已归一化"，这里如实照做。
    let entries: BTreeMap<String, String> = parsed.entries.clone();
    match render_rules(&entries, source) {
        Ok(rendered) => {
            print!("{rendered}");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("render failed: {}", error.message);
            std::process::ExitCode::from(1)
        }
    }
}

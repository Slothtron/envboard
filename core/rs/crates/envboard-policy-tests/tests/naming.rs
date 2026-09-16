//! 命名门禁：**发包标识不得进入代码身份**。
//!
//! 本包不向任何 registry 发布 Python 分发物（Python 这一侧只剩一个被二进制内嵌的
//! 单文件注入器），所以规则是简单的一条：
//!
//! | 规则 | 内容 |
//! |---|---|
//! | 路径 | 任何目录名 / 文件名都不得含 `TOKEN` |
//! | 代码内容 | 代码与配置后缀的文件里不得出现 `TOKEN`，唯一例外是**本判据自身**（它必须写出这个 token 才能检查它） |
//! | 文档 | `.md` 不扫描 —— 文档需要能够命名这个约定 |
//!
//! 保留它是因为：将来任何新宿主适配器都可能重新引入"分发身份"，而"代码身份不得含
//! 发包标识"这条纪律与仓库形态无关 —— 代码身份属于代码，发布身份属于 manifest。

mod common;

use common::{read_text, report, walk_all_relative, walk_relative, workspace_root};

const TOKEN: &str = "slothtron";

/// 允许在**代码内容**里出现 [`TOKEN`] 的文件。只有本判据自身。
const CONTENT_ALLOWLIST: &[&str] = &["core/rs/crates/envboard-policy-tests/tests/naming.rs"];

/// 会被扫描内容的代码 / 配置后缀。`.md` 刻意不在其中（见模块文档）。
const CODE_SUFFIXES: &[&str] = &[
    ".py", ".sh", ".bash", ".rs", ".js", ".mjs", ".css", ".html", ".json", ".toml", ".yml", ".yaml",
];

fn is_code_file(path: &str) -> bool {
    CODE_SUFFIXES.iter().any(|suffix| path.ends_with(suffix))
}

fn scan_paths(paths: &[String]) -> Vec<String> {
    let mut problems = Vec::new();
    for path in paths {
        let name = path.rsplit('/').next().unwrap_or(path);
        if name.contains(TOKEN) {
            problems.push(format!(
                "{path}: 路径成分含 {TOKEN:?}（它属于 manifest，永远不属于路径）"
            ));
        }
    }
    problems
}

fn scan_contents(root: &std::path::Path, files: &[String]) -> Vec<String> {
    let mut problems = Vec::new();
    for file in files.iter().filter(|file| is_code_file(file)) {
        if CONTENT_ALLOWLIST.contains(&file.as_str()) {
            continue;
        }
        let Some(text) = read_text(&root.join(file)) else {
            continue;
        };
        for (number, line) in text.lines().enumerate() {
            if line.contains(TOKEN) {
                problems.push(format!(
                    "{file}:{}: 代码里出现了 {TOKEN:?}；只有 {CONTENT_ALLOWLIST:?} 可以提它",
                    number + 1
                ));
            }
        }
    }
    problems
}

#[test]
fn no_identity_token_leaks_into_code() {
    let root = workspace_root();
    let all = walk_all_relative(&root);
    let files = walk_relative(&root);

    let mut problems = scan_paths(&all);
    problems.extend(scan_contents(&root, &files));

    let code_files = files.iter().filter(|file| is_code_file(file)).count();
    report(
        "naming",
        problems,
        format!(
            "{} 条路径已扫、{code_files} 个代码文件干净；{TOKEN:?} 只允许出现在 {CONTENT_ALLOWLIST:?}",
            all.len()
        ),
    );
}

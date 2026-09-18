//! 门禁共用的仓库遍历与输出。被各 `tests/<gate>.rs` 以 `mod common;` 引入。
//!
//! 只做只读的文件分析：门禁不改仓库里的任何东西，因此可以任意重复执行。
//!
//! `dead_code` 必须允许：每个测试目标都会**各自编译一份**这个模块，于是只被某一个
//! 门禁用到的助手，在别的门禁眼里就是死代码 —— 而 `clippy -D warnings` 会把它当错误。
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

/// 不进扫描的目录：构建产物、VCS 元数据、依赖树、本机工作材料。
///
/// 本机工作材料目录在这里是**必需**的 —— 它不入库（见 `.gitignore`），里面放的是设计、
/// 计划、验收转录这类草稿，让它们参与门禁等于用草稿判仓库红。
/// `node_modules/` / `__pycache__/` **不在**这份名单里：它们的存在本身就是
/// 工具链收敛门禁要抓的事实，用 [`find_dirs`] 单独判定（内容不扫，只判存在性）。
pub const SKIP_DIRS: &[&str] = &[
    "target",
    ".git",
    "node_modules",
    "__pycache__",
    ".zvec-grep",
    ".agents",
];

/// 找目录时也跳过的：这三者要么太大、要么不是本仓材料。
const DIR_SEARCH_SKIP: &[&str] = &[".git", "target", ".agents", ".zvec-grep"];

/// 仓库根。`CARGO_MANIFEST_DIR` 指向 `tests/<crate>`，向上两级。
pub fn workspace_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .join("..")
        .join("..")
        .canonicalize()
        .unwrap_or_else(|error| panic!("cannot resolve the repo root from {manifest:?}: {error}"))
}

/// 仓库内的相对路径，统一用 `/` 分隔（判据与输出都不依赖平台分隔符）。
pub fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// 递归收集文件（跳过 [`SKIP_DIRS`]），按路径排序以保证输出稳定。
pub fn walk(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            let path = entry.path();
            if file_type.is_dir() {
                if SKIP_DIRS.contains(&entry.file_name().to_string_lossy().as_ref()) {
                    continue;
                }
                stack.push(path);
            } else if file_type.is_file() {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// [`walk`] 的相对路径版本。
pub fn walk_relative(root: &Path) -> Vec<String> {
    walk(root).iter().map(|path| relative(root, path)).collect()
}

/// 递归收集**文件与目录**的相对路径（同样跳过 [`SKIP_DIRS`]）。
///
/// 按路径成分判定的门禁（如"任何目录名 / 文件名都不得含发包标识"）需要看到目录本身，
/// 只看文件会漏掉空目录。
pub fn walk_all_relative(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            let path = entry.path();
            if file_type.is_dir() {
                if SKIP_DIRS.contains(&entry.file_name().to_string_lossy().as_ref()) {
                    continue;
                }
                found.push(relative(root, &path));
                stack.push(path);
            } else if file_type.is_file() {
                found.push(relative(root, &path));
            }
        }
    }
    found.sort();
    found
}

/// 找指定名字的**目录**（找到即记录、不再下钻：存在性才是证据，内容不是）。
///
/// 用于 `node_modules/`、`__pycache__/` 这类"本身违规、但内容不该被扫描"的目录 ——
/// 它们被 [`SKIP_DIRS`] 挡在 `walk` 之外，所以必须单独看一眼。
pub fn find_dirs(root: &Path, names: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if names.contains(&name.as_str()) {
                found.push(relative(root, &path));
                continue;
            }
            if DIR_SEARCH_SKIP.contains(&name.as_str()) {
                continue;
            }
            stack.push(path);
        }
    }
    found.sort();
    found
}

/// 读文本；非 UTF-8（图片、二进制）与读不到的返回 `None`，由调用方跳过。
pub fn read_text(path: &Path) -> Option<String> {
    String::from_utf8(fs::read(path).ok()?).ok()
}

/// 门禁的统一出口：没问题打印一行 OK，有问题打完整清单再 panic。
///
/// panic 是刻意的 —— 门禁是一棵 `#[test]`，红就是断言失败；问题清单用
/// `cargo test ... -- --nocapture` 能看到全量（默认只显示失败测试的输出）。
pub fn report(gate: &str, problems: Vec<String>, summary: String) {
    if problems.is_empty() {
        println!("{gate} OK ({summary})");
        return;
    }
    println!("{gate} FAILED ({} problem(s)):", problems.len());
    for problem in &problems {
        println!("  {problem}");
    }
    panic!("{gate} FAILED ({} problem(s))", problems.len());
}

/// 这一行里是否出现了 `word`（按标识符边界判断，避免 `node` 命中 `node_modules`）。
pub fn has_word(line: &str, word: &str) -> bool {
    let mut start = 0;
    while let Some(offset) = line[start..].find(word) {
        let begin = start + offset;
        let end = begin + word.len();
        let before_ok = line[..begin]
            .chars()
            .next_back()
            .is_none_or(|c| !is_word_char(c));
        let after_ok = line[end..].chars().next().is_none_or(|c| !is_word_char(c));
        if before_ok && after_ok {
            return true;
        }
        start = end;
    }
    false
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

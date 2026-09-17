//! 包内文本自包含门禁：**入库文本不得引用不在仓库里的文档**。
//!
//! 为什么需要它：对 clone 本仓的人，指向一份不在仓库里的文档的路径不存在、章节号
//! 无处可查，那条线索读到就断。规范与契约必须"只看本仓就能完整理解"。
//!
//! 本仓的入库文档是 `README.md` / `CHANGELOG.md` / `core/spec/**` / `docs/**` 与各 crate；
//! 设计、计划、实机验收等工作材料放在包内一个**不入库**的目录里（见 `.gitignore`）。
//!
//! 判据（三条，都可机械验证）：
//!
//! 1. **禁用词零命中**：工作区规范文件名、以及那个工作材料目录的名字 —— 后者不入库，
//!    入库文本指向它对读者不可解析；
//! 2. **指向本仓的路径必须存在**（写错的、或搬过来时漏了的一律失败）；
//! 3. **`§` 引用必须带本仓锚点**（判据就在本文件）：任何含 `§` 的行都要同时给出
//!    本仓内的文件（`core/spec/…`、`capabilities.md`、`README.md` …）或写"本文件" ——
//!    裸的章节号离开它出处的那份文档就无从解析（本文件这条判据正是为此）。
//!
//! 不在禁止之列的（可复核的外部事实坐标）：宿主 / 上游源码坐标、实测命令与输出、
//! 版本号、协议编号、外部 URL。

mod common;

use common::{read_text, report, walk_relative, workspace_root};

/// 未入库的本地文档的指称 —— 零命中（例外见 [`LOCAL_ONLY_TOKEN_ALLOWED`]）。
const BANNED_TOKENS: &[&str] = &[
    "plugin-repo-architecture",
    "dsh-plugin-dev-SOP",
    "Octop插件开发SOP",
    // 工作区规范文件在包仓库之外，按文件名指称它同样不可解析
    "AGENTS.md",
];

/// 工作材料目录的名字：它不入库，所以入库文本指向它 = 死指针。
const LOCAL_ONLY_TOKEN: &str = ".agents/";

/// 例外：面向人的发布文档 —— 约定要在仓库里能被发现，否则下一个人会把设计文档
/// 重新写回 `docs/`；以及 `.gitignore`，它必须**写出**这个目录名才能忽略它。
/// 与"分发标识只许出现在面向人的发布文档"是同一手法。
const LOCAL_ONLY_TOKEN_ALLOWED: &[&str] = &["README.md", "CHANGELOG.md", ".gitignore"];

/// 会被扫描的文件后缀。`.service` / `.conf` / `.env` 也算：部署工件里的注释同样是
/// "面向读者的文本"。
const SUFFIXES: &[&str] = &[
    ".md", ".rs", ".py", ".js", ".css", ".html", ".toml", ".sh", ".yml", ".yaml", ".json",
    ".service", ".conf", ".env",
];

/// **无后缀**的文本文件要单独列出来 —— 按后缀筛会把它们整类漏掉。
const EXTRA_FILES: &[&str] = &[".gitignore", ".dockerignore"];

/// 本判据自己的源码：它必须写出这些禁用词、路径前缀与那个目录名才能检查它们，
/// 所以整文件跳过（与它判定别的文件时的口径一致）。
///
/// 这是**已知的豁免面**：门禁源码本身不受这条判据约束，由代码评审与工具链门禁
/// （它们照旧扫这些文件）兜住。
const SELF_SOURCES: &[&str] = &["core/rs/crates/envboard-policy-tests/tests/doc_scope.rs"];

/// 指向本仓的路径引用，用来判"必须存在"。前缀与允许的后缀。
const PATH_PREFIXES: &[&str] = &[
    "core/spec/",
    "core/rs/",
    "adapters/",
    "platforms/",
    "scripts/",
    "ci/",
    "docs/",
];
const PATH_SUFFIXES: &[&str] = &[
    ".md", ".rs", ".py", ".json", ".js", ".css", ".html", ".toml", ".sh",
];

/// 包根下的裸文件名引用也要判存在性。
const BARE_FILES: &[&str] = &["README.md", "CHANGELOG.md", "LICENSE"];

/// **历史记录**豁免"路径必须存在"这一条：CHANGELOG 的历史条目本来就要提到当时存在、
/// 现在已经删掉的文件。它们是记录，不是指向别处的线索。
const HISTORICAL: &[&str] = &["CHANGELOG.md"];

/// 行内豁免标记：路径**故意**不存在（如"断言它已被删除"的清单）。
const ALLOW_MARKER: &str = "doc-scope-lint: allow";

fn is_text_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    SUFFIXES.iter().any(|suffix| path.ends_with(suffix)) || EXTRA_FILES.contains(&name)
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '/' || c == '-'
}

/// 取出这一行里所有"指向本仓的路径引用"。
///
/// 做法刻意不用正则回溯 / 前瞻：先按前缀定位，再一路贪婪吃掉路径字符，最后用
/// `ends_with` 判后缀。这样 `x/normalization.json` 不会被切成 `x/normalization.js`
/// —— 它是 `ends_with(".json")` 的文件，压根不以 `.js` 结尾。
fn path_references(line: &str) -> Vec<String> {
    let mut found = Vec::new();
    for prefix in PATH_PREFIXES {
        let mut from = 0;
        while let Some(offset) = line[from..].find(prefix) {
            let start = from + offset;
            let rest = &line[start..];
            let end = rest
                .char_indices()
                .take_while(|(_, c)| is_word_char(*c))
                .last()
                .map(|(index, c)| index + c.len_utf8())
                .unwrap_or(0);
            let candidate = &rest[..end];
            if PATH_SUFFIXES
                .iter()
                .any(|suffix| candidate.ends_with(suffix))
            {
                found.push(candidate.to_string());
            }
            from = start + prefix.len();
        }
    }
    for bare in BARE_FILES {
        let mut from = 0;
        while let Some(offset) = line[from..].find(bare) {
            let start = from + offset;
            let before_ok = line[..start]
                .chars()
                .next_back()
                .is_none_or(|c| !is_word_char(c));
            let after_ok = line[start + bare.len()..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_ascii_alphanumeric());
            if before_ok && after_ok {
                found.push((*bare).to_string());
            }
            from = start + bare.len();
        }
    }
    found
}

/// 含 `§` 的行必须出现下列之一（本仓锚点；判据见本文件的模块文档）。
fn has_anchor(line: &str) -> bool {
    line.contains("core/spec/")
        || line.contains("docs/")
        || line.contains("README.md")
        || line.contains("CHANGELOG.md")
        || line.contains("capabilities.md")
        || line.contains("rules.md")
        || line.contains("errors.md")
        || line.contains("fixtures/")
        || line.contains("本文件")
}

#[test]
fn in_repo_text_is_self_contained() {
    let root = workspace_root();
    let mut problems = Vec::new();
    let files = walk_relative(&root)
        .into_iter()
        .filter(|file| is_text_file(file))
        .collect::<Vec<_>>();

    for file in &files {
        if SELF_SOURCES.contains(&file.as_str()) {
            continue;
        }
        let Some(text) = read_text(&root.join(file)) else {
            continue;
        };
        for (number, line) in text.lines().enumerate() {
            let number = number + 1;

            for token in BANNED_TOKENS {
                if line.contains(token) {
                    problems.push(format!(
                        "{file}:{number}: 引用未入库的本地文档（禁用词 {token:?}）：{}",
                        line.trim()
                    ));
                }
            }
            if line.contains(LOCAL_ONLY_TOKEN) && !LOCAL_ONLY_TOKEN_ALLOWED.contains(&file.as_str())
            {
                problems.push(format!(
                    "{file}:{number}: 指向未入库的工作材料目录 —— 对 clone 本仓的人那是死指针：{}",
                    line.trim()
                ));
            }

            if line.contains('§') && !has_anchor(line) {
                problems.push(format!(
                    "{file}:{number}: `§` 引用没有本仓锚点（要写清是哪份仓库内文件，或写「本文件」）：{}",
                    line.trim()
                ));
            }

            if HISTORICAL.contains(&file.as_str()) || line.contains(ALLOW_MARKER) {
                continue;
            }
            for candidate in path_references(line) {
                if !root.join(&candidate).exists() {
                    problems.push(format!(
                        "{file}:{number}: 指向的仓库内路径不存在：{candidate}"
                    ));
                }
            }
        }
    }

    report(
        "doc-scope",
        problems,
        format!(
            "{} 个文本文件：禁用词零命中、路径引用都真实存在、`§` 都带本仓锚点",
            files.len()
        ),
    );
}

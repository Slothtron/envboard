//! 工具链收敛门禁：**除 `frontend/` 边界外，仓库里不许有第二种语言工具链的痕迹**。
//!
//! v4 修订（前端工程化）：Node/TS/Vite/pnpm 从「全仓禁止」改为「边界内允许」——
//! 第二种工具链被**圈禁**在 `frontend/` 目录里，而不是被引入仓库主干：
//!
//! - **文件面** R1：Python 面维持全仓禁止（前端工程不需要 Python）；
//! - **文件面** R2：Node/TS 面按前缀判定 —— `frontend/` 内允许清单文件、TS 源码、
//!   打包器配置与 `node_modules`；边界之外任何一处出现即红；
//! - **调用面** R3：可执行面（ci/ scripts/ .service）仍然只许 cargo ——
//!   **默认验证路径保持零 npm/pnpm/node**，前端构建是显式的 frontend 层动作；
//! - **成对判定**：`frontend/src` 与 `frontend/dist` 同存同缺 —— dist 是内嵌进
//!   二进制的构建产物（提交入库），缺一个就是漂移。
//!
//! 关键区分不变：**禁的是工具链越界，不是文件类型**。`crates/web/assets/` 的
//! 历史三件套已退场；浏览器资产现在是 `frontend/dist` 的构建产物（数据）。
//!
//! 迁移期由 [`PENDING`] 承载"待退场"的文件，**双向判定**：白名单条目失效同样失败。

mod common;

use common::{find_dirs, has_word, read_text, report, walk_relative, workspace_root};

/// 迁移白名单：待退场的旧工具链文件。**当前为空**。
const PENDING: &[&str] = &[];

/// 前端边界：第二种工具链唯一合法的容身处。
const FRONTEND_DIR: &str = "frontend";

/// Python 工具链的清单文件 —— 哪里都不许有。
const PYTHON_MANIFESTS: &[&str] = &[
    "pyproject.toml",
    "setup.cfg",
    "Pipfile",
    "Pipfile.lock",
    "poetry.lock",
    "uv.lock",
    "tox.ini",
    "mypy.ini",
    ".python-version",
];

/// Node/TS/pnpm/Vite 工具链的清单与配置 —— 只允许出现在 `frontend/` 下。
const NODE_MANIFESTS: &[&str] = &[
    "package.json",
    "package-lock.json",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "yarn.lock",
    "bun.lockb",
    ".npmrc",
];

/// 打包器 / 编译器配置文件名前缀 —— 同上，只许进 `frontend/`。
const BUILDER_CONFIG_PREFIXES: &[&str] = &[
    "tsconfig",
    "vite.config.",
    "tsdown.config.",
    "esbuild",
    "rollup.config.",
    "webpack.config.",
];

/// TS 源码后缀 —— 只许出现在 `frontend/` 下（本仓没有别处需要 TS）。
const TS_SUFFIXES: &[&str] = &[".ts", ".tsx", ".mts", ".cts"];

/// 可执行面里禁止出现的命令（整词匹配）。`cargo` 与宿主二进制不在此列。
/// 前端构建工具（pnpm/npm/vite/node）全部在内：默认绿路径 cargo-only。
const FORBIDDEN_COMMANDS: &[&str] = &[
    "npm", "npx", "pnpm", "yarn", "bun", "node", "deno", "vite", "pip", "pip3", "pipx", "poetry",
    "uvx", "mypy", "pytest", "ruff", "black", "flake8", "isort", "tox",
];

/// 禁用的短语形态（工具名 + 子命令）。
const FORBIDDEN_PHRASES: &[&str] = &["uv run", "uv pip", "uv sync"];

struct Scan {
    files: Vec<String>,
    python: Vec<String>,
    node: Vec<String>,
    call: Vec<String>,
    /// 本次真的被 `PENDING` 挡下的条目 —— 用于判白名单是否过期。
    used: Vec<String>,
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// 路径是否落在前端边界内（`frontend/…`）。
fn inside_frontend(path: &str) -> bool {
    path == FRONTEND_DIR || path.starts_with(&format!("{FRONTEND_DIR}/"))
}

fn scan() -> Scan {
    let root = workspace_root();
    let files = walk_relative(&root);
    let mut used = Vec::new();

    let python = python_face(&root, &files, &mut used);
    let node = node_face(&root, &files, &mut used);
    let call = call_face(&root, &files, &mut used);
    Scan {
        files,
        python,
        node,
        call,
        used,
    }
}

/// R1：Python 面（全仓禁止，不变）。
fn python_face(root: &std::path::Path, files: &[String], used: &mut Vec<String>) -> Vec<String> {
    let mut problems = Vec::new();
    for file in files {
        let name = basename(file);

        if name.ends_with(".py") || name.ends_with(".pyi") {
            if PENDING.contains(&file.as_str()) {
                used.push(file.clone());
            } else {
                problems.push(format!(
                    "{file}: 工具链不许用 Python 写（v3：仓库里没有非 Rust 产品代码）"
                ));
            }
        }

        if PYTHON_MANIFESTS.contains(&name)
            || (name.starts_with("requirements") && name.ends_with(".txt"))
        {
            problems.push(format!("{file}: 不许有 Python 工具链的清单文件"));
        }
    }

    for directory in find_dirs(root, &["__pycache__"]) {
        problems.push(format!(
            "{directory}: 不许有 Python 字节码缓存目录（工具链跑过 Python 的痕迹）"
        ));
    }
    problems
}

/// R2：Node / TS 面 —— 边界判定。
fn node_face(root: &std::path::Path, files: &[String], used: &mut Vec<String>) -> Vec<String> {
    let mut problems = Vec::new();
    for file in files {
        let name = basename(file);
        let in_frontend = inside_frontend(file);

        if name == "package.json" {
            if in_frontend {
                if PENDING.contains(&file.as_str()) {
                    used.push(file.clone());
                }
                continue;
            }
            if PENDING.contains(&file.as_str()) {
                used.push(file.clone());
            } else {
                problems.push(format!(
                    "{file}: Node 清单文件只允许出现在 frontend/ 边界内"
                ));
            }
            continue;
        }

        if !in_frontend {
            if NODE_MANIFESTS.contains(&name) {
                problems.push(format!(
                    "{file}: Node 工具链清单只允许出现在 frontend/ 边界内"
                ));
            }
            if BUILDER_CONFIG_PREFIXES.iter().any(|p| name.starts_with(p)) {
                problems.push(format!(
                    "{file}: 前端打包器配置只允许出现在 frontend/ 边界内"
                ));
            }
            if TS_SUFFIXES.iter().any(|suffix| name.ends_with(suffix)) {
                problems.push(format!(
                    "{file}: TypeScript 源码只允许出现在 frontend/ 边界内"
                ));
            }
        }
    }

    // node_modules / pnpm store：只许在 frontend/ 下存在（.gitignore 之外再判一道）。
    for directory in find_dirs(root, &["node_modules", ".pnpm-store"]) {
        if !inside_frontend(&directory) {
            problems.push(format!(
                "{directory}: 依赖树目录只允许出现在 frontend/ 边界内"
            ));
        }
    }
    problems
}

/// 成对判定：frontend/src ↔ frontend/dist（dist 是内嵌源，必须提交）。
fn frontend_pair_is_consistent() {
    let root = workspace_root();
    let src = root.join(FRONTEND_DIR).join("src");
    let dist = root.join(FRONTEND_DIR).join("dist");
    let src_exists = src.is_dir();
    let dist_exists = dist.is_dir();
    let problems: Vec<String> = if src_exists == dist_exists {
        Vec::new()
    } else if src_exists {
        vec!["frontend/dist: 有源码没有构建产物 —— 内嵌源缺失（pnpm build 后提交）".into()]
    } else {
        vec!["frontend/dist: 有构建产物没有源码 —— 产物失去可重建性".into()]
    };
    report(
        "toolchain/frontend-pair",
        problems,
        format!("frontend/src 与 frontend/dist 成对（src={src_exists}, dist={dist_exists}）"),
    );
}

/// 可执行面：会被 CI 或部署直接执行的文本。文档与注释不在此列。
fn is_executable_surface(file: &str) -> bool {
    (file.starts_with("ci/") && file.ends_with(".sh"))
        || (file.starts_with("scripts/") && file.ends_with(".sh"))
        || file.ends_with(".service")
        || file == ".gitlab-ci.yml"
        || file.starts_with(".gitlab/")
        || (file.starts_with(".github/workflows/")
            && (file.ends_with(".yml") || file.ends_with(".yaml")))
}

/// 从一层 shell 单词里取出"指向本仓脚本"的路径；不是脚本引用就返回 `None`。
fn script_reference(token: &str) -> Option<String> {
    let cleaned =
        token.trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '(' | ')' | ';' | '{' | '}'));
    let cleaned = cleaned
        .strip_prefix("$ROOT/")
        .or_else(|| cleaned.strip_prefix("${ROOT}/"))
        .unwrap_or(cleaned);
    let cleaned = cleaned.strip_prefix("./").unwrap_or(cleaned);
    if cleaned.ends_with(".py") && cleaned.contains('/') {
        Some(cleaned.to_string())
    } else {
        None
    }
}

/// R3：调用面（可执行面只许 cargo）。
fn call_face(root: &std::path::Path, files: &[String], used: &mut Vec<String>) -> Vec<String> {
    let mut problems = Vec::new();
    for file in files.iter().filter(|file| is_executable_surface(file)) {
        let Some(text) = read_text(&root.join(file)) else {
            continue;
        };
        for (number, line) in text.lines().enumerate() {
            let number = number + 1;
            let words: Vec<&str> = line.split_whitespace().collect();

            for command in FORBIDDEN_COMMANDS {
                if has_word(line, command) {
                    problems.push(format!(
                        "{file}:{number}: 可执行面里不许调用 `{command}`（前端构建是显式 frontend 层动作，不进默认路径）"
                    ));
                }
            }
            for phrase in FORBIDDEN_PHRASES {
                if line.contains(phrase) {
                    problems.push(format!("{file}:{number}: 可执行面里不许出现 `{phrase}`"));
                }
            }
            for pair in words.windows(2) {
                if pair[0].contains("python") && pair[1].starts_with("-m") {
                    problems.push(format!("{file}:{number}: 可执行面不许调用 Python 工具"));
                }
            }
            for word in &words {
                let Some(script) = script_reference(word) else {
                    continue;
                };
                if PENDING.contains(&script.as_str()) {
                    used.push(script);
                } else {
                    problems.push(format!(
                        "{file}:{number}: 可执行面调用了仓库内的 Python 脚本 `{script}`"
                    ));
                }
            }
        }
    }
    problems
}

#[test]
fn r1_no_python_toolchain_in_the_tree() {
    let scan = scan();
    report(
        "toolchain/r1-python-face",
        scan.python,
        format!("{} files scanned, 禁用清单零命中", scan.files.len()),
    );
}

#[test]
fn r2_node_toolchain_stays_inside_the_frontend_boundary() {
    let scan = scan();
    report(
        "toolchain/r2-node-face",
        scan.node,
        format!(
            "{} files scanned, Node/TS 痕迹全部在 frontend/ 边界内",
            scan.files.len()
        ),
    );
}

#[test]
fn r3_executable_surface_never_calls_a_second_toolchain() {
    let scan = scan();
    report(
        "toolchain/r3-call-face",
        scan.call,
        "可执行面只用 cargo".to_string(),
    );
}

#[test]
fn frontend_source_and_dist_are_a_pair() {
    frontend_pair_is_consistent();
}

#[test]
fn the_migration_whitelist_has_no_stale_entries() {
    let scan = scan();
    let mut stale: Vec<String> = PENDING
        .iter()
        .filter(|entry| !scan.used.contains(&entry.to_string()))
        .map(|entry| (*entry).to_string())
        .collect();
    stale.sort();
    let problems = stale
        .into_iter()
        .map(|entry| {
            format!("{entry}: 白名单条目已失效（文件已退场或已不违规）—— 请从 PENDING 删掉它")
        })
        .collect::<Vec<_>>();
    report(
        "toolchain/whitelist",
        problems,
        format!("迁移白名单 {} 条全部有效（清空即收敛完成）", PENDING.len()),
    );
}

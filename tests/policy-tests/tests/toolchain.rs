//! 工具链收敛门禁：**工具链各居其位，不许蔓延**。
//!
//! v5 修订（CI 编排跨平台化）：CI 编排从 bash 改为 Python（仅标准库，跨 Linux /
//! Windows 原生），Python 因此从「全仓禁止」改为「**圈禁**在 `scripts/` 的编排位」；
//! Node/TS/Vite/pnpm 维持圈禁在 `frontend/`。三个语言面：
//!
//! - **文件面** R1：`.py` 只许出现在 `scripts/`（CI 编排位）；产品代码仍禁 Python；
//!   Python 工具链的清单文件哪里都不许有；
//! - **文件面** R2：Node/TS 面按前缀判定 —— `frontend/` 内允许清单文件、TS 源码、
//!   打包器配置与 `node_modules`；边界之外任何一处出现即红；
//! - **调用面** R3：可执行面（`scripts/` `mise.toml` `.service`）里，前端工具链
//!   （pnpm/node）只许**指向 frontend/ 边界**（同一行必须点名 frontend）；cargo
//!   不受此限。两条工具链以**固定顺序**衔接：先前端构建产出 `frontend/dist`
//!   （构建产物，不入库），再 `cargo build` 经 `include_dir!` 内嵌；缺 dist 时
//!   `crates/web/build.rs` 在编译期响亮失败并给出构建指引。
//!
//! 关键区分不变：**禁的是工具链越界，不是文件类型**。`crates/web/assets/` 的
//! 历史三件套已退场；浏览器资产现在是 `frontend/dist` 的构建产物（数据）。

mod common;

use common::{find_dirs, has_word, read_text, report, walk_relative, workspace_root};

/// Python 编排位：`.py` 文件唯一合法的容身处（CI 编排，仅标准库）。
const SCRIPTS_DIR: &str = "scripts";

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

/// 可执行面里禁止出现的命令（整词匹配）。cargo 与宿主二进制不在此列。
/// 前端工具链（pnpm/node）不整词禁用，改为「同行必须点名 frontend 边界」（见 R3）；
/// 其余包管理器与 Python 工具一律禁止。
const FORBIDDEN_COMMANDS: &[&str] = &[
    "npm", "npx", "yarn", "bun", "deno", "vite", "pip", "pip3", "pipx", "poetry", "uvx", "mypy",
    "pytest", "ruff", "black", "flake8", "isort", "tox",
];

/// 受「同行点名 frontend 边界」约束的前端工具链命令（整词匹配）。
const FRONTEND_SCOPED_COMMANDS: &[&str] = &["pnpm", "node"];

/// 禁用的短语形态（工具名 + 子命令）。
const FORBIDDEN_PHRASES: &[&str] = &["uv run", "uv pip", "uv sync"];

struct Scan {
    files: Vec<String>,
    python: Vec<String>,
    node: Vec<String>,
    call: Vec<String>,
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// 路径是否落在 Python 编排位内（`scripts/…`）。
fn inside_scripts(path: &str) -> bool {
    path.starts_with(&format!("{SCRIPTS_DIR}/"))
}

/// 路径是否落在前端边界内（`frontend/…`）。
fn inside_frontend(path: &str) -> bool {
    path == FRONTEND_DIR || path.starts_with(&format!("{FRONTEND_DIR}/"))
}

fn scan() -> Scan {
    let root = workspace_root();
    let files = walk_relative(&root);

    let python = python_face(&root, &files);
    let node = node_face(&root, &files);
    let call = call_face(&root, &files);
    Scan {
        files,
        python,
        node,
        call,
    }
}

/// R1：Python 文件只许在 scripts/ 编排位；清单文件哪里都不许有。
fn python_face(root: &std::path::Path, files: &[String]) -> Vec<String> {
    let mut problems = Vec::new();
    for file in files {
        let name = basename(file);

        if name.ends_with(".py") || name.ends_with(".pyi") {
            if inside_scripts(file) {
                continue;
            }
            problems.push(format!(
                "{file}: Python 文件只允许出现在 scripts/ 编排位（CI 编排，仅标准库）"
            ));
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
fn node_face(root: &std::path::Path, files: &[String]) -> Vec<String> {
    let mut problems = Vec::new();
    for file in files {
        let name = basename(file);
        let in_frontend = inside_frontend(file);

        if name == "package.json" {
            if in_frontend {
                continue;
            }
            problems.push(format!(
                "{file}: Node 清单文件只允许出现在 frontend/ 边界内"
            ));
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

/// 可执行面：会被 CI 或部署直接执行的文本。文档与注释不在此列。
fn is_executable_surface(file: &str) -> bool {
    file == "mise.toml"
        || (file.starts_with("scripts/") && (file.ends_with(".py") || file.ends_with(".sh")))
        || file.ends_with(".service")
        || file == ".gitlab-ci.yml"
        || file.starts_with(".gitlab/")
        || (file.starts_with(".github/workflows/")
            && (file.ends_with(".yml") || file.ends_with(".yaml")))
}

/// R3：调用面（可执行面里，前端工具链只许指向 frontend/ 边界；其余禁用命令零命中）。
fn call_face(root: &std::path::Path, files: &[String]) -> Vec<String> {
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
                        "{file}:{number}: 可执行面里不许调用 `{command}`（包管理器与 Python 工具不进可执行面）"
                    ));
                }
            }
            for command in FRONTEND_SCOPED_COMMANDS {
                if has_word(line, command) && !line.to_lowercase().contains("frontend") {
                    problems.push(format!(
                        "{file}:{number}: 调用 `{command}` 的行必须点名 frontend 边界（前端工具链只许指向 frontend/）"
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
        "可执行面里禁用命令零命中；pnpm/node 调用全部点名 frontend 边界".to_string(),
    );
}

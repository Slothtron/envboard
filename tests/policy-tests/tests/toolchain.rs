//! 工具链收敛门禁：**仓库里不许有第二种语言工具链的任何痕迹**（v3：纯 Rust）。
//!
//! 这条规则的价值在于可机械判定 —— 它必须由门禁自己证明，而不是写在文档里靠人自觉。
//! 判据分三面：
//!
//! - **文件面** R1/R2：禁出现的清单文件与源码后缀；
//! - **调用面** R3：可执行面里禁出现的命令形态；
//! - **白名单反向**：迁移白名单条目失效同样判红（v3 起表为空 = 收敛完成）。
//!
//! 关键区分：**禁的是工具链，不是文件类型**。浏览器资产的 `.js` / `.css` / `.html`
//! 是数据（`include_str!` 内嵌、没有构建步骤），不算工具链、不在禁列；
//! 而任何 `.py` 都是第二种语言 —— v3 起仓库里没有"宿主适配器"这个角落了。
//!
//! 迁移期由 [`PENDING`] 承载"待退场"的文件，**双向判定**：白名单条目失效（文件已删
//! 或已不再违规）同样失败，否则这张表会慢慢变成"什么都放行"的垃圾桶。

mod common;

use common::{find_dirs, has_word, read_text, report, walk_relative, workspace_root};

/// 迁移白名单：待退场的旧工具链文件。**已清空 —— 收敛完成**。
///
/// 这张表本身留着：将来若又要搬一条旧门禁，它同时是进度表与降级开关
/// （判断逻辑见 `the_migration_whitelist_has_no_stale_entries`：条目失效同样失败）。
const PENDING: &[&str] = &[];

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

/// Node/TS 工具链的清单与配置 —— 同上。浏览器资产的后缀（`.js`/`.css`/`.html`）
/// **刻意不在此列**：它们是数据，不是工具链。
const NODE_MANIFESTS: &[&str] = &[
    "package-lock.json",
    "pnpm-workspace.yaml",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lockb",
    ".npmrc",
];

/// 可执行面里禁止出现的命令（整词匹配）。`cargo` 与宿主二进制不在此列。
const FORBIDDEN_COMMANDS: &[&str] = &[
    "npm", "npx", "pnpm", "yarn", "bun", "node", "deno", "pip", "pip3", "pipx", "poetry", "uvx",
    "mypy", "pytest", "ruff", "black", "flake8", "isort", "tox",
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

/// R1：Python 面。
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

    // `__pycache__` 被 walk 跳过（它的内容不是证据），所以单独判存在性。
    for directory in find_dirs(root, &["__pycache__"]) {
        problems.push(format!(
            "{directory}: 不许有 Python 字节码缓存目录（工具链跑过 Python 的痕迹）"
        ));
    }
    problems
}

/// R2：Node / TS 面。
fn node_face(root: &std::path::Path, files: &[String], used: &mut Vec<String>) -> Vec<String> {
    let mut problems = Vec::new();
    for file in files {
        let name = basename(file);

        if name == "package.json" {
            if PENDING.contains(&file.as_str()) {
                used.push(file.clone());
            } else {
                problems.push(format!("{file}: 不许有 Node 的清单文件"));
            }
            continue;
        }

        if NODE_MANIFESTS.contains(&name) {
            problems.push(format!("{file}: 不许有 Node 工具链的清单 / 锁文件"));
        }

        if name.ends_with(".ts")
            || name.ends_with(".tsx")
            || name.ends_with(".mts")
            || name.ends_with(".cts")
        {
            problems.push(format!(
                "{file}: 不许有 TypeScript 源码（本仓没有 TS 构建步骤）"
            ));
        }

        if name.starts_with("tsconfig")
            || name.starts_with("vite.config.")
            || name.starts_with("tsdown.config.")
            || name.starts_with("esbuild")
            || name.starts_with("rollup.config.")
            || name.starts_with("webpack.config.")
        {
            problems.push(format!("{file}: 不许有前端 / 打包器配置"));
        }
    }

    for directory in find_dirs(root, &["node_modules"]) {
        problems.push(format!("{directory}: 不许有 Node 依赖树目录"));
    }
    problems
}

/// 可执行面：会被 CI 或部署直接执行的文本。文档与注释不在此列 ——
/// 判据只约束"真的跑得起来的东西"。
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
///
/// 之所以按**路径**而不是按"解释器后面跟什么"来判：解释器常常藏在变量里，
/// 按被引用的脚本路径判才抓得住真实调用点。
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

/// R3：调用面。
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
                        "{file}:{number}: 可执行面里不许调用 `{command}`（第二种语言工具链）"
                    ));
                }
            }
            for phrase in FORBIDDEN_PHRASES {
                if line.contains(phrase) {
                    problems.push(format!("{file}:{number}: 可执行面里不许出现 `{phrase}`"));
                }
            }
            // 解释器调 `-m`：v3 连"跑适配器脚本"这个例外都不存在了。
            for pair in words.windows(2) {
                if pair[0].contains("python") && pair[1].starts_with("-m") {
                    problems.push(format!("{file}:{number}: 可执行面不许调用 Python 工具"));
                }
            }
            // 被引用的本仓脚本：只允许出现在迁移白名单里（表为空 = 一个都不许）。
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
fn r2_no_node_or_typescript_toolchain_in_the_tree() {
    let scan = scan();
    report(
        "toolchain/r2-node-face",
        scan.node,
        format!("{} files scanned, 无 Node/TS 工具链痕迹", scan.files.len()),
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

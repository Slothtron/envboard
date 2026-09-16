//! 工具链收敛门禁：**除宿主适配器外，仓库里不许有第二种语言工具链的痕迹**。
//!
//! 这条规则的价值在于可机械判定 —— 它必须由门禁自己证明，而不是写在文档里靠人自觉。
//! 判据分三面：
//!
//! - **文件面** R1/R2：禁出现的清单文件与源码后缀；
//! - **调用面** R3：可执行面里禁出现的命令形态；
//! - **反向守卫** R4：保留的适配器脚本仍是单文件。
//!
//! 关键区分：**禁的是工具链，不是文件类型**。浏览器的 `assets/app.js` / `app.css`
//! 是数据（`include_str!` 内嵌、没有构建步骤），`adapters/` 下的 `.py` 是产品代码
//! （宿主 mitmproxy 用 `-s` 加载它），两者都不算工具链，因此都不在禁列。
//!
//! 迁移期由 [`PENDING`] 承载"待退场"的文件，**双向判定**：白名单条目失效（文件已删
//! 或已不再违规）同样失败，否则这张表会慢慢变成"什么都放行"的垃圾桶。

mod common;

use common::{find_dirs, has_word, read_text, report, walk_relative, workspace_root};

/// 迁移白名单：待退场的旧工具链文件。每迁完一条就删掉一行；**清空 = 收敛完成**。
const PENDING: &[&str] = &[
    "package.json",
    "scripts/artifact_check.py",
    "scripts/compile_check.py",
    "scripts/spike_m0_5.py",
    "scripts/verify_contract.py",
    "scripts/verify_dual_impl.py",
    "scripts/verify_live_v2.py",
];

/// 唯一允许的非 Rust 角落：宿主适配器。这种脚本是**产品代码**，由宿主解释器加载。
const ADAPTER_PREFIX: &str = "adapters/";

/// Python 工具链的清单文件 —— 哪里都不许有（连适配器也不需要：它是单文件脚本）。
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
    adapter: Vec<String>,
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
    let adapter = adapter_guard(&files);
    Scan {
        files,
        python,
        node,
        call,
        adapter,
        used,
    }
}

/// R1：Python 面。
fn python_face(root: &std::path::Path, files: &[String], used: &mut Vec<String>) -> Vec<String> {
    let mut problems = Vec::new();
    for file in files {
        let name = basename(file);
        let in_adapter = file.starts_with(ADAPTER_PREFIX);

        if (name.ends_with(".py") || name.ends_with(".pyi")) && !in_adapter {
            if PENDING.contains(&file.as_str()) {
                used.push(file.clone());
            } else {
                problems.push(format!(
                    "{file}: 工具链不许用 Python 写（唯一例外是 `{ADAPTER_PREFIX}` 下的宿主适配器脚本）"
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
        if !directory.starts_with(ADAPTER_PREFIX) {
            problems.push(format!(
                "{directory}: 不许有 Python 字节码缓存目录（工具链跑过 Python 的痕迹）"
            ));
        }
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
/// 之所以按**路径**而不是按"解释器后面跟什么"来判：解释器常常藏在变量里
/// （解释器由 `$PYTHON` 之类的变量给出），按被引用的脚本路径判才抓得住真实调用点。
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
            // `<解释器> -m <工具>`：唯一允许的解释器形态是"解释器 + 适配器脚本路径"。
            for pair in words.windows(2) {
                if pair[0].contains("python") && pair[1].starts_with("-m") {
                    problems.push(format!(
                        "{file}:{number}: 不许用 `-m` 方式调 Python 工具；\
                         唯一允许的形态是 `<解释器> <适配器脚本路径>`"
                    ));
                }
            }
            // 被引用的本仓脚本：必须在适配器目录里，或在迁移白名单里。
            for word in &words {
                let Some(script) = script_reference(word) else {
                    continue;
                };
                if script.starts_with(ADAPTER_PREFIX) {
                    continue;
                }
                if PENDING.contains(&script.as_str()) {
                    used.push(script);
                } else {
                    problems.push(format!(
                        "{file}:{number}: 调用了仓库内的脚本 `{script}` —— \
                         非适配器脚本不许被执行"
                    ));
                }
            }
        }
    }
    problems
}

/// R4：反向守卫 —— 适配器角落里必须仍是"单文件、纯标准库"的形态。
fn adapter_guard(files: &[String]) -> Vec<String> {
    let mut problems = Vec::new();
    let scripts: Vec<&String> = files
        .iter()
        .filter(|file| file.starts_with(ADAPTER_PREFIX) && file.ends_with(".py"))
        .collect();
    if scripts.len() != 1 {
        problems.push(format!(
            "{ADAPTER_PREFIX} 下必须恰好一个适配器脚本（它是被宿主加载的产品代码），\
             实际 {}: {scripts:?}",
            scripts.len()
        ));
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
        "可执行面只用 cargo 与宿主适配器".to_string(),
    );
}

#[test]
fn r4_the_adapter_corner_stays_a_single_script() {
    let scan = scan();
    report(
        "toolchain/r4-adapter-guard",
        scan.adapter,
        format!("{ADAPTER_PREFIX} 下恰好一个适配器脚本"),
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

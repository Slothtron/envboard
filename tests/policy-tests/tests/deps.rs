//! 依赖方向门禁：Layering 与"纯逻辑"由 Rust 侧的 manifest 事实判定。
//!
//! 断言（逐条）：
//!
//! 1. 内部依赖边必须落在允许表内（prod 与 dev 分表）；
//! 2. 纯逻辑 crate 不得依赖运行时 / 系统库（必须经端口访问外界）；
//! 3. workspace 根**唯一**（包根 virtual manifest）；
//! 4. `Cargo.lock` 与 `rust-toolchain.toml` 在位（可复现构建与运行下限的前提）；
//! 5. 注册表与磁盘**一一对应**（未登记即失败、登记了但没了也失败）；
//! 6. **只有测试目标**的 crate 不得有产物依赖（它们不进产物，也不该拖进产物依赖）；
//! 7. 门禁 crate 不得被任何 crate 依赖（判据必须是文本事实，不该被产品类型牵着走）；
//! 8. **工作台 lib 面不认识引擎**：`envboard-server` 的 lib 侧源码（除 `src/main.rs`
//!    组合根外）禁止出现引擎 crate 的引用 —— "只认识管理器的公开 API"这条不变量
//!    依旧成立，裁决在源文件面（manifest 分不出 lib 与 bin）；
//! 9. **每份 manifest 显式声明 `publish = false`** —— 本仓不发布 crate，发布物是二进制；
//!    把这件事写成 manifest 事实，才让默认层"不跑 `cargo package --list`"有据可依。

mod common;

use common::{read_text, report, walk_relative, workspace_root};
use std::collections::{BTreeSet, HashSet};

/// 允许的内部依赖边（"谁 → 可以依赖谁"）。不在表里的 crate 视为未登记。
///
/// 这张表管的是**会进产物的依赖**（`dependencies` / `build-dependencies`）。
const ALLOWED_INTERNAL: &[(&str, &[&str])] = &[
    // 引擎库 = 唯一内部根（api 词汇 / rules / domain / 引擎本体全在其中，
    // 原先 api-domain-rules-core 的四条内部边变成了 crate 内的模块纪律，
    // 见下面第 2 条的源文件面判据）
    ("envboard-engine", &[]),
    ("envboard-engine-fake", &["envboard-engine"]),
    ("envboard-manager", &["envboard-engine"]),
    // 工作台 = 唯一宿主入口（lib 的 HTTP 面 + [[bin]] 组合根）。边表里的
    // envboard-engine 只允许 src/main.rs（装配引擎与共享 CA）使用；lib 侧
    // **依旧只认识管理器的公开 API**，由下面的源文件面判据钉死。
    ("envboard-server", &["envboard-engine", "envboard-manager"]),
    // 纯测试 crate
    ("envboard-contract-tests", &[]),
    // 工程门禁：不依赖任何内部 crate
    ("envboard-policy-tests", &[]),
];

/// 只允许出现在 `dev-dependencies` 里的额外边。
///
/// 分层不变量约束的是**产物的依赖图**；测试目标用测试替身（`core-fake`）不但无害，
/// 正是"管理器测试不需要装 mitmproxy"要的效果。把这类边单列出来，比把 `core-fake`
/// 加进上面那张表更清楚：它没有被发布出去。
const ALLOWED_DEV_INTERNAL: &[(&str, &[&str])] = &[
    ("envboard-server", &["envboard-engine-fake"]),
    ("envboard-manager", &["envboard-engine-fake"]),
    ("envboard-contract-tests", &["envboard-engine"]),
];

/// 纯逻辑模块：api / rules / domain 不许引用运行时 / 系统库（源文件面判据；
/// crate 已合并，manifest 分不出模块边界）。扫描时按源文件包含的记号判定。
const PURE_MODULE_PREFIXES: &[&str] = &[
    "src/api/",
    "src/rules/",
    "src/domain/",
    "src/api.rs",
    "src/rules.rs",
    "src/domain.rs",
];
const PURE_FORBIDDEN_TOKENS: &[&str] = &["tokio", "libc", "rcgen", "rustls", "axum"];

/// 纯逻辑 crate 禁止出现的运行时 / 系统依赖。
const IMPURE_DEPS: &[&str] = &[
    "tokio",
    "libc",
    "axum",
    "hyper",
    "hyper-util",
    "reqwest",
    "tower",
    "tower-http",
    "mio",
    "socket2",
];

/// 只有测试目标的 crate：它们的价值是 `cargo test` 的断言，不该有任何产物依赖。
const TEST_ONLY_CRATES: &[&str] = &["envboard-contract-tests", "envboard-policy-tests"];

/// 门禁 crate：谁都不许依赖它。
const GATE_CRATE: &str = "envboard-policy-tests";

const PROD_SECTIONS: &[&str] = &["dependencies", "build-dependencies"];
const DEV_SECTIONS: &[&str] = &["dev-dependencies"];

type Manifest = toml::Table;

fn allowed(table: &[(&str, &[&str])], name: &str) -> Option<BTreeSet<String>> {
    table
        .iter()
        .find(|(crate_name, _)| *crate_name == name)
        .map(|(_, deps)| deps.iter().map(|dep| (*dep).to_string()).collect())
}

/// 指定 section 里的内部 crate 名。内部 crate 有两种写法：带 `path`，或名为 `envboard-*`。
fn internal_deps(manifest: &Manifest, sections: &[&str]) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for section in sections {
        let Some(table) = manifest.get(*section).and_then(|value| value.as_table()) else {
            continue;
        };
        for (name, spec) in table {
            let has_path = spec
                .as_table()
                .is_some_and(|spec| spec.contains_key("path"));
            // 内部 crate 有两种写法：带 `path`，或名为 `envboard-*`（走 workspace 依赖）。
            // 反过来，`toml.workspace = true` 这种**外部** crate 的 workspace 写法不算内部边。
            if has_path || name.starts_with("envboard-") {
                found.insert(name.clone());
            }
        }
    }
    found
}

fn external_deps(manifest: &Manifest, sections: &[&str]) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for section in sections {
        let Some(table) = manifest.get(*section).and_then(|value| value.as_table()) else {
            continue;
        };
        for name in table.keys() {
            if !name.starts_with("envboard-") {
                found.insert(name.clone());
            }
        }
    }
    found
}

fn parse(manifest_path: &std::path::Path, problems: &mut Vec<String>) -> Option<Manifest> {
    let text = read_text(manifest_path)?;
    match text.parse::<Manifest>() {
        Ok(manifest) => Some(manifest),
        Err(error) => {
            problems.push(format!("invalid TOML in {manifest_path:?}: {error}"));
            None
        }
    }
}

#[test]
fn the_rust_layering_holds() {
    let root = workspace_root();
    let mut problems = Vec::new();

    let crate_dirs: Vec<std::path::PathBuf> = ["crates", "tests"]
        .iter()
        .flat_map(|dir| {
            std::fs::read_dir(root.join(dir))
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
        })
        .collect();
    if crate_dirs.is_empty() {
        report(
            "deps",
            vec!["no crates under crates/ or tests/".to_string()],
            String::new(),
        );
        return;
    }

    // 3. workspace 根唯一。
    let mut workspace_manifests = Vec::new();
    for file in walk_relative(&root) {
        if file.rsplit('/').next() != Some("Cargo.toml") {
            continue;
        }
        let Some(manifest) = parse(&root.join(&file), &mut problems) else {
            continue;
        };
        if manifest.contains_key("workspace") {
            workspace_manifests.push(file);
        }
    }
    if workspace_manifests != vec!["Cargo.toml".to_string()] {
        problems.push(format!(
            "workspace 根必须是唯一的包根 Cargo.toml，实际找到：{workspace_manifests:?}"
        ));
    }

    // 4. 可复现构建与运行下限的前提文件。
    for required in ["Cargo.lock", "rust-toolchain.toml"] {
        if !root.join(required).exists() {
            problems.push(format!(
                "{required} 缺失：它是必需产物（`Cargo.lock` 保证可复现构建，\
                 `rust-toolchain.toml` 声明运行下限）"
            ));
        }
    }

    // 1 / 2 / 5 / 6 / 7：逐个 crate。
    let mut seen: HashSet<String> = HashSet::new();

    for crate_dir in crate_dirs {
        let dir_name = crate_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let manifest_path = crate_dir.join("Cargo.toml");
        if !manifest_path.exists() {
            problems.push(format!("{dir_name}/ 没有 Cargo.toml"));
            continue;
        }
        let Some(manifest) = parse(&manifest_path, &mut problems) else {
            continue;
        };
        let Some(name) = manifest
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(|name| name.as_str())
        else {
            problems.push(format!("{dir_name}: 缺 package.name"));
            continue;
        };
        let name = name.to_string();
        seen.insert(name.clone());

        let Some(allowed_prod) = allowed(ALLOWED_INTERNAL, &name) else {
            problems.push(format!(
                "{name}: 未在本门禁注册 —— 引入 crate 时要同时登记它的允许依赖边与分层"
            ));
            continue;
        };
        let extra_dev = allowed(ALLOWED_DEV_INTERNAL, &name).unwrap_or_default();
        let allowed_dev: BTreeSet<String> = allowed_prod.union(&extra_dev).cloned().collect();

        for dependency in internal_deps(&manifest, PROD_SECTIONS) {
            if !allowed_prod.contains(&dependency) {
                problems.push(format!(
                    "{name} 依赖 {dependency}，违反分层（允许：{allowed_prod:?}）"
                ));
            }
        }
        for dependency in internal_deps(&manifest, DEV_SECTIONS) {
            if !allowed_dev.contains(&dependency) {
                problems.push(format!(
                    "{name} 的 dev 依赖 {dependency} 不是已登记的测试专用边\
                     （允许的 dev 边：{:?}）",
                    allowed_dev.difference(&allowed_prod).collect::<Vec<_>>()
                ));
            }
        }

        // 6. 只有测试目标的 crate 不该有产物依赖。
        if TEST_ONLY_CRATES.contains(&name.as_str()) {
            let prod = internal_deps(&manifest, PROD_SECTIONS);
            let external = external_deps(&manifest, PROD_SECTIONS);
            if !prod.is_empty() || !external.is_empty() {
                problems.push(format!(
                    "{name} 只有测试目标，`dependencies` 必须为空\
                     （它是 `cargo test` 的产物，不进发布物）；实际有 {prod:?} {external:?}"
                ));
            }
        }

        // 9. 不发布 crate：显式 publish = false。
        let publishes = manifest
            .get("package")
            .and_then(|package| package.get("publish"))
            .and_then(|value| value.as_bool());
        if publishes != Some(false) {
            problems.push(format!(
                "{name}: 必须在 [package] 里显式写 `publish = false`（本仓不发布 crate，\
                 发布物是 `envboard` 二进制；不写不等于声明了）"
            ));
        }

        // 8. 工作台 lib 面不认识引擎（源文件面判据）。
        if name == "envboard-server" {
            let web_src = crate_dir.join("src");
            let main_rs = web_src.join("main.rs");
            if !main_rs.exists() {
                problems.push(
                    "envboard-server/src/main.rs 缺失：二进制 = 工作台启动器，组合根就在这里"
                        .to_string(),
                );
            }
            for entry in walk_relative(&web_src) {
                let relative = entry.clone();
                if relative.ends_with("main.rs") {
                    continue;
                }
                let Some(text) = read_text(&web_src.join(&relative)) else {
                    continue;
                };
                let assembly = [
                    "EngineInstance",
                    "EngineConfig",
                    "CompiledConfig",
                    "SharedCa",
                    "EngineBackend",
                ]
                .iter()
                .find(|symbol| text.contains(*symbol));
                if let Some(symbol) = assembly {
                    problems.push(format!(
                        "envboard-server/src/{relative} 引用了引擎装配符号 {symbol:?}：lib 面只许认识管理器的公开 API（共享错误码等词汇除外），引擎装配只允许出现在 src/main.rs"
                    ));
                }
            }
        }

        // 2'. 纯逻辑模块（源文件面判据）：api / rules / domain 不碰运行时与 TLS 栈。
        if name == "envboard-engine" {
            let engine_src = crate_dir.join("src");
            for entry in walk_relative(&engine_src) {
                let is_pure = PURE_MODULE_PREFIXES.iter().any(|prefix| {
                    entry.starts_with(prefix.trim_end_matches('/')) || entry.starts_with(prefix)
                });
                if !is_pure {
                    continue;
                }
                let Some(text) = read_text(&engine_src.join(&entry)) else {
                    continue;
                };
                let token = PURE_FORBIDDEN_TOKENS
                    .iter()
                    .find(|token| text.contains(*token));
                if let Some(token) = token {
                    problems.push(format!(
                        "envboard-engine/src/{entry} 是纯逻辑模块却引用了 {token:?}：必须经端口访问外界"
                    ));
                }
            }
        }

        // 7. 门禁 crate 不得被依赖。
        let all_edges: BTreeSet<String> = internal_deps(&manifest, PROD_SECTIONS)
            .union(&internal_deps(&manifest, DEV_SECTIONS))
            .cloned()
            .collect();
        if all_edges.contains(GATE_CRATE) {
            problems.push(format!(
                "{name} 依赖了 {GATE_CRATE}：工程门禁不是产品依赖，判据也不该被产品类型牵着走"
            ));
        }
    }

    // 5. 反向：登记了但磁盘上没有。
    let missing: Vec<String> = ALLOWED_INTERNAL
        .iter()
        .map(|(name, _)| (*name).to_string())
        .filter(|name| !seen.contains(name))
        .collect();
    if !missing.is_empty() {
        problems.push(format!("已登记但磁盘上不存在：{missing:?}"));
    }

    report(
        "deps",
        problems,
        format!(
            "{} 个 crate，分层完整、全部显式 publish = false；纯逻辑边界在 api/rules/domain 模块面（源文件判据）",
            seen.len()
        ),
    );
}

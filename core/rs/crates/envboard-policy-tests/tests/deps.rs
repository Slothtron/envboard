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
//! 8. **工作台 lib 面不认识引擎**：`envboard-web` 的 lib 侧源码（除 `src/main.rs`
//!    组合根外）禁止出现 `envboard_core` 的引用 —— "只认识管理器的公开 API"这条
//!    不变量在 CLI 退役、`[[bin]]` 并入本 crate 之后**依旧成立**，只是裁决从边表
//!    细化到了源文件面（manifest 分不出 lib 与 bin）；
//! 9. **每份 manifest 显式声明 `publish = false`** —— 本仓不发布 crate，发布物是二进制；
//!    把这件事写成 manifest 事实，才让默认层"不跑 `cargo package --list`"有据可依。

mod common;

use common::{read_text, report, walk_relative, workspace_root};
use std::collections::{BTreeSet, HashSet};

/// 允许的内部依赖边（"谁 → 可以依赖谁"）。不在表里的 crate 视为未登记。
///
/// 这张表管的是**会进产物的依赖**（`dependencies` / `build-dependencies`）。
const ALLOWED_INTERNAL: &[(&str, &[&str])] = &[
    // 根：契约词汇，不依赖任何内部 crate
    ("envboard-core-api", &[]),
    // 环境层要校验 `insecure_hosts` 的每个条目 —— host 的归一化/校验只在 rules
    // 里有那一份实现，不在 domain 里复制第二份 label 规则。两者都是纯逻辑，也不成环。
    ("envboard-domain", &["envboard-core-api", "envboard-rules"]),
    ("envboard-rules", &["envboard-core-api"]),
    (
        "envboard-core-fake",
        &["envboard-core-api", "envboard-rules"],
    ),
    (
        "envboard-manager",
        &["envboard-core-api", "envboard-domain", "envboard-rules"],
    ),
    // 工作台 = 唯一宿主入口（lib 的 HTTP 面 + [[bin]] 组合根）。边表里的
    // envboard-core 只允许 src/main.rs（装配引擎与共享 CA）使用；lib 侧
    // （api / config / lib.rs）**依旧只认识管理器的公开 API**，由下面的
    // 源文件面判据把这条不变量钉死 —— "换引擎实现不动工作台"没被搬家稀释。
    (
        "envboard-web",
        &[
            "envboard-core-api",
            "envboard-domain",
            "envboard-manager",
            "envboard-core",
        ],
    ),
    // v3 纯库引擎。规则解析复用 envboard-rules 的唯一实现（注入器镜像随
    // mitmproxy 一起退场）；"不依赖 axum"由这张边表本身就是判据。
    ("envboard-core", &["envboard-core-api", "envboard-rules"]),
    (
        "envboard-contract-tests",
        &["envboard-core-api", "envboard-domain", "envboard-rules"],
    ),
    // 工程门禁：不依赖任何内部 crate
    ("envboard-policy-tests", &[]),
];

/// 只允许出现在 `dev-dependencies` 里的额外边。
///
/// 分层不变量约束的是**产物的依赖图**；测试目标用测试替身（`core-fake`）不但无害，
/// 正是"管理器测试不需要装 mitmproxy"要的效果。把这类边单列出来，比把 `core-fake`
/// 加进上面那张表更清楚：它没有被发布出去。
const ALLOWED_DEV_INTERNAL: &[(&str, &[&str])] = &[
    ("envboard-web", &["envboard-core-fake"]),
    ("envboard-manager", &["envboard-core-fake"]),
    (
        "envboard-contract-tests",
        &["envboard-core-api", "envboard-domain", "envboard-rules"],
    ),
];

/// 纯逻辑 crate：只允许标准库 + 序列化 / 错误这类"无副作用"的依赖。
const PURE_CRATES: &[&str] = &["envboard-core-api", "envboard-domain", "envboard-rules"];

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

    let crates_dir = root.join("core/rs/crates");
    if !crates_dir.is_dir() {
        report(
            "deps",
            vec![format!("no crates at {crates_dir:?}")],
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
    let mut crate_dirs: Vec<std::path::PathBuf> = std::fs::read_dir(&crates_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    crate_dirs.sort();

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

        if PURE_CRATES.contains(&name.as_str()) {
            let impure: Vec<String> = external_deps(&manifest, PROD_SECTIONS)
                .into_iter()
                .filter(|dep| IMPURE_DEPS.contains(&dep.as_str()))
                .collect();
            if !impure.is_empty() {
                problems.push(format!(
                    "{name} 是纯逻辑 crate 却依赖 {impure:?}：core 不得直接触碰运行时\
                     （必须经端口）"
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

        // 8. 工作台 lib 面不认识引擎（CLI 退役后的源文件面判据）。
        if name == "envboard-web" {
            let web_src = crate_dir.join("src");
            let main_rs = web_src.join("main.rs");
            if !main_rs.exists() {
                problems.push(
                    "envboard-web/src/main.rs 缺失：二进制 = 工作台启动器，组合根就在这里"
                        .to_string(),
                );
            }
            let Ok(entries) = std::fs::read_dir(&web_src) else {
                problems.push(format!("envboard-web/src 读不了：{web_src:?}"));
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.file_name().is_some_and(|n| n == "main.rs") {
                    continue;
                }
                let Some(text) = read_text(&path) else {
                    continue;
                };
                if text.contains("envboard_core::") {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    problems.push(format!(
                        "envboard-web/src/{name} 引用了 envboard_core：lib 面只许认识管理器的公开 API，引擎装配只允许出现在 src/main.rs"
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
            "{} 个 crate，分层完整、全部显式 publish = false；纯逻辑：{PURE_CRATES:?}",
            seen.len()
        ),
    );
}

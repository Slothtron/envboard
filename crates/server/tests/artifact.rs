//! 发布物内容校验：发布物必须**只含预期文件**。
//!
//! 本包不发布 crate —— 发布物是 **`envboard` 二进制**（外加它运行时物化出来的注入器）。
//! 所以这里校验的是「发布工件清单」，而不是 `cargo package --list`：
//!
//! 1. **必需文件在位**：`Cargo.lock`（可复现构建）、`rust-toolchain.toml`（运行下限）、
//!    `LICENSE`、`README.md`、`CHANGELOG.md`；
//! 2. **v1 的残留必须为零**：`src/`、`addons/`、`pyproject.toml`、`tests/` 都不该再存在 ——
//!    否则会出现"两套实现并存、但只有一套在用"的迷惑状态；
//! 3. **注入器必须是单文件、且被二进制内嵌**：源码里不得出现对 v1 模块的 import，
//!    二进制里必须找得到它的标记字符串（`include_str!` 真的嵌进去了）；
//! 4. **产物目录不得入库**：`target/` 必须在 `.gitignore` 里；
//! 5. **发布工件清单闭合**：二进制 + 必需文件 + systemd unit，逐项存在。
//!
//! 二进制用 `CARGO_BIN_EXE_envboard` 取：cargo 保证跑集成测试前已经构建好它，
//! 于是"artifact 必须先 cargo build"这条顺序要求不存在（也不会拼 `target/...` 路径）。
//!
//! 为什么不用 `cargo package --list`：本仓的 crate **不可打包** —— 内部依赖都是不带版本的
//! path 依赖。
//! 因此"不发布 crate"是显式声明的 manifest 事实（`publish = false`，由 `deps` 门禁钉住），
//! 默认层只做**声明式**校验；对构建产物的实内容校验属于发布流程。

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("repo root")
}

const REQUIRED: &[&str] = &[
    "Cargo.lock",
    "rust-toolchain.toml",
    "LICENSE",
    "README.md",
    "CHANGELOG.md",
];

/// 这几条**故意**不存在：它们断言 v1 的产物已清干净（v1 的 Python `tests/`
/// 目录已由 crates/ 与 tests/ 的 Rust crate 取代，故不再列）。
const REMOVED_IN_V2: &[&str] = &[
    "src",
    "addons",
    "pyproject.toml",
    "scripts/pack_check.py", // doc-scope-lint: allow —— v1 的发布物校验脚本，路径故意不存在
];

/// 这些**测试资产**不进二进制，但必须随仓库存在 —— mitm 测试的四件假值 CA
/// fixture 曾被全局 gitignore（`*.pem`）静默吞掉，制造了"worktree 全绿、fresh
/// clone 必红"的事故（测试到 master 上 NotFound）。全局 ignore 管得着仓库想留的
/// 文件，所以入库判据必须由门禁自己钉。
const REPO_TEST_ASSETS: &[&str] = &[
    "crates/engine/tests/fixtures/mitmproxy-compat/mitmproxy-ca.pem",
    "crates/engine/tests/fixtures/mitmproxy-compat/mitmproxy-ca-cert.pem",
    "crates/engine/tests/fixtures/mitmproxy-compat-alt/mitmproxy-ca.pem",
    "crates/engine/tests/fixtures/mitmproxy-compat-alt/mitmproxy-ca-cert.pem",
];

/// 发布工件清单（二进制另由 [`envboard_binary`] 校验）。
const RELEASE_ARTIFACTS: &[&str] = &[
    "Cargo.lock",
    "rust-toolchain.toml",
    "LICENSE",
    "README.md",
    "CHANGELOG.md",
    "scripts/systemd/envboard.service",
];

fn envboard_binary() -> &'static str {
    env!("CARGO_BIN_EXE_envboard")
}

/// 收集 TS 联合类型（`export type X = ... ;`）里的全部字符串字面量。
fn union_literals(text: &str, anchor: &str) -> std::collections::BTreeSet<String> {
    let mut found = std::collections::BTreeSet::new();
    let Some(start) = text.find(anchor) else {
        return found;
    };
    let rest = &text[start..];
    let region = match rest.find(';') {
        Some(end) => &rest[..end],
        None => rest,
    };
    let mut cursor = region;
    while let Some(open) = cursor.find('"') {
        cursor = &cursor[open + 1..];
        match cursor.find('"') {
            Some(close) => {
                found.insert(cursor[..close].to_string());
                cursor = &cursor[close + 1..];
            }
            None => break,
        }
    }
    found
}

/// 收集 TS/TSX 数组定义里 `key: "…"` 字面量的值（NAV 的视图键）。
fn key_literals(text: &str, anchor: &str) -> std::collections::BTreeSet<String> {
    let mut found = std::collections::BTreeSet::new();
    let needle = "key: \"";
    for (index, _) in text.match_indices(needle) {
        if index < text.find(anchor).unwrap_or(usize::MAX) {
            continue;
        }
        let rest = &text[index + needle.len()..];
        if let Some(close) = rest.find('"') {
            found.insert(rest[..close].to_string());
        }
    }
    found
}

#[test]
fn the_release_artifact_contains_only_expected_files() {
    let root = repo_root();
    let mut problems = Vec::new();

    for name in REQUIRED {
        if !root.join(name).exists() {
            problems.push(format!("缺必需工件：{name}"));
        }
    }

    for name in REMOVED_IN_V2 {
        if root.join(name).exists() {
            problems.push(format!(
                "{name} 应在 v2 被移除 —— 留着它会让人分不清哪套实现是权威"
            ));
        }
    }

    // v3 反向断言：宿主适配器的角落整体退场（原"注入器自包含"检查的接替者，
    // 全仓零 .py 由工具链门禁 R1 兜底）。
    if root.join("adapters").exists() {
        problems.push("v3 不再有任何 adapters/ 目录 —— 引擎在仓库内部，没有宿主接线层".to_string());
    }

    // v3 断言：进程内引擎在场（引擎线程名是引擎运行时的必然足迹）。
    let binary = std::fs::read(envboard_binary()).expect("built binary must be readable");
    const ENGINE_MARKER: &[u8] = b"envboard-engine-";
    if !binary
        .windows(ENGINE_MARKER.len())
        .any(|w| w == ENGINE_MARKER)
    {
        problems.push("二进制里找不到进程内引擎的簿记标记 —— 发布的还是 v2 形态？".to_string());
    }

    let gitignore = std::fs::read_to_string(root.join(".gitignore")).expect(".gitignore");
    if !gitignore.contains("target/") {
        problems.push("`target/` 必须被 git 忽略".to_string());
    }

    for name in RELEASE_ARTIFACTS {
        if !root.join(name).exists() {
            problems.push(format!("发布工件清单里的 {name} 不存在"));
        }
    }

    // 测试资产同样逐项在位（它们不进发布物，但必须随仓库走）。
    for name in REPO_TEST_ASSETS {
        if !root.join(name).exists() {
            problems.push(format!(
                "测试资产 {name} 不在仓库里 —— fresh clone 上的测试必红"
            ));
        }
    }

    // ------------------------------------------------------------------ #
    // v4：内嵌资产 = frontend/dist（Vite 构建产物，include_dir! 进二进制）。
    // 最阴险的坏法仍是"截断/漂移"：头部完好、尾部消失，页面白屏却处处绿灯。
    // 钉住：dist 在位 + 体积下限 + 外链形态 + 关键符号；旧三件套必须已退场。
    // ------------------------------------------------------------------ #
    if root.join("crates/web/assets").exists() {
        problems
            .push("crates/web/assets 应已随 v4 前端工程化退场 —— 留着会出现两套资产".to_string());
    }

    let dist_dir = root.join("frontend/dist");
    let dist_index = std::fs::read_to_string(dist_dir.join("index.html"))
        .expect("内嵌的 frontend/dist/index.html 必须存在（pnpm build 产物，提交入库）");
    let assets_dir = dist_dir.join("assets");
    let mut js_files: Vec<_> = std::fs::read_dir(&assets_dir)
        .expect("frontend/dist/assets 必须存在")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "js"))
        .collect();
    let css_files: Vec<_> = std::fs::read_dir(&assets_dir)
        .expect("frontend/dist/assets 必须存在")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "css"))
        .collect();
    if js_files.is_empty() || css_files.is_empty() {
        problems.push("dist/assets 缺 JS 或 CSS 产物 —— 构建不完整？".to_string());
    }
    js_files.sort();
    let js_bundle = js_files
        .first()
        .map(|path| std::fs::read_to_string(path).expect("dist js 可读"))
        .unwrap_or_default();
    let css_bundle = css_files
        .first()
        .map(|path| std::fs::read_to_string(path).expect("dist css 可读"))
        .unwrap_or_default();
    if js_bundle.len() < 400_000 {
        problems.push(format!(
            "dist JS 只有 {} B —— 疑似截断（HeroUI+React 基线 400KB+）",
            js_bundle.len()
        ));
    }
    if css_bundle.len() < 300_000 {
        problems.push(format!(
            "dist CSS 只有 {} B —— 疑似截断（Tailwind+HeroUI 基线 300KB+）",
            css_bundle.len()
        ));
    }

    // 资产形态 = 外链（严格 CSP 的前提）：index.html 引 hashed 资源、无内联脚本、无 CDN。
    if !dist_index.contains("/assets/") {
        problems.push(
            "dist/index.html 没有引用 /assets/ 外链 —— 资产形态漂移（CSP 会拒绝内联）".to_string(),
        );
    }
    if dist_index.contains("<script>") {
        problems.push(
            "dist/index.html 出现内联 <script> —— 严格 CSP 下静默失效，且违反资产纪律".to_string(),
        );
    }
    if dist_index.contains("src=\"http") || dist_index.contains("href=\"http") {
        problems.push("dist/index.html 引用外部 CDN —— 离线单二进制的前提被破坏".to_string());
    }

    // 接线两端各钉一枚：前端连流 + 关键交互文案在 bundle 里。
    for (needle, where_) in [
        ("工作台导航", "dist js"),
        ("/api/debug/stream", "dist js"),
        ("/api/events", "dist js"),
        ("此操作不可撤销", "dist js"),
    ] {
        if !js_bundle.contains(needle) {
            problems.push(format!("{where_} 缺 {needle:?} —— 前端接线/交互契约退场"));
        }
    }

    // 规范：明亮主题禁纯黑（v1 的教训：令牌缺席时 fallback 悄悄生效）。
    if css_bundle.contains("rgba(0, 0, 0") {
        problems.push("dist css 出现纯黑 rgba(0, 0, 0, …) —— 规范禁纯黑".to_string());
    }

    // v4 的"导航 ↔ 视图主体一一对应"等价断言：data.ts 的 ViewKey、SideBar 的
    // GROUPS keys、Workbench 的 `view === "…"` 分支三个集合必须相等 ——
    // 加视图漏任何一处，点它必然白屏（旧三件套时代真坏过）。
    let view_keys = union_literals(
        &std::fs::read_to_string(root.join("frontend/src/data.ts")).expect("data.ts"),
        "export type ViewKey",
    );
    let nav_keys = key_literals(
        &std::fs::read_to_string(root.join("frontend/src/SideBar.tsx")).expect("SideBar.tsx"),
        "const NAV",
    );
    let workbench_src =
        std::fs::read_to_string(root.join("frontend/src/Workbench.tsx")).expect("Workbench.tsx");
    let body_keys: std::collections::BTreeSet<String> = workbench_src
        .match_indices("view === \"")
        .filter_map(|(index, _)| {
            let rest = &workbench_src[index + "view === \"".len()..];
            rest.split('"').next().map(str::to_string)
        })
        .collect();
    if view_keys != nav_keys {
        problems.push(format!(
            "ViewKey {view_keys:?} 与侧栏导航 {nav_keys:?} 不一致 —— 加视图必须三处同改"
        ));
    }
    if view_keys != body_keys {
        problems.push(format!(
            "ViewKey {view_keys:?} 与 Workbench 渲染分支 {body_keys:?} 不一致 —— 漏分支的视图点开即白屏"
        ));
    }

    // 内嵌在场证明：二进制里找得到入口页的标题字符串（include_dir! 真的嵌了）。
    if !binary
        .windows("envboard 环境代理工作台".len())
        .any(|window| window == "envboard 环境代理工作台".as_bytes())
    {
        problems.push("二进制里找不到前端入口页标题 —— dist 没有内嵌进产物？".to_string());
    }

    let api_rs =
        std::fs::read_to_string(root.join("crates/web/src/api.rs")).expect("api.rs 必须存在");
    if !api_rs.contains("\"/api/debug/stream\"") {
        problems.push("api.rs 没有 /api/debug/stream 路由 —— 实时流端点退场".to_string());
    }

    if !problems.is_empty() {
        println!("artifact FAILED ({} problem(s)):", problems.len());
        for problem in &problems {
            println!("  {problem}");
        }
        panic!("artifact FAILED ({} problem(s))", problems.len());
    }

    let size = std::fs::metadata(envboard_binary())
        .map(|meta| meta.len())
        .unwrap_or(0);
    println!(
        "artifact OK (binary {} KiB、{} 个必需工件在位、{} 个 v1 残留、\
         发布工件清单 {} 项齐全、导航 ↔ 视图主体 {} 对、无 adapters/ 角落)",
        size / 1024,
        REQUIRED.len(),
        REMOVED_IN_V2.len(),
        RELEASE_ARTIFACTS.len(),
        view_keys.len()
    );
}

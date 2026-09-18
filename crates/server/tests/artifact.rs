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

/// 这几条**故意**不存在：它们断言 v1 的产物已清干净。
const REMOVED_IN_V2: &[&str] = &[
    "src",
    "addons",
    "pyproject.toml",
    "tests",
    "scripts/pack_check.py", // doc-scope-lint: allow —— v1 的发布物校验脚本，路径故意不存在
];

/// 这些**测试资产**不进二进制，但必须随仓库存在 —— mitm 测试的四件假值 CA
/// fixture 曾被全局 gitignore（`*.pem`）静默吞掉，制造了"worktree 全绿、fresh
/// clone 必红"的事故（测试到 master 上 NotFound）。全局 ignore 管得着仓库想留的
/// 文件，所以入库判据必须由门禁自己钉。
const REPO_TEST_ASSETS: &[&str] = &[
    "core/rs/crates/envboard-core/tests/fixtures/mitmproxy-compat/mitmproxy-ca.pem",
    "core/rs/crates/envboard-core/tests/fixtures/mitmproxy-compat/mitmproxy-ca-cert.pem",
    "core/rs/crates/envboard-core/tests/fixtures/mitmproxy-compat-alt/mitmproxy-ca.pem",
    "core/rs/crates/envboard-core/tests/fixtures/mitmproxy-compat-alt/mitmproxy-ca-cert.pem",
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

    // v3 断言：进程内引擎在场（引擎线程名是 envboard-core 运行时的必然足迹）。
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

    // 内嵌资产没有编译期检查 —— 最阴险的坏法是"截断"：头部完好、尾部消失，
    // 页面白屏或卡死却处处绿灯。钉住两端：最小规模 + 关键符号在场。
    let app_js = std::fs::read_to_string(root.join("crates/server/assets/app.js"))
        .expect("内嵌的 app.js 必须存在");
    if app_js.lines().count() < 1_800 {
        problems.push(format!(
            "app.js 只剩 {} 行 —— 疑似截断（基线 2000+ 行）",
            app_js.lines().count()
        ));
    }
    for marker in [
        "envboardReady",
        "function renderDetail(",
        "refreshAll",
        "addEventListener",
    ] {
        if !app_js.contains(marker) {
            problems.push(format!("app.js 缺关键符号 {marker:?} —— 被截断或改写了？"));
        }
    }
    let index_html = std::fs::read_to_string(root.join("crates/server/assets/index.html"))
        .expect("内嵌的 index.html 必须存在");
    for id in ["env-form", "detail-actions", "panel-config"] {
        if !index_html.contains(id) {
            problems.push(format!(
                "index.html 缺 id {id:?} —— 前端结构变了，门禁与契约要一起更新"
            ));
        }
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
         发布工件清单 {} 项齐全、无 adapters/ 角落)",
        size / 1024,
        REQUIRED.len(),
        REMOVED_IN_V2.len(),
        RELEASE_ARTIFACTS.len()
    );
}

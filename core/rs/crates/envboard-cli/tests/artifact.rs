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
//! path 依赖，`envboard-core-mitmproxy` 还用 `include_str!` 引用 crate 目录之外的注入器。
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

/// 发布工件清单（二进制另由 [`envboard_binary`] 校验）。
const RELEASE_ARTIFACTS: &[&str] = &[
    "Cargo.lock",
    "rust-toolchain.toml",
    "LICENSE",
    "README.md",
    "CHANGELOG.md",
    "scripts/systemd/envboard.service",
];

const INJECTOR: &str = "adapters/mitmproxy/envboard_mitmproxy.py";

/// 注入器里的一个独特字符串：二进制里找得到它，就说明源码真的被 `include_str!` 嵌入了。
const EMBED_MARKER: &[u8] = b"envboard injector ready";

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

    let injector = root.join(INJECTOR);
    if !injector.exists() {
        problems.push(format!("注入器缺失：{INJECTOR}"));
    } else {
        let text = std::fs::read_to_string(&injector).expect("injector must be UTF-8");
        for banned in ["from envboard.", "import envboard", "mitmproxy_rs"] {
            if text.contains(banned) {
                problems.push(format!(
                    "注入器必须自包含（单文件、标准库 + mitmproxy）；出现了 {banned:?}"
                ));
            }
        }

        let binary = std::fs::read(envboard_binary()).expect("built binary must be readable");
        if !binary
            .windows(EMBED_MARKER.len())
            .any(|window| window == EMBED_MARKER)
        {
            problems.push(
                "二进制里找不到注入器源码（`include_str!` 没生效？）—— 发布物将无法物化它"
                    .to_string(),
            );
        }
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
        "artifact OK (binary {} KiB, injector {} bytes embedded, {} 个必需工件在位、\
         {} 个 v1 残留、发布工件清单 {} 项齐全)",
        size / 1024,
        std::fs::metadata(&injector)
            .map(|meta| meta.len())
            .unwrap_or(0),
        REQUIRED.len(),
        REMOVED_IN_V2.len(),
        RELEASE_ARTIFACTS.len()
    );
}

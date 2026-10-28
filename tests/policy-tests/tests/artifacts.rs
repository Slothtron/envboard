//! 发布物纪律门禁：**仓库里必须有的东西一件不缺，该清干净的残留一处不留**。
//!
//! 原来这批断言住在 web 宿主的集成测试里（`crates/server/tests/artifact.rs`， // doc-scope-lint: allow
//! 已随 web 形态出库）。<!-- doc-scope-lint: allow -->
//! web 形态整体出库后，里面**与形态无关**的那一半留了下来（本文件），
//! **与二进制内容有关**的那一半搬到了它真正的归属：`crates/desktop/tests/artifact.rs`
//! （只有产物 crate 的集成测试才拿得到 `CARGO_BIN_EXE_envboard`）。
//! 判据本身仍然全是文件面事实，所以放在 policy-tests 里，与依赖方向、工具链收敛
//! 同一层跑。
//!
//! 断言（逐条）：
//!
//! 1. **必需文件在位**：`Cargo.lock`（可复现构建的前提）、`rust-toolchain.toml`
//!    （运行下限）、`LICENSE`、`README.md`、`CHANGELOG.md`；
//! 2. **历史形态的残留为零**：v1 的 Python 时代目录（`src/`、`addons/`、
//!    `pyproject.toml`）与 v3 的宿主接线层（`adapters/`）都不许再存在 ——
//!    留着它们会让人分不清哪套实现是权威；
//! 3. **产物目录不入库**：`.gitignore` 必须忽略 `target/`；
//! 4. **测试资产必须随仓库存在**：mitm 测试消费的假值 CA fixture 曾被全局
//!    `*.pem` 静默吞掉，制造过"worktree 全绿、fresh clone 必红"的事故。
//!    全局 ignore 管的是仓库想留的文件，所以"这些文件必须在"这件事必须由门禁自己钉。

mod common;

use common::{read_text, report, workspace_root};

/// 缺一样就不算完整交付的仓库文件。
const REQUIRED: &[&str] = &[
    "Cargo.lock",
    "rust-toolchain.toml",
    "LICENSE",
    "README.md",
    "CHANGELOG.md",
];

/// 这几条**故意**断言不存在：它们是历代形态的残留，清干净才算换过代。
const MUST_NOT_EXIST: &[&str] = &["src", "addons", "pyproject.toml", "adapters"];

/// 不进二进制、但必须随仓库存在的测试资产（全局 `*.pem` 会吞掉它们，
/// 所以 `.gitignore` 里为它们开了显式豁免 —— 这里钉住"文件真的在"）。
const REPO_TEST_ASSETS: &[&str] = &[
    "crates/engine/tests/fixtures/mitmproxy-compat/mitmproxy-ca.pem",
    "crates/engine/tests/fixtures/mitmproxy-compat/mitmproxy-ca-cert.pem",
    "crates/engine/tests/fixtures/mitmproxy-compat-alt/mitmproxy-ca.pem",
    "crates/engine/tests/fixtures/mitmproxy-compat-alt/mitmproxy-ca-cert.pem",
];

#[test]
fn the_repository_ships_what_it_declares() {
    let root = workspace_root();
    let mut problems = Vec::new();

    for name in REQUIRED {
        if !root.join(name).exists() {
            problems.push(format!("缺必需工件：{name}"));
        }
    }

    for name in MUST_NOT_EXIST {
        if root.join(name).exists() {
            problems.push(format!(
                "{name} 属于已退场的历史形态 —— 它还在就说明有两套实现并存"
            ));
        }
    }

    let gitignore = read_text(&root.join(".gitignore")).unwrap_or_default();
    if !gitignore.contains("target/") {
        problems.push("`target/` 必须被 git 忽略（构建产物一律由构建生成）".to_string());
    }

    for name in REPO_TEST_ASSETS {
        if !root.join(name).exists() {
            problems.push(format!(
                "测试资产 {name} 不在仓库里 —— fresh clone 上 mitm 测试会 NotFound，\
                 而 worktree 里全绿（这正是它曾经发生过的原因）"
            ));
        }
    }

    report(
        "artifacts",
        problems,
        format!(
            "{} 件必需文件 + {} 件测试资产在位，{} 类历史残留为零",
            REQUIRED.len(),
            REPO_TEST_ASSETS.len(),
            MUST_NOT_EXIST.len()
        ),
    );
}

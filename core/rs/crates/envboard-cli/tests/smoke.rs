//! 冒烟：二进制能跑、`--core fake` 能建环境、环境列表看得到。
//!
//! 这一层原先写在 `ci/verify.sh` 的 `step_smoke` 里（含一处 shell 里的文本匹配），
//! 现已搬进测试：**入口脚本只做编排，判据一律在能编译的语言里**。
//! 注入器 `--render` 的那条断言不再单独写一遍 —— `envboard-rules` 的 `dual_impl`
//! 会把每个 rules fixture 都经注入器渲染，并与 Rust 侧逐字节比对，而 Rust 侧渲染格式
//! 由 `envboard-rules` 自己的单测钉住（含 `# envboard rules file` 一行）。

use std::path::{Path, PathBuf};
use std::process::Command;

fn envboard() -> &'static str {
    env!("CARGO_BIN_EXE_envboard")
}

/// 每个测试一个临时状态目录，互不干扰。
fn state_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("envboard-smoke-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create state dir");
    dir
}

fn run(state: &Path, args: &[&str]) -> std::process::Output {
    Command::new(envboard())
        .arg("--state-dir")
        .arg(state)
        .args(args)
        .output()
        .expect("run envboard")
}

#[test]
fn the_binary_smokes_with_the_fake_core() {
    let state = state_dir("basic");

    let help = run(&state, &["--help"]);
    assert!(
        help.status.success(),
        "`--help` 必须成功：{}",
        String::from_utf8_lossy(&help.stderr)
    );

    let status = run(&state, &["--core", "fake", "status"]);
    assert!(
        status.status.success(),
        "`--core fake status` 必须成功：{}",
        String::from_utf8_lossy(&status.stderr)
    );

    let added = run(
        &state,
        &["--core", "fake", "env", "add", "smoke", "--port", "16666"],
    );
    assert!(
        added.status.success(),
        "建环境必须成功：{}",
        String::from_utf8_lossy(&added.stderr)
    );

    let second = run(
        &state,
        &["--core", "fake", "env", "add", "probe", "--port", "16667"],
    );
    assert!(
        second.status.success(),
        "第二个环境也必须能建：{}",
        String::from_utf8_lossy(&second.stderr)
    );

    let listed = run(&state, &["--core", "fake", "env", "list"]);
    let stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(
        listed.status.success() && stdout.contains("smoke"),
        "环境列表里必须看得到刚建的环境：{stdout}"
    );

    let _ = std::fs::remove_dir_all(&state);
}

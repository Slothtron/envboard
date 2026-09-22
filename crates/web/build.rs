//! 构建前置检查：`frontend/dist` 必须先于 cargo 存在。
//!
//! 两条工具链的衔接纪律（README「工具链纪律」）：先 `pnpm build` 产出
//! `frontend/dist`（构建产物，不入库），再 `cargo build` 经 `include_dir!`
//! 内嵌。本脚本把这个顺序变成**编译期的响亮失败** —— 缺 dist 时给出
//! 可执行的构建指引，而不是让 include_dir 宏抛一句看不懂的路径错误。

use std::path::Path;

fn main() {
    let dist = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("frontend")
        .join("dist");
    println!("cargo::rerun-if-changed={}", dist.display());
    if !dist.join("index.html").exists() {
        panic!(
            "frontend/dist 缺失或为空 —— 构建顺序是先前端后 Rust：\n  \
             cd frontend && pnpm install --frozen-lockfile && pnpm build\n  \
             （或 python scripts/verify.py frontend）\n\
             产物 dist/ 不入库，由本 crate 的 include_dir! 在编译期内嵌。"
        );
    }
}

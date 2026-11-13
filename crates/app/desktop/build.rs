//! 构建衔接：**`cargo build` 自动驱动前端构建**（自 envboard-web 移植，语义不变），
//! 随后跑 tauri-build（它把 `frontendDist` 在编译期嵌进二进制）。
//!
//! ```text
//! cargo build（envboard-desktop）
//!   └─ pnpm install --frozen-lockfile --prefer-offline   （cwd = frontend/）
//!   └─ pnpm build（tsc --noEmit && vite build → frontend/dist/）
//!   └─ 校验 dist/index.html 存在，缺则响亮失败
//!   └─ tauri_build::build()（嵌入 dist + 生成权限/图标资源）
//! ```
//!
//! 三条性质：
//!
//! * **增量友好**：rerun-if-changed 只盯前端源码与清单 —— 前端不变时不重跑 pnpm；
//! * **离线口径**：`--prefer-offline` 吃已暖的 pnpm store；fresh clone 首次构建要网，
//!   之后 `cargo build --offline` 可达；
//! * **逃生门**：`ENVBOARD_SKIP_FRONTEND_BUILD=1` 跳过前端构建、直接消费既有
//!   `frontend/dist`；dist 缺失时依旧响亮失败。`ENVBOARD_PNPM` 可指定 pnpm 可执行文件。
//!
//! tauri.conf.json 不设 beforeBuildCommand：前端构建由本脚本持有，避免
//! `cargo tauri build` 时 pnpm 跑两遍。beforeDevCommand 只负责起 vite dev server。

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    // crates/app/desktop → 仓库根（上三级）→ frontend/
    let frontend = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .join("frontend");
    let dist = frontend.join("dist");

    // 前端源码 / 清单 / 配置 / 锁文件：任何一处变化都触发重新构建。
    for entry in [
        "src",
        "index.html",
        "package.json",
        "pnpm-lock.yaml",
        "vite.config.ts",
        "tsconfig.json",
    ] {
        println!("cargo::rerun-if-changed={}", frontend.join(entry).display());
    }
    println!("cargo::rerun-if-env-changed=ENVBOARD_SKIP_FRONTEND_BUILD");
    println!("cargo::rerun-if-env-changed=ENVBOARD_PNPM");

    if std::env::var_os("ENVBOARD_SKIP_FRONTEND_BUILD").is_some_and(|value| value == "1") {
        require_dist(
            &dist,
            "ENVBOARD_SKIP_FRONTEND_BUILD=1 要求既有 frontend/dist",
        );
    } else {
        let pnpm = std::env::var("ENVBOARD_PNPM").unwrap_or_else(|_| "pnpm".to_string());
        run_pnpm(
            &pnpm,
            &["install", "--frozen-lockfile", "--prefer-offline"],
            &frontend,
        );
        run_pnpm(&pnpm, &["build"], &frontend);
        require_dist(&dist, "pnpm build 没有产出 frontend/dist/index.html");
    }

    tauri_build::build();
}

/// dist 必须真的在：tauri-build 内嵌的是它，缺了就是构建被破坏。
fn require_dist(dist: &Path, context: &str) {
    if !dist.join("index.html").exists() {
        panic!(
            "{context} —— 前端构建产物缺失。\n  \
             正常路径：直接 cargo build（build.rs 自动跑 pnpm install + build）；\n  \
             手动排查：cd frontend && pnpm install --frozen-lockfile && pnpm build；\n  \
             工具链版本由 mise.toml 锁定（node/pnpm），pnpm 可用 ENVBOARD_PNPM 指定。"
        );
    }
}

/// 跑一条 pnpm 命令；失败响亮 panic（带命令与退出码，不静默跳过）。
fn run_pnpm(pnpm: &str, args: &[&str], cwd: &Path) {
    let status = Command::new(pnpm)
        .args(args)
        .current_dir(cwd)
        .status()
        .or_else(|error| {
            // Windows 上 npm 全局安装的 pnpm 是 .cmd shim，CreateProcess 拉不起 ——
            // 回退 cmd /C。mise 安装的 pnpm.exe 走上面的直接路径。
            if error.kind() == std::io::ErrorKind::NotFound && cfg!(windows) {
                Command::new("cmd")
                    .arg("/C")
                    .arg(pnpm)
                    .args(args)
                    .current_dir(cwd)
                    .status()
            } else {
                Err(error)
            }
        })
        .unwrap_or_else(|error| {
            panic!(
                "无法启动 pnpm（{pnpm}）：{error}\n  \
                 出路一：mise install（node/pnpm 版本由 mise.toml 锁定）；\n  \
                 出路二：ENVBOARD_PNPM=<pnpm 可执行文件路径> 重试。"
            )
        });
    if !status.success() {
        panic!(
            "前端构建失败：`{pnpm} {}`（cwd={}）退出码 {status}",
            args.join(" "),
            cwd.display()
        );
    }
}

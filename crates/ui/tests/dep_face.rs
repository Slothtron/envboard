//! 依赖面守门：ADR-1 的硬理由（“ui 不许依赖业务 crate”）必须由编译器和这条测试一起守。
//!
//! 编译器那半靠 `Cargo.toml` 里不写那行；这条测试防的是“有人补上了那行”——
//! 它会读自己的清单，发现 `[dependencies]` 里出现 `envboard-*` 就失败。
//! 只查 `[dependencies]`：`[dev-dependencies]` 允许（测试可以借别的东西做对照）。

const MANIFEST: &str = include_str!("../Cargo.toml");

fn section<'a>(text: &'a str, name: &str) -> Vec<&'a str> {
    let mut inside = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            inside = trimmed == format!("[{name}]");
            continue;
        }
        if inside && !trimmed.is_empty() && !trimmed.starts_with('#') {
            out.push(trimmed);
        }
    }
    out
}

#[test]
fn ui_does_not_depend_on_any_business_crate() {
    let deps = section(MANIFEST, "dependencies");
    assert!(!deps.is_empty(), "没读到 [dependencies]，测试本身失效了");
    for line in &deps {
        assert!(
            !line.contains("envboard-"),
            "envboard-ui 的 [dependencies] 里出现了业务 crate：`{line}`。\
             ui 是叶子，只认 NSH 基座；要读实例请去 crates/app/nsh。"
        );
    }
}

#[test]
fn ui_only_takes_the_nsh_base() {
    let deps = section(MANIFEST, "dependencies");
    let names: Vec<&str> = deps
        .iter()
        .map(|l| l.split('.').next().unwrap_or(l).trim())
        .collect();
    for name in ["nsh-base", "nsh-core"] {
        assert!(names.contains(&name), "缺基座依赖 `{name}`");
    }
    assert_eq!(
        names.len(),
        2,
        "ui 只该有 nsh-base / nsh-core 两条依赖，当前是 {names:?}"
    );
}

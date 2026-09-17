//! 注册表一致性门禁：契约表 ↔ 静态注册表 ↔ 内置插件实现 一一对应。
//!
//! 三方互为真相源是不可接受的（第二真相）；这条门禁把三方钉成同一份事实：
//!
//! 1. core/spec/capabilities.md 的「v3 插件与能力注册表」表格；
//! 2. envboard-core/src/plugin.rs 的 CAPABILITIES 静态数组（id/layer/phase/depends_on）；
//! 3. envboard-core/src/builtin.rs 里实现 Plugin::id 的内置插件（builtin 层的实现面）。

mod common;

use common::{report, workspace_root};

const SPEC: &str = "core/spec/capabilities.md";
const PLUGIN_RS: &str = "core/rs/crates/envboard-core/src/plugin.rs";
const BUILTIN_RS: &str = "core/rs/crates/envboard-core/src/builtin.rs";

/// 双引号字符。在字面量里逐次转义太容易出错，这里给一个常量。
const Q: &str = "\"";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Entry {
    id: String,
    layer: String,
    phase: String,
    depends: Vec<String>,
}

fn between(hay: &str, open: &str, closers: &[&str]) -> Option<String> {
    let start = hay.find(open)? + open.len();
    let rest = &hay[start..];
    let end = closers
        .iter()
        .filter_map(|closer| rest.find(closer))
        .min()
        .unwrap_or(rest.len());
    Some(rest[..end].trim().to_string())
}

/// 解析 plugin.rs 的 CAPABILITIES 静态数组（id 用 Rust 变体形状，归一在比对前）。
fn parse_static(text: &str) -> Vec<Entry> {
    let Some(marker) = text.find("pub static CAPABILITIES") else {
        return Vec::new();
    };
    let rest = &text[marker..];
    let Some(block_end) = rest.find("];") else {
        return Vec::new();
    };
    let block = &rest[..block_end];
    block
        .split("CapabilityDescriptor {")
        .skip(1)
        .filter_map(|chunk| {
            let body = chunk.split('}').next().unwrap_or(chunk);
            let id = between(body, &format!("id: {Q}"), &[Q])?;
            let layer = between(body, "layer: Layer::", &[",", " ", "}"])?;
            let phase = between(body, "phase: Phase::", &[",", " ", "}"])?;
            let deps_raw = between(body, "depends_on: &[", &["]"])?;
            let depends: Vec<String> = deps_raw
                .split('"')
                .skip(1)
                .step_by(2)
                .map(str::to_string)
                .collect();
            Some(Entry {
                id,
                layer,
                phase,
                depends,
            })
        })
        .collect()
}

fn layer_token(name: &str) -> Option<&'static str> {
    match name {
        "Kernel" => Some("kernel"),
        "Builtin" => Some("builtin"),
        "Extension" => Some("extension"),
        _ => None,
    }
}

fn phase_token(name: &str) -> Option<&'static str> {
    match name {
        "Startup" => Some("startup"),
        "Connect" => Some("connect"),
        "Request" => Some("request"),
        "Response" => Some("response"),
        "Log" => Some("log"),
        _ => None,
    }
}

/// 解析 capabilities.md 表格行（"—"/"-" 视作无依赖）。
fn parse_spec_table(text: &str) -> Vec<Entry> {
    let Some(marker) = text.find("## v3 插件与能力注册表") else {
        return Vec::new();
    };
    let rest = &text[marker..];
    let section_end = rest[3..]
        .find("\n## ")
        .map(|at| at + 3)
        .unwrap_or(rest.len());
    let section = &rest[..section_end];
    let mut out = Vec::new();
    for line in section.lines() {
        let line = line.trim();
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line
            .split('|')
            .map(str::trim)
            .filter(|cell| !cell.is_empty())
            .collect();
        if cells.len() < 4 || cells[0] == "id" || cells[0].starts_with("---") {
            continue;
        }
        let depends: Vec<String> = match cells[3] {
            "—" | "-" | "" => Vec::new(),
            other => other
                .split([' ', ','])
                .filter(|token| !token.is_empty() && *token != "—")
                .map(str::to_string)
                .collect(),
        };
        out.push(Entry {
            id: cells[0].to_string(),
            layer: cells[1].to_string(),
            phase: cells[2].to_string(),
            depends,
        });
    }
    out
}

/// builtin.rs 里实现的插件 id（按 id() 的返回字面量）。
fn parse_builtin_impls(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(at) = text[cursor..].find("fn id(&self) -> &'static str") {
        let start = cursor + at;
        let Some(end) = text[start..].find('}') else {
            break;
        };
        if let Some(lit) = between(&text[start..start + end], Q, &[Q]) {
            out.push(lit);
        }
        cursor = start + end;
    }
    out.sort();
    out
}

#[test]
fn registry_table_static_and_builtin_impls_agree() {
    let root = workspace_root();
    let mut problems = Vec::new();
    let spec_text = std::fs::read_to_string(root.join(SPEC)).unwrap_or_default();
    let static_text = std::fs::read_to_string(root.join(PLUGIN_RS)).unwrap_or_default();
    let builtin_text = std::fs::read_to_string(root.join(BUILTIN_RS)).unwrap_or_default();

    let mut from_static = parse_static(&static_text);
    let mut from_spec = parse_spec_table(&spec_text);
    if from_static.is_empty() {
        problems.push(
            "CAPABILITIES static not found in plugin.rs (parser or file drifted)".to_string(),
        );
    }
    if from_spec.is_empty() {
        problems.push("registry table not found in capabilities.md".to_string());
    }
    // 表用 spec 的小写 token 形状；静态用 Rust 变体名。归一到 spec 形状再比。
    for entry in &mut from_static {
        entry.layer = layer_token(&entry.layer)
            .map(str::to_string)
            .unwrap_or_else(|| entry.layer.clone());
        entry.phase = phase_token(&entry.phase)
            .map(str::to_string)
            .unwrap_or_else(|| entry.phase.clone());
    }
    from_static.sort();
    from_spec.sort();
    if from_static != from_spec {
        problems.push(format!(
            "registry drift: spec table and CAPABILITIES static differ\nspec={from_spec:?}\nstatic={from_static:?}"
        ));
    }

    // builtin 层的 id 必须与 builtin.rs 的实现一一对应。
    let mut expected: Vec<String> = from_spec
        .iter()
        .filter(|entry| entry.layer == "builtin")
        .map(|entry| entry.id.clone())
        .collect();
    expected.sort();
    let implemented = parse_builtin_impls(&builtin_text);
    if expected != implemented {
        problems.push(format!(
            "builtin layer drift: table says {expected:?}, builtin.rs implements {implemented:?}"
        ));
    }

    report("registry", problems, format!("{} entries", from_spec.len()));
}

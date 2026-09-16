//! 跨语言对拍：Rust 与注入器两份 hosts 解析实现必须输出**逐字节**一致。
//!
//! 为什么需要它：`core/spec/capabilities.md` 允许（甚至要求）两份实现并存 ——
//! Rust 管导入/规范化/落盘，Python 注入器管运行时解析。重复的代价必须由契约测试兜住。
//! `rules.md` 的 8 个 fixture 只覆盖常规路径，这里再补一批**对抗性输入**（BOM、孤儿 CR、
//! 制表符、注释截断、非法与超长 label、IPv6 各种写法、冲突、空文本、无尾换行…），
//! 因为这些才是两份实现容易分叉的地方。
//!
//! **已知分歧用白名单显式声明**，并做双向检查：未声明的分歧失败（真的 bug 或契约变更）；
//! 声明过的分歧消失了**也**失败（白名单过期会掩盖新分歧）。
//!
//! 这一层需要解释器（要跑注入器），所以标 `#[ignore]`：默认的
//! `cargo test --workspace` 因此不需要 Python；`ci/verify.sh` 的 adapter 层用
//! `--ignored` 显式触发它。解释器缺失时**响亮失败**，不静默跳过。
//!
//! 跑法：`cargo test -p envboard-rules --test dual_impl --locked --offline -- --ignored`
//! （可用 `ENVBOARD_PYTHON=<路径>` 指定解释器。）

use std::path::{Path, PathBuf};
use std::process::Command;

/// Rust 侧一次性入口：它就在本 crate 里，cargo 保证跑测试前已经构建好。
const RUST_BIN: &str = env!("CARGO_BIN_EXE_render_fixture");

/// `{用例: 原因}` —— 已知分歧。**现在是空的**，这正是目标状态：两份实现完全一致。
///
/// 保留这张表（而不是删掉机制）是因为它承载了一条纪律：出现分歧必须显式声明，
/// 且声明过期（分歧消失）同样失败 —— 否则白名单会慢慢变成"什么都放行"。
///
/// 历史条目：`v6_zone_id`（Python 的 `ipaddress` 接受带 zone id 的 IPv6、Rust 不接受）。
/// 注入器按契约显式拒绝 `%`，分歧消失，条目随之删除。
const KNOWN_DIVERGENCES: &[(&str, &str)] = &[];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .join("..")
        .canonicalize()
        .expect("repo root")
}

fn injector() -> PathBuf {
    repo_root().join("adapters/mitmproxy/envboard_mitmproxy.py")
}

/// 解释器：`ENVBOARD_PYTHON` → `python3` → `python` → **响亮失败**。
fn interpreter() -> String {
    if let Ok(configured) = std::env::var("ENVBOARD_PYTHON")
        && !configured.is_empty()
    {
        return configured;
    }
    for candidate in ["python3", "python"] {
        if Command::new(candidate)
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            return candidate.to_string();
        }
    }
    panic!(
        "对拍需要一个 Python 解释器来跑注入器，但 PATH 上没有 python3 / python。\n\
         两条出路：\n\
           1) 装适配器宿主（mitmproxy 自带一个解释器），或用 ENVBOARD_PYTHON=<路径> 指定；\n\
           2) 跑不吃解释器的子集：cargo test --workspace（本测试已标 #[ignore]）"
    );
}

fn adversarial() -> Vec<(&'static str, String)> {
    vec![
        ("empty", String::new()),
        ("only_nl", "\n".into()),
        ("three_nl", "\n\n\n".into()),
        ("crlf_only", "\r\n".into()),
        ("no_trailing_nl", "10.0.0.1 a.example.com".into()),
        ("trailing_nl", "10.0.0.1 a.example.com\n".into()),
        ("extra_nl", "10.0.0.1 a.example.com\n\n".into()),
        ("bom_once", "\u{feff}10.0.0.1 a.example.com".into()),
        ("bom_twice", "\u{feff}\u{feff}10.0.0.1 a.example.com".into()),
        (
            "crlf_pair",
            "10.0.0.1 a.example.com\r\n10.0.0.2 b.example.com\r\n".into(),
        ),
        (
            "lone_cr",
            "10.0.0.1 a.example.com\r10.0.0.2 b.example.com".into(),
        ),
        ("tabs_and_indent", "  10.0.0.1\ta.example.com  ".into()),
        (
            "trailing_comment",
            "10.0.0.1 a.example.com # trailing".into(),
        ),
        ("multi_hash", "10.0.0.1 a.example.com  # c1 # c2".into()),
        ("only_comment", "# only comment".into()),
        ("indented_comment", "   # indented".into()),
        ("blank_variants", "\t\n  \n".into()),
        ("too_few_ip", "10.0.0.1".into()),
        ("too_few_host", "a.example.com".into()),
        ("two_ips", "1.2.3.4 5.6.7.8".into()),
        ("ip_in_host_position", "10.0.0.1 300.1.2.3".into()),
        (
            "multi_host",
            "10.0.0.1 a.example.com b.example.com c.example.com".into(),
        ),
        (
            "host_first_multi",
            "a.example.com b.example.com 10.0.0.1".into(),
        ),
        ("wildcard_prefix", "10.0.0.1 *.wild.example.com".into()),
        ("double_wildcard", "10.0.0.1 *.*.a.example.com".into()),
        ("upper_and_root_dot", "10.0.0.1 API.Example.COM.".into()),
        ("leading_dash_label", "10.0.0.1 -bad.example.com".into()),
        ("trailing_dash_label", "10.0.0.1 bad-.example.com".into()),
        ("double_dot", "10.0.0.1 a..b".into()),
        ("underscore_label", "10.0.0.1 a_b.com".into()),
        ("dash_label", "10.0.0.1 a-b.com".into()),
        ("underscore_only", "10.0.0.1 _".into()),
        ("dot_only", "10.0.0.1 .".into()),
        (
            "label_63",
            format!("10.0.0.1 {}.example.com", "a".repeat(63)),
        ),
        (
            "label_64",
            format!("10.0.0.1 {}.example.com", "a".repeat(64)),
        ),
        ("non_ascii_label", "10.0.0.1 \u{c4}.example.com".into()),
        ("uppercase_host_only", "10.0.0.1 COM".into()),
        ("single_char_host", "10.0.0.1 a.example.com b".into()),
        ("v6_bare", "2001:db8::1 v6.example.com".into()),
        ("v6_uncanonical", "2001:0db8::1 v6.example.com".into()),
        ("v6_brackets", "[2001:db8::2] v6b.example.com".into()),
        ("v6_zone_id", "fe80::1%eth0 v6c.example.com".into()),
        (
            "ipv4_mapped_v6",
            "::ffff:10.0.0.1 mapped.example.com".into(),
        ),
        ("zero_address", "0.0.0.0 zero.example.com".into()),
        ("broadcast", "255.255.255.255 bcast.example.com".into()),
        ("leading_zero_v4", "010.0.0.1 a.example.com".into()),
        ("five_octets", "1.2.3.4.5 a.example.com".into()),
        (
            "conflict_two",
            "10.0.0.1 a.example.com\n10.0.0.2 a.example.com".into(),
        ),
        (
            "conflict_three",
            "10.0.0.1 a.example.com\n10.0.0.2 a.example.com\n10.0.0.3 a.example.com".into(),
        ),
        (
            "same_ip_twice",
            "10.0.0.1 a.example.com\n10.0.0.1 a.example.com".into(),
        ),
        ("split_host_and_ip", "10.0.0.1\na.example.com".into()),
        (
            "hash_inside_host",
            "10.0.0.1 a.example.com#b.example.com".into(),
        ),
        (
            "mixed_document",
            "10.0.0.1 a.example.com\nbad line here\n# comment\n\n\
             10.0.0.1 b.example.com 10.0.0.2 c.example.com\r\n"
                .into(),
        ),
        (
            "many_hosts_one_ip",
            format!(
                "10.0.0.9 {}",
                (0..20)
                    .map(|index| format!("h{index}.example.com"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        ),
        (
            "shared_ip_across_lines",
            "10.0.0.9 a.example.com\n10.0.0.9 b.example.com\n10.0.0.9 c.example.com".into(),
        ),
    ]
}

/// 把一段 hosts 文本落成临时 fixture，两侧各跑一次，比较退出码与 stdout **字节**。
fn compare(name: &str, text: &str, directory: &Path, interpreter: &str) -> Result<(), String> {
    let fixture = directory.join(format!("{name}.json"));
    let payload = serde_json::json!({
        "capability": "rules.parse",
        "description": name,
        "input": { "text": text },
    });
    std::fs::write(&fixture, payload.to_string()).expect("write temp fixture");

    let rust = Command::new(RUST_BIN)
        .arg(&fixture)
        .output()
        .expect("run render_fixture");
    let python = Command::new(interpreter)
        .arg(injector())
        .arg("--render")
        .arg("--fixture")
        .arg(&fixture)
        .output()
        .expect("run the injector");

    if rust.status.code() == python.status.code() && rust.stdout == python.stdout {
        return Ok(());
    }
    let preview =
        |bytes: &[u8]| String::from_utf8_lossy(&bytes[..bytes.len().min(200)]).into_owned();
    Err(format!(
        "rust(rc={:?}) {:?}\n        py  (rc={:?}) {:?}",
        rust.status.code(),
        preview(&rust.stdout),
        python.status.code(),
        preview(&python.stdout),
    ))
}

#[test]
#[ignore = "需要 Python 解释器跑注入器；由 ci/verify.sh 的 adapter 层用 --ignored 触发"]
fn both_hosts_parsers_are_byte_identical() {
    let root = repo_root();
    let interpreter = interpreter();
    let directory = std::env::temp_dir().join(format!("envboard-dual-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create temp dir");

    let mut problems = Vec::new();
    let mut diverged: Vec<&str> = Vec::new();
    let mut checked = 0;

    // 1) 契约 fixture：逐个取 input.text 走同一条对拍路径。
    let mut fixtures: Vec<PathBuf> = std::fs::read_dir(root.join("core/spec/fixtures/rules"))
        .expect("rules fixtures")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    fixtures.sort();
    for fixture in &fixtures {
        let raw = std::fs::read_to_string(fixture).expect("read fixture");
        let case: serde_json::Value = serde_json::from_str(&raw).expect("fixture JSON");
        let text = case["input"]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let name = format!(
            "fixture_{}",
            fixture.file_stem().unwrap_or_default().to_string_lossy()
        );
        checked += 1;
        if let Err(detail) = compare(&name, &text, &directory, &interpreter) {
            problems.push(format!("fixture {} diverged:\n        {detail}", name));
        }
    }

    // 2) 对抗性输入。
    for (name, text) in adversarial() {
        checked += 1;
        if let Err(detail) = compare(name, &text, &directory, &interpreter) {
            if KNOWN_DIVERGENCES.iter().any(|(known, _)| *known == name) {
                diverged.push(name);
                continue;
            }
            problems.push(format!(
                "case {name} diverged (undeclared):\n        {detail}"
            ));
        }
    }

    let _ = std::fs::remove_dir_all(&directory);

    // 双向检查：声明过的分歧若消失，白名单过期同样失败。
    for (name, _) in KNOWN_DIVERGENCES {
        if !diverged.contains(name) {
            problems.push(format!(
                "declared divergence {name:?} no longer diverges — remove it from \
                 KNOWN_DIVERGENCES (a stale allowlist can hide new divergences)"
            ));
        }
    }

    if !problems.is_empty() {
        println!("dual FAILED ({} problem(s)):", problems.len());
        for problem in &problems {
            println!("  - {problem}");
        }
        panic!("dual FAILED ({} problem(s))", problems.len());
    }

    println!(
        "dual OK ({checked} cases byte-identical, {} declared divergence(s))",
        diverged.len()
    );
}

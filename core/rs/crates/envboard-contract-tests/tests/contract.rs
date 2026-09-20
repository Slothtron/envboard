//! Rust 侧消费 `core/spec/fixtures` 的契约测试。
//!
//! 本文件是契约的**唯一**形状与语义宿主：按能力分发、只比对 `expect` 里出现的键、
//! 把失败用例的 `code` / `field` 当作断言，并额外钉住 fixture 本身的形状
//! （见 `every_fixture_pins_the_contract_shape`）。`rules.parse` 另有跨语言逐字节
//! 对拍：`envboard-rules` 的 `dual_impl` 测试。
//!
//! 注意：契约断言**不跟随实现**。改实现让测试通过之前，先问"契约该不该改"，
//! 该改就改 `core/spec/` 并同步 fixture。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use envboard_core_api::{ErrorCode, InstanceState};
use envboard_domain::{
    Desired, Environment, InstanceRecord, PortRequest, candidate_ports, occupancy_from,
    plan_reconcile, select_port,
};
use envboard_rules::{parse_hosts_text, render_rules};
use serde_json::{Value, json};

/// `<包根>/core/spec/fixtures`
fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../core/spec/fixtures")
}

/// 能力 → fixture 目录。这份注册表就是契约的一部分，改它要同时改 `core/spec/`。
const CAPABILITIES: &[(&str, &str)] = &[
    ("environment.validate", "environment"),
    ("environment.merge", "merge"),
    ("proxy.validate", "proxy"),
    ("rules.parse", "rules"),
    ("insecure.hosts", "insecure"),
    ("port.allocate", "ports"),
    ("instance.reconcile", "lifecycle"),
];

struct Case {
    path: PathBuf,
    capability: String,
    description: String,
    input: Value,
    expect: Value,
}

fn load_cases(directory: &str) -> Vec<Case> {
    let root = fixtures_root().join(directory);
    let capability = CAPABILITIES
        .iter()
        .find(|(_, dir)| *dir == directory)
        .map(|(capability, _)| (*capability).to_string())
        .expect("directory must be registered");

    let mut paths: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", root.display()))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();

    let mut cases = Vec::new();
    for path in paths {
        let raw = std::fs::read_to_string(&path).expect("fixture must be readable");
        let value: Value = serde_json::from_str(&raw)
            .unwrap_or_else(|error| panic!("{}: invalid JSON: {error}", path.display()));
        let declared = value["capability"].as_str().unwrap_or_default();
        assert_eq!(
            declared,
            capability,
            "{}: capability must match its directory",
            path.display()
        );
        assert!(
            value["description"]
                .as_str()
                .is_some_and(|text| !text.is_empty()),
            "{}: fixtures are read by humans; keep a description",
            path.display()
        );
        assert!(
            value.get("input").is_some(),
            "{}: missing input",
            path.display()
        );
        assert!(
            value["expect"].get("ok").is_some(),
            "{}: missing expect.ok",
            path.display()
        );
        cases.push(Case {
            path,
            capability: capability.to_string(),
            description: value["description"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            input: value["input"].clone(),
            expect: value["expect"].clone(),
        });
    }
    cases
}

/// 比对 `expect` 里出现的每个键（与 Python 侧同款语义：fixture 只钉它关心的字段）。
fn assert_fields(case: &Case, actual: &Value) {
    let expect = case.expect.as_object().expect("expect must be an object");
    for (key, expected) in expect {
        if key == "ok" {
            continue;
        }
        assert_eq!(
            actual.get(key),
            Some(expected),
            "{} [{}] ({}) : field {key:?}",
            case.path.display(),
            case.description,
            case.capability
        );
    }
}

fn assert_error(case: &Case, error: envboard_core_api::Error) {
    let expect = case.expect.as_object().expect("expect must be an object");
    assert_ne!(
        expect.get("ok"),
        Some(&Value::Bool(true)),
        "{}: expected success, got {error}",
        case.path.display()
    );
    if let Some(code) = expect.get("code").and_then(Value::as_str) {
        assert_eq!(
            error.code.as_str(),
            code,
            "{}: error code",
            case.path.display()
        );
    }
    if let Some(field) = expect.get("field").and_then(Value::as_str) {
        assert_eq!(
            error.field.as_deref(),
            Some(field),
            "{}: field path",
            case.path.display()
        );
    }
}

// --------------------------------------------------------------------------- //
// 各能力的驱动
// --------------------------------------------------------------------------- //

#[test]
fn environment_validate_matches_the_contract() {
    for case in load_cases("environment") {
        match Environment::from_json(&case.input) {
            Ok(environment) => {
                assert_ne!(
                    case.expect["ok"],
                    Value::Bool(false),
                    "{}: expected failure",
                    case.path.display()
                );
                assert_fields(&case, &environment.to_json());
            }
            Err(error) => assert_error(&case, error),
        }
    }
}

#[test]
fn environment_merge_matches_the_contract() {
    for case in load_cases("merge") {
        let base = Environment::from_json(&case.input["base"])
            .unwrap_or_else(|error| panic!("{}: base must be valid: {error}", case.path.display()));
        match base.merged(&case.input["patch"]) {
            Ok(merged) => {
                assert_ne!(
                    case.expect["ok"],
                    Value::Bool(false),
                    "{}: expected failure",
                    case.path.display()
                );
                assert_fields(&case, &merged.to_json());
            }
            Err(error) => assert_error(&case, error),
        }
    }
}

#[test]
fn proxy_validate_matches_the_contract() {
    for case in load_cases("proxy") {
        match envboard_domain::UpstreamProxy::from_json(&case.input) {
            Ok(proxy) => {
                assert_ne!(
                    case.expect["ok"],
                    Value::Bool(false),
                    "{}: expected failure",
                    case.path.display()
                );
                assert_fields(&case, &proxy.to_json());
            }
            Err(error) => assert_error(&case, error),
        }
    }
}

#[test]
fn port_allocate_matches_the_contract() {
    for case in load_cases("ports") {
        let input = case.input.as_object().expect("input must be an object");
        let u16_of = |key: &str| {
            input
                .get(key)
                .and_then(Value::as_u64)
                .and_then(|raw| u16::try_from(raw).ok())
        };
        let list_of = |key: &str| -> Vec<u16> {
            input
                .get(key)
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_u64)
                        .filter_map(|raw| u16::try_from(raw).ok())
                        .collect()
                })
                .unwrap_or_default()
        };
        let range = input.get("range").map(|range| {
            (
                range["min"].as_u64().unwrap_or(0) as u16,
                range["max"].as_u64().unwrap_or(u64::from(u16::MAX)) as u16,
            )
        });

        let request = PortRequest {
            existing: u16_of("existing"),
            requested: u16_of("requested"),
            candidates: list_of("candidates"),
            range: range.unwrap_or(envboard_domain::DEFAULT_PORT_RANGE),
            max_attempts: input
                .get("max_attempts")
                .and_then(Value::as_u64)
                .map(|raw| raw as usize)
                .unwrap_or(envboard_domain::DEFAULT_MAX_ATTEMPTS),
        };
        let busy: BTreeSet<u16> = list_of("taken").into_iter().collect();
        let mut occupied = occupancy_from(&busy);

        match select_port(&request, &mut occupied) {
            Ok(decision) => {
                assert_ne!(
                    case.expect["ok"],
                    Value::Bool(false),
                    "{}: expected failure",
                    case.path.display()
                );
                let actual = json!({
                    "port": decision.port,
                    "warnings": decision.warnings.iter().map(|warning| warning.as_str()).collect::<Vec<_>>(),
                });
                assert_fields(&case, &actual);
            }
            Err(error) => assert_error(&case, error),
        }
    }
}

#[test]
fn insecure_hosts_matches_the_contract() {
    for case in load_cases("insecure") {
        let input = case.input.as_object().expect("input must be an object");
        let hosts: Vec<String> = input["hosts"]
            .as_array()
            .expect("hosts must be an array")
            .iter()
            .map(|item| item.as_str().unwrap_or_default().to_string())
            .collect();
        let sni = input.get("sni").and_then(Value::as_str);
        let address = input.get("address").and_then(Value::as_str);
        let actual = json!({ "match": envboard_rules::insecure_matches(&hosts, sni, address) });
        assert_fields(&case, &actual);
    }
}

#[test]
fn instance_reconcile_matches_the_contract() {
    for case in load_cases("lifecycle") {
        let input = case.input.as_object().expect("input must be an object");
        let instances: Vec<InstanceRecord> = input["instances"]
            .as_array()
            .expect("instances must be an array")
            .iter()
            .map(|raw| InstanceRecord {
                env: raw["env"].as_str().unwrap_or_default().to_string(),
                desired: match raw["desired"].as_str() {
                    Some("running") => Desired::Running,
                    _ => Desired::Stopped,
                },
                live: raw["live"].as_bool().unwrap_or(false),
                listen_port: raw
                    .get("listen_port")
                    .and_then(Value::as_u64)
                    .map(|port| port as u16),
            })
            .collect();
        let occupied: BTreeSet<u16> = input
            .get("occupied_by_others")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_u64)
                    .map(|port| port as u16)
                    .collect()
            })
            .unwrap_or_default();

        let plan = plan_reconcile(&instances, &occupied);
        let actual = json!({
            "actions": plan.actions.iter().map(|(env, action)| json!([env, action.as_str()])).collect::<Vec<_>>(),
            "warnings": plan.warnings.iter().map(|warning| warning.as_str()).collect::<Vec<_>>(),
        });
        assert_fields(&case, &actual);
    }
}

#[test]
fn rules_parse_matches_the_contract_and_is_deterministic() {
    for case in load_cases("rules") {
        let text = case.input["text"].as_str().unwrap_or_default();
        let source = case
            .input
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let import = parse_hosts_text(text);

        let actual = json!({
            "entries": import.entries,
            "accepted": import.accepted(),
            "ips": import.ips(),
            "conflicts": import
                .conflicts
                .iter()
                .map(|conflict| json!([conflict.host, conflict.dropped, conflict.kept]))
                .collect::<Vec<_>>(),
            "skipped_reasons": import.skipped.iter().map(|item| item.reason.as_str()).collect::<Vec<_>>(),
            "skipped_details": import.skipped.iter().map(|item| item.detail.clone()).collect::<Vec<_>>(),
            "stats": import.stats_json(),
        });
        assert_fields(&case, &actual);

        // 契约里的确定性条款：渲染逐字节可复现，且 parse(render(x)) 稳定。
        let first = render_rules(&import.entries, source).expect("render must succeed");
        let reparsed = parse_hosts_text(&first);
        let second = render_rules(&reparsed.entries, source).expect("render must succeed");
        assert_eq!(
            first,
            second,
            "{}: rendering must be byte-stable",
            case.path.display()
        );
        assert_eq!(
            reparsed.entries,
            import.entries,
            "{}: parse(render(entries)) must round-trip",
            case.path.display()
        );
    }
}

// --------------------------------------------------------------------------- //
// 防呆：目录与注册表必须一致，否则"绿"没有意义
// --------------------------------------------------------------------------- //

#[test]
fn fixture_directories_match_the_capability_registry() {
    let root = fixtures_root();
    let mut present: Vec<String> = std::fs::read_dir(&root)
        .expect("fixtures root must exist")
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    present.sort();

    let mut declared: Vec<String> = CAPABILITIES
        .iter()
        .map(|(_, dir)| (*dir).to_string())
        .collect();
    declared.sort();

    assert_eq!(
        present, declared,
        "fixtures/ 下的目录与 capabilities 注册表必须一一对应（新增能力要同时改 \
         core/spec/capabilities.md 与这里）"
    );

    let total: usize = CAPABILITIES
        .iter()
        .map(|(_, dir)| load_cases(dir).len())
        .sum();
    assert_eq!(
        total, 79,
        "契约 fixture 总数变了：请同时更新 core/spec/ 与 README 里的数字"
    );
}

// --------------------------------------------------------------------------- //
// fixture 形状：确保"绿"不是靠空断言换来的
// --------------------------------------------------------------------------- //

/// `load_cases` 已经钉住了这些：`capability` 与所在目录一致、`description` 非空、
/// 有 `input`、有 `expect.ok`。这里补上它**没管**的两条 —— 它们才是"防呆"的要害：
///
/// 1. **失败用例必须钉住错误码**：只写 `ok: false` 的用例，实现回任何错误都能通过；
/// 2. **成功用例至少要钉一个归一化字段**：只写 `ok: true` 同理，等于没断言。
///
/// 另外断言每个 fixture 都是注册目录下的 `.json`（形状校验与 fixture 消费者同处一地，
/// 所以这条判据放在这里，而不是另立一个门禁）。
#[test]
fn every_fixture_pins_the_contract_shape() {
    let root = fixtures_root();
    let mut checked = 0;

    for (capability, directory) in CAPABILITIES {
        let dir = root.join(directory);
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", dir.display()))
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect();
        paths.sort();

        for path in paths {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            assert!(
                path.extension().is_some_and(|ext| ext == "json"),
                "{capability}: {} 只能是 .json 形式的 golden case",
                path.display()
            );
            checked += 1;

            let raw = std::fs::read_to_string(&path).expect("fixture must be readable");
            let value: Value = serde_json::from_str(&raw)
                .unwrap_or_else(|error| panic!("{}: invalid JSON: {error}", path.display()));
            let expect = value["expect"]
                .as_object()
                .unwrap_or_else(|| panic!("{}/{name}: expect must be an object", directory));

            match expect.get("ok").and_then(Value::as_bool) {
                Some(true) => assert!(
                    expect.len() >= 2,
                    "{capability}/{name}: 成功用例必须至少钉一个归一化字段（只有 ok 等于没断言）"
                ),
                Some(false) => assert!(
                    expect
                        .get("code")
                        .and_then(Value::as_str)
                        .is_some_and(|c| !c.is_empty()),
                    "{capability}/{name}: 失败用例必须钉住 expect.code（否则实现回任何错误都能通过）"
                ),
                None => panic!("{capability}/{name}: expect.ok must be a boolean"),
            }
        }
    }

    assert!(checked > 0, "一个 fixture 都没扫到：路径或注册表坏了");
}

#[test]
fn the_orchestration_surface_is_core_neutral() {
    // 一条"架构成立与否"的断言：管理器的端口抽象、错误码、状态词汇与
    // 能力表完全不依赖任何具体引擎实现 —— 只引用类型，证明编译期独立性。
    let capabilities = envboard_core_api::CoreCapabilities {
        listen: true,
        http1_only: true,
        ..Default::default()
    };
    assert!(capabilities.listen);
    assert_eq!(ErrorCode::PortConflict.as_str(), "port_conflict");
    assert_eq!(InstanceState::Stopped.as_str(), "stopped");
    let candidates = candidate_ports((16_000, 16_002), &BTreeSet::new(), 7);
    assert_eq!(candidates.len(), 3);
    let _: BTreeMap<String, String> = BTreeMap::new();
}

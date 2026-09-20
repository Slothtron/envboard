//! 管理器的行为测试（v3）—— 不依赖任何外部进程/解释器（「不装 mitmproxy 也能
//! 跑默认流水线」的验收条件在 v3 变成默认形态本身）。
//!
//! v2 的这套测试逐条迁移（暂存文件 manager_lifecycle.rs.pending-migration 是清单）：
//! 断言**搬家不减少** —— 机制随子进程模型退场的（状态文件新鲜度、收敛窗口、配置
//! 回显比对、PID 复用、上一代重启），其**目的**由 v3 等价断言接替（unhealthy 来自
//! 引擎报告、apply 被拒保留旧快照、失败引擎由 reconcile 重启）。每条接替在测试名与
//! 注释里写明对应关系。
//!
//! 组合仍是"真文件系统 + 真 socket + FakeEngine"：状态持久化、端口分配与绑定、
//! 冲突重试这些最容易写错的部分，用内存替身反而测不到。

use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};

use envboard_core_api::{
    ClockPort, ErrorCode, InstanceState, LogLevel, LoggerPort, NullLogger, ProxyEngine,
};
use envboard_core_fake::FakeEngine;
use envboard_manager::infra::{RealFiles, SocketPortProbe};
use envboard_manager::{
    EnvView, JsonFileStateRepo, Manager, ManagerConfig, PersistedState, PortProbe, StateRepo,
};

// --------------------------------------------------------------------------- //
// 测试脚手架
// --------------------------------------------------------------------------- //

/// 每个测试一段独立的端口窗口，避免并行测试互相抢端口。
static NEXT_WINDOW: AtomicU16 = AtomicU16::new(21_000);

/// 手动时钟 —— 账本时间戳完全确定。
#[derive(Debug)]
struct SettableClock {
    now: std::sync::atomic::AtomicU64,
}

impl SettableClock {
    fn new(now: u64) -> Self {
        Self {
            now: std::sync::atomic::AtomicU64::new(now),
        }
    }
}

impl ClockPort for SettableClock {
    fn now_unix(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
}

/// 记录日志以便断言（重试、热应用、失败都必须可见）。
#[derive(Debug, Default)]
struct RecordingLogger {
    lines: std::sync::Mutex<Vec<String>>,
}

impl LoggerPort for RecordingLogger {
    fn log(&self, level: LogLevel, message: &str) {
        self.lines
            .lock()
            .unwrap()
            .push(format!("{}: {message}", level.as_str()));
    }
}

struct Harness {
    manager: Manager,
    directory: std::path::PathBuf,
    clock: Arc<SettableClock>,
    logger: Arc<RecordingLogger>,
    repo: Arc<JsonFileStateRepo>,
    engine: Arc<FakeEngine>,
}

impl Harness {
    fn new() -> Self {
        Self::build()
    }

    fn build() -> Self {
        let window = NEXT_WINDOW.fetch_add(40, Ordering::SeqCst);
        let directory =
            std::env::temp_dir().join(format!("envboard-test-{}-{window}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();

        let mut config = ManagerConfig::new(&directory);
        config.port_range = (window, window + 39);
        config.listen_host = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

        let clock = Arc::new(SettableClock::new(1_000_000));
        let logger = Arc::new(RecordingLogger::default());
        let repo = Arc::new(JsonFileStateRepo::new(config.state_file()));
        let engine = Arc::new(FakeEngine::new());
        let manager = Manager::new(
            config,
            Arc::clone(&engine) as Arc<dyn ProxyEngine>,
            Arc::new(SocketPortProbe),
            Arc::new(RealFiles),
            Arc::clone(&clock) as Arc<dyn ClockPort>,
            Arc::clone(&logger) as Arc<dyn LoggerPort>,
            Arc::clone(&repo) as Arc<dyn StateRepo>,
            false, // 测试里不抢目录锁
        )
        .unwrap();

        Self {
            manager,
            directory,
            clock,
            logger,
            repo,
            engine,
        }
    }

    /// FakeEngine 的注入旋钮（错误契约两档、失败重启路径）。
    fn engine(&self) -> &Arc<FakeEngine> {
        &self.engine
    }

    /// 在同一个 state_dir 上再起一个管理器（模拟重启）。每次都是全新的
    /// FakeEngine：进程内实例不跨管理器存活，这正是 v3 的既定语义。
    fn standalone(&self) -> Manager {
        let engine: Arc<dyn ProxyEngine> = Arc::new(FakeEngine::new());
        Manager::new(
            self.manager.config().clone(),
            engine,
            Arc::new(SocketPortProbe),
            Arc::new(RealFiles),
            self.clock.clone() as Arc<dyn ClockPort>,
            // 共享日志记录器：standalone 上的热应用等动作必须可断言。
            self.logger.clone() as Arc<dyn LoggerPort>,
            Arc::new(JsonFileStateRepo::new(self.manager.config().state_file()))
                as Arc<dyn StateRepo>,
            false,
        )
        .unwrap()
    }

    fn saved(&self) -> PersistedState {
        self.repo.load().unwrap()
    }

    fn log_lines(&self) -> Vec<String> {
        self.logger.lines.lock().unwrap().clone()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.directory).ok();
    }
}

fn create_beta(harness: &Harness) -> EnvView {
    harness
        .manager
        .create(&serde_json::json!({"name": "beta", "description": "灰度"}))
        .unwrap()
}

fn create_beta_with(harness: &Harness, extra: serde_json::Value) -> EnvView {
    let mut input = serde_json::json!({"name": "beta", "description": "灰度"});
    for (key, value) in extra.as_object().expect("extra must be an object") {
        input[key] = value.clone();
    }
    harness.manager.create(&input).unwrap()
}

fn assert_in_range(view: &EnvView, harness: &Harness) {
    let (min, max) = harness.manager.config().port_range;
    assert!(
        view.listen.port >= min && view.listen.port <= max,
        "port {} outside test range {min}-{max}",
        view.listen.port
    );
}

/// 直接改状态文件 —— 构造"上一次运行留下的状态"（legacy 场景）。
/// 管理器缓存状态于内存（单一写者），改完必须另起管理器才看得到。
fn prepare_state(harness: &Harness, prepare: impl FnOnce(&mut PersistedState)) {
    let mut state = harness.saved();
    prepare(&mut state);
    harness.repo.save(&state).unwrap();
}

// --------------------------------------------------------------------------- //
// 端口与创建
// --------------------------------------------------------------------------- //

#[test]
fn create_allocates_a_free_port_in_range_and_persists_it() {
    let harness = Harness::new();
    let beta = create_beta(&harness);
    assert_in_range(&beta, &harness);
    assert_eq!(beta.desired, envboard_domain::Desired::Stopped);
    assert!(!beta.proxy_command.is_empty(), "工作台要给出可复制的一行");

    let prod = harness
        .manager
        .create(&serde_json::json!({"name": "prod"}))
        .unwrap();
    assert_ne!(beta.listen.port, prod.listen.port);
    assert_in_range(&prod, &harness);

    let state = harness.saved();
    assert_eq!(state.environments.len(), 2);
    assert!(state.find("beta").is_some());
    assert_eq!(state.auto_port.get("beta"), Some(&true));
}

#[test]
fn create_keeps_an_explicit_port_and_marks_it_as_not_auto() {
    let harness = Harness::new();
    let (min, _) = harness.manager.config().port_range;
    let view = harness
        .manager
        .create(&serde_json::json!({"name": "beta", "listen": {"port": min + 7}}))
        .unwrap();
    assert_eq!(view.listen.port, min + 7);
    assert_eq!(harness.saved().auto_port.get("beta"), Some(&false));
}

#[test]
fn create_rejects_duplicate_names_and_bad_input() {
    let harness = Harness::new();
    create_beta(&harness);
    let duplicate = harness
        .manager
        .create(&serde_json::json!({"name": "BETA"}))
        .unwrap_err();
    assert_eq!(duplicate.code, ErrorCode::Conflict);

    let bad = harness
        .manager
        .create(&serde_json::json!({"name": "9bad", "listen": {"port": 1234}}))
        .unwrap_err();
    assert_eq!(bad.field.as_deref(), Some("environment.name"));

    let legacy = harness
        .manager
        .create(&serde_json::json!({"name": "old", "dns_servers": ["10.0.0.53"]}))
        .unwrap_err();
    assert_eq!(legacy.field.as_deref(), Some("environment.dns_servers"));
}

// --------------------------------------------------------------------------- //
// 启停与健康（v3：健康 = 引擎内存报告；接替 v2 的状态文件三兄弟）
// --------------------------------------------------------------------------- //

#[tokio::test]
async fn start_and_stop_round_trip_updates_desired_state_and_health() {
    let harness = Harness::new();
    let beta = create_beta(&harness);

    let started = harness.manager.start("beta").await.unwrap();
    assert_eq!(started.desired, envboard_domain::Desired::Running);
    assert_eq!(
        harness.manager.health("beta").await.unwrap(),
        InstanceState::Running
    );

    // 端口真的在监听（真 socket，不是替身）
    let addr = std::net::SocketAddr::new(beta.listen.host, beta.listen.port);
    assert!(
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500)).is_ok()
    );

    let stopped = harness.manager.stop("beta").await.unwrap();
    assert_eq!(stopped.desired, envboard_domain::Desired::Stopped);
    assert_eq!(
        harness.manager.health("beta").await.unwrap(),
        InstanceState::Stopped
    );
    // 实例随 stop 消失：端口不再接受连接（v2 里这条由"状态文件被删除"表达）。
    assert!(
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(200)).is_err()
    );
}

/// 接替 v2 的 stale_status_file → unhealthy：v3 的 unhealthy 来自**引擎自己的报告**
/// （监听面异常等），带原因上抛。
#[tokio::test]
async fn an_unhealthy_engine_reports_its_reason() {
    let harness = Harness::new();
    create_beta(&harness);
    harness.manager.start("beta").await.unwrap();
    assert!(harness.engine().inject_state(
        "beta",
        InstanceState::Unhealthy {
            reason: "accept loop degraded".to_string(),
        },
    ));

    match harness.manager.health("beta").await.unwrap() {
        InstanceState::Unhealthy { reason } => assert!(reason.contains("degraded"), "{reason}"),
        other => panic!("expected unhealthy, got {other:?}"),
    }
    // 列表视图与权威判定同源（v2 病根的结构性反证）。
    assert_eq!(
        harness.manager.get("beta").unwrap().health.as_str(),
        "unhealthy"
    );
}

/// 接替 v2 的 missing_status_file（"TCP 可连不等于在跑"）：v3 的真相是内存报告 ——
/// 引擎线程死了（failed）即使端口曾属于它，也如实报 failed，期望保持 running。
#[tokio::test]
async fn a_failed_engine_is_visible_not_guessed_from_ports() {
    let harness = Harness::new();
    create_beta(&harness);
    harness.manager.start("beta").await.unwrap();
    assert!(harness.engine().inject_state(
        "beta",
        InstanceState::Failed {
            reason: "engine thread panicked".to_string(),
        },
    ));

    let view = harness.manager.get("beta").unwrap();
    assert_eq!(view.health.as_str(), "failed");
    assert_eq!(
        view.desired,
        envboard_domain::Desired::Running,
        "自己死的实例期望不变：等 reconcile 拉回来"
    );
}

// --------------------------------------------------------------------------- //
// 热应用的两档错误契约（接替 v2 的 config_echo_mismatch / 收敛窗 / config_error 三案）
// --------------------------------------------------------------------------- //

#[tokio::test]
async fn apply_rejection_keeps_the_old_snapshot_and_marks_the_environment() {
    let harness = Harness::new();
    create_beta(&harness);
    harness.manager.start("beta").await.unwrap();

    // 装配被拒：整套拒绝、旧快照继续服务、必须可见（禁止静默降级）。
    harness
        .engine()
        .set_apply_failure(Some("bad insecure host: *.wildcard"));
    let updated = harness
        .manager
        .update(
            "beta",
            &serde_json::json!({"insecure_hosts": ["api.example.com"]}),
        )
        .unwrap();
    assert_eq!(updated.insecure_hosts, ["api.example.com"], "账本记录期望");
    match updated.health {
        InstanceState::Unhealthy { reason } => {
            assert!(
                reason.contains("previous snapshot") && reason.contains("wildcard"),
                "{reason}"
            );
        }
        other => panic!("expected unhealthy mark, got {other:?}"),
    }
    assert!(
        harness
            .log_lines()
            .iter()
            .any(|line| line.contains("apply failed, previous snapshot still serving")),
        "{:?}",
        harness.log_lines()
    );

    // 修好配置再改一次：应用成功必须把标记**清掉**（旧失败不该继续缠着环境）。
    harness.engine().set_apply_failure(None);
    let healed = harness
        .manager
        .update(
            "beta",
            &serde_json::json!({"insecure_hosts": ["fixed.example.com"]}),
        )
        .unwrap();
    assert_eq!(healed.health, InstanceState::Running);
    assert_eq!(healed.insecure_hosts, ["fixed.example.com"]);
}

/// v2 的"刚写完配置的收敛窗口内不误报"在 v3 不存在（同步装配没有窗口）；
/// 它的**目的**由这条接替：回执与期望同源 —— 应用成功后引擎报告里的 hash
/// 就是账本这份配置算出来的（无"回显不符"的介质）。
#[tokio::test]
async fn a_hot_apply_succeeds_synchronously_and_is_logged() {
    let harness = Harness::new();
    create_beta(&harness);
    harness.manager.start("beta").await.unwrap();
    harness
        .manager
        .update("beta", &serde_json::json!({"insecure_hosts": ["a.test"]}))
        .unwrap();
    let lines = harness.log_lines();
    assert!(
        lines
            .iter()
            .any(|line| line.contains("hot-applied configuration")),
        "热应用必须留痕: {lines:?}"
    );
    assert_eq!(
        harness.manager.get("beta").unwrap().health.as_str(),
        "running"
    );
}

// --------------------------------------------------------------------------- //
// 规则账本（v3：rendered 直供引擎；链与 config.json 已退场）
// --------------------------------------------------------------------------- //

#[tokio::test]
async fn binding_an_unknown_rules_name_is_refused_when_it_is_written() {
    let harness = Harness::new();
    let error = harness
        .manager
        .create(&serde_json::json!({"name": "beta", "rules": "nope"}))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound);
    assert_eq!(error.field.as_deref(), Some("environment.rules"));
    assert!(
        harness.manager.get("beta").is_err(),
        "被拒绝的环境不得留下持久化痕迹"
    );

    harness
        .manager
        .import_rules("nope", "10.0.0.11 api.example.com\n")
        .unwrap();
    let created = harness
        .manager
        .create(&serde_json::json!({"name": "beta", "rules": "nope"}))
        .unwrap();
    assert_eq!(created.rules_count, 1);
    assert!(!created.rules_missing);
    let started = harness.manager.start("beta").await.unwrap();
    assert_eq!(started.health.as_str(), "running");
}

/// 接替"软链断了=不覆盖"：v3 没有链，**账本里没有那条规则**才会发生
/// rules_missing（典型来源是 v2 状态文件里绑着已丢失名字的 legacy 环境）。
/// 语义不变：不炸启动、不静默 —— 视图明说；导入即热应用把覆盖补回来。
#[tokio::test]
async fn a_legacy_binding_without_the_ledger_entry_is_visible_not_fatal() {
    let harness = Harness::new();
    harness
        .manager
        .create(&serde_json::json!({"name": "beta"}))
        .unwrap();
    // 手工把绑定写进状态（模拟 v2 状态里绑着已丢失的 "ghost"）。
    prepare_state(&harness, |state| {
        for raw in state.environments.iter_mut() {
            if raw.get("name").and_then(serde_json::Value::as_str) == Some("beta") {
                raw["rules"] = serde_json::json!("ghost");
            }
        }
    });

    // 状态文件是"上一次写者留下的事实"：当前管理器缓存自己的视图，
    // 读改动必须另起管理器（单一写者纪律，flock 在真部署里保证互斥）。
    let sm = harness.standalone();
    let view = sm.get("beta").unwrap();
    assert!(view.rules_missing, "视图必须说清绑定已不在账本");
    assert_eq!(view.rules_count, 0);

    // 启动不因它失败（v2 语义保留：缺失即不覆盖，不是响亮失败）。
    let started = sm.start("beta").await.unwrap();
    assert_eq!(started.health.as_str(), "running");

    // 补导入同名规则 → 绑定立刻生效且热应用到在跑的实例。
    sm.import_rules("ghost", "10.0.0.11 api.example.com\n")
        .unwrap();
    let fixed = sm.get("beta").unwrap();
    assert!(!fixed.rules_missing);
    assert_eq!(fixed.rules_count, 1);
    assert!(
        harness
            .log_lines()
            .iter()
            .any(|line| line.contains("hot-applied configuration")),
        "导入必须对在跑实例热应用"
    );
}

#[test]
fn the_rules_ledger_backfills_from_the_directory_and_heals_the_files() {
    let harness = Harness::new();
    harness
        .manager
        .import_rules(
            "beta",
            "10.0.0.11 api.example.com\n10.0.0.12 b.example.com\n",
        )
        .unwrap();
    let path = harness.manager.config().rules_path("beta");
    let rendered = std::fs::read_to_string(&path).unwrap();

    // 升级迁移：账本没有这段 → 对账从物化目录回填。
    prepare_state(&harness, |state| state.rules.clear());
    let reopened = harness.standalone();
    assert_eq!(
        reopened.rules_list().unwrap(),
        vec!["beta".to_string()],
        "物化文件必须回填进账本"
    );
    assert_eq!(reopened.rules_read("beta").unwrap(), rendered);

    // 自愈：物化文件被改坏 → 按账本正文逐字节重建。
    std::fs::write(&path, "garbage\n").unwrap();
    let messages = reopened.reconcile_rules().unwrap();
    assert!(
        messages.iter().any(|line| line.contains("rebuilt")),
        "got {messages:?}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), rendered);

    // 正文以账本为准：物化文件没了也答得出来。
    std::fs::remove_file(&path).unwrap();
    assert_eq!(reopened.rules_read("beta").unwrap(), rendered);
}

#[test]
fn rules_delete_refuses_a_bound_rule() {
    let harness = Harness::new();
    harness
        .manager
        .import_rules("beta", "10.0.0.11 api.example.com\n")
        .unwrap();
    harness
        .manager
        .create(&serde_json::json!({"name": "beta", "rules": "beta"}))
        .unwrap();

    let error = harness.manager.rules_delete("beta").unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);

    harness
        .manager
        .update("beta", &serde_json::json!({"rules": null}))
        .unwrap();
    harness.manager.rules_delete("beta").unwrap();
    assert!(harness.manager.rules_list().unwrap().is_empty());
    assert!(!harness.manager.config().rules_path("beta").exists());
}

#[test]
fn import_rules_normalizes_writes_and_tightens_permissions() {
    let harness = Harness::new();
    let messy = "b.example.com 10.0.0.2\n10.0.0.1 a.example.com\n# comment\nnonsense\n";
    let path = harness.manager.import_rules("beta", messy).unwrap();

    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.contains("10.0.0.1 a.example.com"));
    assert!(written.contains("10.0.0.2 b.example.com"));
    assert!(!written.contains("nonsense"));
    assert!(written.ends_with('\n'));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "规则文件是导入状态，保持私有");
    }

    let error = harness.manager.import_rules("../escape", "").unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidConfig);
}

// --------------------------------------------------------------------------- //
// 端口冲突的两条规则不得混用
// --------------------------------------------------------------------------- //

#[tokio::test]
async fn explicit_port_conflict_never_reallocates_silently() {
    let harness = Harness::new();
    let (min, _) = harness.manager.config().port_range;
    let port = min + 3;
    harness
        .manager
        .create(&serde_json::json!({"name": "beta", "listen": {"port": port}}))
        .unwrap();

    let squatter = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();

    let error = harness.manager.start("beta").await.unwrap_err();
    assert_eq!(error.code, ErrorCode::PortConflict);

    let view = harness.manager.get("beta").unwrap();
    assert_eq!(view.listen.port, port, "显式端口绝不静默重分配");
    assert_eq!(view.health.as_str(), "port_conflict");
    assert_eq!(view.desired, envboard_domain::Desired::Running);
    drop(squatter);
}

#[tokio::test]
async fn auto_allocated_port_conflict_retries_once_with_a_new_port() {
    let harness = Harness::new();
    let beta = create_beta(&harness);

    let squatter = std::net::TcpListener::bind(("127.0.0.1", beta.listen.port)).unwrap();
    let started = harness.manager.start("beta").await.unwrap();

    assert_ne!(started.listen.port, beta.listen.port, "重试必须换端口");
    assert_eq!(started.health.as_str(), "running");
    assert!(
        harness
            .log_lines()
            .iter()
            .any(|line| line.contains("retrying once")),
        "重试必须留痕: {:?}",
        harness.log_lines()
    );
    drop(squatter);
}

#[test]
fn the_options_passthrough_is_gone_and_its_tombstone_is_accepted() {
    let harness = Harness::new();
    let error = harness
        .manager
        .create(&serde_json::json!({
            "name": "beta",
            "options": {"ssl_insecure": "true"},
        }))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidConfig);
    assert_eq!(error.field.as_deref(), Some("environment.options"));
    assert!(
        error.message.contains("insecure_hosts"),
        "错误必须指出替代路径: {}",
        error.message
    );
    assert!(harness.manager.get("beta").is_err());

    // 空对象是墓碑：旧版本每环境都写 options:{}，一律拒绝会让升级卡死。
    let created = harness
        .manager
        .create(&serde_json::json!({"name": "beta", "options": {}}))
        .unwrap();
    assert_eq!(created.name, "beta");
    assert!(
        harness
            .saved()
            .find("beta")
            .unwrap()
            .get("options")
            .is_none(),
        "墓碑不得写回"
    );
}

// --------------------------------------------------------------------------- //
// 热字段 / 停机字段（PATCH 矩阵）
// --------------------------------------------------------------------------- //

#[tokio::test]
async fn insecure_hosts_are_persisted_and_hot_editable() {
    let harness = Harness::new();
    let created = create_beta_with(
        &harness,
        serde_json::json!({"insecure_hosts": ["app.example.com"]}),
    );
    assert_eq!(created.insecure_hosts, ["app.example.com"]);

    let started = harness.manager.start("beta").await.unwrap();
    assert_eq!(started.health.as_str(), "running");

    // 放行清单是热的：v3 的"热"= 一次同步 apply（v2 靠轮询文件）。
    let updated = harness
        .manager
        .update(
            "beta",
            &serde_json::json!({"insecure_hosts": ["web.example.net", "app.example.com"]}),
        )
        .unwrap();
    assert_eq!(
        updated.insecure_hosts,
        ["app.example.com", "web.example.net"]
    );
    assert_eq!(updated.health.as_str(), "running");

    let state = harness.saved();
    assert_eq!(
        state.find("beta").unwrap()["insecure_hosts"],
        serde_json::json!(["app.example.com", "web.example.net"])
    );

    // 凭据是停机字段（v2 划分保留：v3 凭据已不经 argv，但字段矩阵不漂移）。
    let error = harness
        .manager
        .update(
            "beta",
            &serde_json::json!({"proxy_user": "alice", "proxy_password": "s3cret"}),
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);

    harness.manager.stop("beta").await.unwrap();
    let with_credentials = harness
        .manager
        .update(
            "beta",
            &serde_json::json!({"proxy_user": "alice", "proxy_password": "s3cret"}),
        )
        .unwrap();
    assert!(with_credentials.proxy_auth_enabled);
    assert_eq!(with_credentials.health.as_str(), "stopped");
    // 视图/SSE 广播绝不能回显凭据。
    assert!(
        !serde_json::to_string(&with_credentials.to_json())
            .unwrap()
            .contains("s3cret")
    );
    // 明文只落在 0600 的状态文件里（v3 不再出现在任何启动参数/argv）。
    assert_eq!(
        harness.saved().find("beta").unwrap()["proxy_password"],
        serde_json::json!("s3cret")
    );
}

#[tokio::test]
async fn description_and_the_rules_binding_are_hot() {
    let harness = Harness::new();
    harness
        .manager
        .import_rules("beta", "10.0.0.11 api.example.com\n")
        .unwrap();
    harness
        .manager
        .import_rules(
            "gamma",
            "10.0.0.12 b.example.com\n10.0.0.13 c.example.com\n",
        )
        .unwrap();
    harness
        .manager
        .create(&serde_json::json!({"name": "beta", "description": "旧的"}))
        .unwrap();
    harness.manager.start("beta").await.unwrap();

    // 运行中改描述：允许且不重启。
    let view = harness
        .manager
        .update("beta", &serde_json::json!({"description": "灰度 v2"}))
        .unwrap();
    assert_eq!(view.description, "灰度 v2");
    assert_eq!(view.health.as_str(), "running", "热改不得重启实例");

    // 运行中换绑定：条数立刻跟着变（v2 靠换链+轮询；v3 靠 apply 回执）。
    let bound = harness
        .manager
        .update("beta", &serde_json::json!({"rules": "beta"}))
        .unwrap();
    assert_eq!(bound.rules_count, 1);
    assert!(!bound.rules_missing);
    assert_eq!(bound.health.as_str(), "running");

    let switched = harness
        .manager
        .update("beta", &serde_json::json!({"rules": "gamma"}))
        .unwrap();
    assert_eq!(switched.rules_count, 2);
    assert_eq!(switched.description, "灰度 v2", "没提的字段原样保持");

    // 解绑是显式语义："不覆盖"。
    let unbound = harness
        .manager
        .update("beta", &serde_json::json!({"rules": null}))
        .unwrap();
    assert!(unbound.rules.is_none());
    assert!(!unbound.rules_missing);
    assert_eq!(unbound.rules_count, 0);
}

#[tokio::test]
async fn update_refuses_identity_changes_while_running_and_applies_them_when_stopped() {
    let harness = Harness::new();
    let beta = create_beta(&harness);
    harness.manager.start("beta").await.unwrap();

    for patch in [
        serde_json::json!({"listen": {"port": beta.listen.port + 1}}),
        serde_json::json!({"name": "gamma"}),
        serde_json::json!({"proxy_user": "alice", "proxy_password": "s3cret"}),
    ] {
        let error = harness.manager.update("beta", &patch).unwrap_err();
        assert_eq!(error.code, ErrorCode::Conflict, "patch: {patch}");
        assert!(error.message.contains("stop it"), "got: {}", error.message);
    }
    assert_eq!(harness.manager.get("beta").unwrap().name, "beta");

    harness.manager.stop("beta").await.unwrap();
    let renamed = harness
        .manager
        .update(
            "beta",
            &serde_json::json!({"name": "gamma", "listen": {"port": beta.listen.port + 1}}),
        )
        .unwrap();
    assert_eq!(renamed.name, "gamma");
    assert_eq!(renamed.listen.port, beta.listen.port + 1);
    assert!(harness.manager.get("beta").is_err(), "旧名字消失");

    let state = harness.saved();
    assert!(state.find("beta").is_none());
    assert!(state.find("gamma").is_some());
    assert_eq!(
        state.desired.get("gamma"),
        Some(&envboard_domain::Desired::Stopped)
    );
    assert!(!state.desired.contains_key("beta"));
    assert_eq!(state.auto_port.get("gamma"), Some(&true));
    assert!(
        harness
            .manager
            .get("gamma")
            .unwrap()
            .proxy_command
            .contains(&(beta.listen.port + 1).to_string()),
        "可复制的一行必须跟着新端口"
    );

    harness
        .manager
        .create(&serde_json::json!({"name": "prod"}))
        .unwrap();
    let error = harness
        .manager
        .update("prod", &serde_json::json!({"name": "gamma"}))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);
}

#[tokio::test]
async fn update_drops_a_mark_that_no_longer_applies() {
    let harness = Harness::new();
    let (min, _) = harness.manager.config().port_range;
    let port = min + 11;
    harness
        .manager
        .create(&serde_json::json!({"name": "beta", "listen": {"port": port}}))
        .unwrap();

    let squatter = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    let error = harness.manager.start("beta").await.unwrap_err();
    assert_eq!(error.code, ErrorCode::PortConflict);
    assert_eq!(
        harness.manager.get("beta").unwrap().health.as_str(),
        "port_conflict"
    );

    // 换到空位后旧标记必须消失 —— 否则界面拿新端口报旧冲突，等于说谎。
    let free = min + 12;
    harness
        .manager
        .update("beta", &serde_json::json!({"listen": {"port": free}}))
        .unwrap();
    let view = harness.manager.get("beta").unwrap();
    assert_eq!(view.listen.port, free);
    assert_ne!(view.health.as_str(), "port_conflict");
    drop(squatter);

    assert_eq!(
        harness.manager.start("beta").await.unwrap().health.as_str(),
        "running"
    );
}

#[test]
fn update_rejects_an_unknown_field_and_a_renamed_legacy_one() {
    let harness = Harness::new();
    create_beta(&harness);
    let error = harness
        .manager
        .update("beta", &serde_json::json!({"dns_servers": ["10.0.0.53"]}))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidConfig);
    assert_eq!(error.field.as_deref(), Some("environment.dns_servers"));
}

// --------------------------------------------------------------------------- //
// 删除
// --------------------------------------------------------------------------- //

#[tokio::test]
async fn remove_requires_a_stopped_environment() {
    let harness = Harness::new();
    create_beta(&harness);
    harness.manager.start("beta").await.unwrap();

    let error = harness.manager.remove("beta").unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);

    harness.manager.stop("beta").await.unwrap();
    harness.manager.remove("beta").unwrap();
    assert_eq!(harness.saved().environments.len(), 0);
    assert!(harness.manager.get("beta").is_err());
}

#[test]
fn removing_an_environment_sweeps_v2_residue_but_never_the_rules_library() {
    let harness = Harness::new();
    harness
        .manager
        .import_rules("beta", "10.0.0.11 api.example.com\n")
        .unwrap();
    harness
        .manager
        .create(&serde_json::json!({"name": "beta", "rules": "beta"}))
        .unwrap();

    // 模拟 v2 遗留的 agent 目录（config.json + 软链的残骸）。
    let agent_dir = harness.manager.config().env_agent_dir("beta");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("config.json"), "{}").unwrap();

    harness.manager.remove("beta").unwrap();
    assert!(!agent_dir.exists(), "agent 目录随环境消失");
    // 规则库不动：删规则是 rules.delete 的事。
    assert_eq!(
        harness.manager.rules_list().unwrap(),
        vec!["beta".to_string()]
    );
    assert!(harness.manager.config().rules_path("beta").exists());
}

// --------------------------------------------------------------------------- //
// reconcile（v3：desired × 内存报告）
// --------------------------------------------------------------------------- //

#[tokio::test]
async fn reconcile_restores_desired_running_after_a_restart() {
    let harness = Harness::new();
    {
        let first = harness.standalone();
        first.create(&serde_json::json!({"name": "beta"})).unwrap();
        first.start("beta").await.unwrap();
        // 离开作用域：引擎替身随管理器 drop → 实例消失（v3 里这就是"进程重启"）。
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let reopened = harness.standalone();
    let report = reopened.reconcile().await.unwrap();
    assert!(
        report
            .actions
            .iter()
            .any(|(env, action)| env == "beta" && action == "start"),
        "desired=running 必须被拉回: {:?}",
        report.actions
    );
    assert_eq!(
        reopened.health("beta").await.unwrap(),
        InstanceState::Running
    );
}

/// 接替 v2 的孤儿清理/PID 复用两案：v3 的期望翻停由 reconcile 停掉在跑实例；
/// 且"没在跑 + 期望停"绝不产生动作 —— **不存在可被误杀的进程**，PID 复用的
/// 病根在结构上消失（record/live 身份比对整体删除）。
#[tokio::test]
async fn reconcile_is_quiet_when_reality_already_matches_desire() {
    let harness = Harness::new();
    harness
        .manager
        .create(&serde_json::json!({"name": "beta"}))
        .unwrap();
    harness
        .manager
        .create(&serde_json::json!({"name": "quiet"}))
        .unwrap();
    harness.manager.start("beta").await.unwrap();

    let report = harness.manager.reconcile().await.unwrap();
    assert_eq!(
        action_of(&report, "beta"),
        Some("keep"),
        "live 且期望 running：keep，不是重启: {:?}",
        report.actions
    );
    assert_eq!(
        action_of(&report, "quiet"),
        None,
        "期望停且没在跑：连 keep 都不该出现（无动作）"
    );
    assert_eq!(
        harness.manager.get("beta").unwrap().health.as_str(),
        "running"
    );
}

fn action_of<'a>(report: &'a envboard_manager::ReconcileReport, env: &str) -> Option<&'a str> {
    report
        .actions
        .iter()
        .find(|(name, _)| name == env)
        .map(|(_, action)| action.as_str())
}

/// 接替 v2 的"上一代实例被 reconcile 重启"：v3 要重启的是**崩掉的引擎** ——
/// 报告 failed、期望仍 running → reconcile 拉回 running（任务级崩溃的自愈面）。
#[tokio::test]
async fn a_failed_engine_is_brought_back_by_reconcile() {
    let harness = Harness::new();
    create_beta(&harness);
    harness.manager.start("beta").await.unwrap();
    assert!(harness.engine().inject_state(
        "beta",
        InstanceState::Failed {
            reason: "boom".to_string(),
        },
    ));

    let report = harness.manager.reconcile().await.unwrap();
    assert!(
        report
            .actions
            .iter()
            .any(|(env, action)| env == "beta" && action == "start"),
        "{:?}",
        report.actions
    );
    assert_eq!(
        harness.manager.health("beta").await.unwrap(),
        InstanceState::Running,
        "重启后回到 running"
    );
}

/// v2 里 MarkConflict 由 plan 探测产生；v3 把探测退场，冲突在 Start 的绑定路径
/// 上如实浮出并留标记。断言保留三件事：动作发生了、端口没动、期望保持 running。
#[tokio::test]
async fn reconcile_survives_a_held_port_as_conflict_not_reassignment() {
    let harness = Harness::new();
    let beta = create_beta(&harness);

    let squatter = std::net::TcpListener::bind(("127.0.0.1", beta.listen.port)).unwrap();
    prepare_state(&harness, |state| {
        state
            .desired
            .insert("beta".into(), envboard_domain::Desired::Running);
    });

    // desired=running 是"上一次写者留下的期望"：standalone 读它并据规划。
    let sm = harness.standalone();
    let report = sm.reconcile().await.unwrap();
    assert!(
        report
            .actions
            .iter()
            .any(|(env, action)| env == "beta" && action == "mark_conflict"),
        "{:?}",
        report.actions
    );
    let view = sm.get("beta").unwrap();
    assert_eq!(view.health.as_str(), "port_conflict");
    assert_eq!(
        view.listen.port, beta.listen.port,
        "端口是身份的对外面：不静默换"
    );
    assert_eq!(view.desired, envboard_domain::Desired::Running);
    drop(squatter);
}

#[test]
fn create_probes_only_until_the_first_free_port() {
    // 惰性探测守门：区间全空时只探 1 次（每次判定都是一次真 bind，实测约 21ms，
    // "先探一遍区间"在 1000 个候选上要 21 秒，是不可接受的退化）。
    let window = NEXT_WINDOW.fetch_add(40, Ordering::SeqCst);
    let directory =
        std::env::temp_dir().join(format!("envboard-probe-{}-{window}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();

    let mut config = ManagerConfig::new(&directory);
    config.port_range = (window, window + 39);
    let probe = Arc::new(CountingProbe::default());
    let clock: Arc<dyn ClockPort> = Arc::new(envboard_core_api::ManualClock::new(1_000));
    let manager = Manager::new(
        config,
        Arc::new(FakeEngine::new()) as Arc<dyn ProxyEngine>,
        Arc::clone(&probe) as Arc<dyn PortProbe>,
        Arc::new(RealFiles),
        Arc::clone(&clock),
        Arc::new(NullLogger) as Arc<dyn LoggerPort>,
        Arc::new(JsonFileStateRepo::new(directory.join("state.json"))) as Arc<dyn StateRepo>,
        false,
    )
    .unwrap();

    manager
        .create(&serde_json::json!({"name": "beta"}))
        .unwrap();
    assert_eq!(
        probe.calls.load(Ordering::SeqCst),
        1,
        "探测必须在第一个空闲端口停下，不得遍历整个区间"
    );
    std::fs::remove_dir_all(&directory).ok();
}

/// 计数探针：真实行为 + 计数（探测只服务于分配，启动以绑定为准）。
#[derive(Debug, Default)]
struct CountingProbe {
    calls: std::sync::atomic::AtomicUsize,
}

impl PortProbe for CountingProbe {
    fn is_free(&self, host: std::net::IpAddr, port: u16) -> bool {
        self.calls.fetch_add(1, Ordering::SeqCst);
        <SocketPortProbe as PortProbe>::is_free(&SocketPortProbe, host, port)
    }
}

/// 契约（capabilities.md「实例日志」）：每次启动写一行运行标记，追加不截断。
/// 这也是"日志面板在第一个请求之前就有东西可看"的来源。
#[tokio::test]
async fn starting_an_instance_writes_the_run_marker_line() {
    let harness = Harness::new();
    create_beta(&harness);
    harness.manager.start("beta").await.unwrap();
    let path = harness
        .manager
        .config()
        .log_file("beta")
        .expect("默认 log_dir 是开着的");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.starts_with("--- envboard: env=beta listen=127.0.0.1:"),
        "{text}"
    );
    assert!(text.contains("started="), "{text}");

    // 重启追加第二段而不是截断：跨重启的历史靠标记分段。
    harness.manager.stop("beta").await.unwrap();
    harness.manager.start("beta").await.unwrap();
    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        after
            .lines()
            .filter(|line| line.starts_with("--- envboard:"))
            .count(),
        2,
        "{after}"
    );
}

// --------------------------------------------------------------------------- //
// 上游代理（chained proxy）
// --------------------------------------------------------------------------- //

#[test]
fn proxy_put_creates_replaces_and_never_echoes_credentials() {
    let harness = Harness::new();
    let view = harness
        .manager
        .proxy_put(&serde_json::json!({
            "name": "Corp", "host": "proxy.example.com", "port": 3128,
            "user": "alice", "password": "s3cret"
        }))
        .unwrap();
    assert_eq!(view.name, "corp");
    assert_eq!(view.host, "proxy.example.com");
    assert_eq!(view.port, 3128);
    assert!(view.has_auth);
    // 视图永不回显凭据
    let rendered = view.to_json().to_string();
    assert!(!rendered.contains("s3cret") && !rendered.contains("alice"), "{rendered}");

    // 同名即整体替换（与规则导入同构）
    let updated = harness
        .manager
        .proxy_put(&serde_json::json!({"name": "corp", "host": "10.0.0.9", "port": 8080}))
        .unwrap();
    assert_eq!(updated.port, 8080);
    assert!(!updated.has_auth);
    assert_eq!(harness.manager.proxy_list().unwrap().len(), 1);

    // 非法输入响亮失败，字段路径指向缺失的那一边
    let bad = harness
        .manager
        .proxy_put(&serde_json::json!({
            "name": "corp", "host": "10.0.0.9", "port": 8080, "user": "alice"
        }))
        .unwrap_err();
    assert_eq!(bad.code, ErrorCode::InvalidConfig);
    assert_eq!(bad.field.as_deref(), Some("proxy.password"));
}

#[test]
fn environment_cannot_bind_an_unknown_upstream_proxy() {
    let harness = Harness::new();
    let error = harness
        .manager
        .create(&serde_json::json!({"name": "beta", "upstream": "corp"}))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidConfig);
    assert_eq!(error.field.as_deref(), Some("environment.upstream"));

    // 建好代理后绑定成功；删除被引用的代理被拒并点名引用方
    harness
        .manager
        .proxy_put(&serde_json::json!({"name": "corp", "host": "10.0.0.9", "port": 8080}))
        .unwrap();
    let beta = harness
        .manager
        .create(&serde_json::json!({"name": "beta", "upstream": "corp"}))
        .unwrap();
    assert_eq!(beta.upstream.as_deref(), Some("corp"));
    let error = harness.manager.proxy_delete("corp").unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);
    assert!(error.message.contains("beta"), "{error}");

    // 解绑后可删；再绑定不存在的名字照旧响亮失败
    harness
        .manager
        .update("beta", &serde_json::json!({"upstream": null}))
        .unwrap();
    harness.manager.proxy_delete("corp").unwrap();
    let error = harness
        .manager
        .update("beta", &serde_json::json!({"upstream": "corp"}))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidConfig);
    assert_eq!(error.field.as_deref(), Some("environment.upstream"));
}

#[test]
fn proxy_view_lists_references_and_the_ledger_survives_a_restart() {
    let harness = Harness::new();
    harness
        .manager
        .proxy_put(&serde_json::json!({"name": "corp", "host": "10.0.0.9", "port": 8080}))
        .unwrap();
    harness
        .manager
        .create(&serde_json::json!({"name": "beta", "upstream": "corp"}))
        .unwrap();
    harness
        .manager
        .create(&serde_json::json!({"name": "gamma", "upstream": "corp"}))
        .unwrap();
    let view = harness.manager.proxy_get("corp").unwrap();
    assert_eq!(view.references, vec!["beta".to_string(), "gamma".to_string()]);

    // 账本落盘，重启后原样活过来
    assert_eq!(harness.saved().proxies.len(), 1);
    let again = harness.standalone();
    assert_eq!(again.proxy_list().unwrap().len(), 1);
}

#[tokio::test]
async fn binding_the_upstream_is_hot_and_reaches_the_engine_spec() {
    let harness = Harness::new();
    harness
        .manager
        .proxy_put(&serde_json::json!({
            "name": "corp", "host": "10.0.0.9", "port": 8080,
            "user": "alice", "password": "s3cret"
        }))
        .unwrap();
    let _ = harness.manager.create(&serde_json::json!({"name": "beta"})).unwrap();
    let started = harness.manager.start("beta").await.unwrap();
    assert_eq!(started.health.as_str(), "running");
    let hash_before = harness.engine().config_hash_of("beta").unwrap();

    // 运行中改绑定（热字段）：不重启即生效，config_hash 变化被引擎确认
    let updated = harness
        .manager
        .update("beta", &serde_json::json!({"upstream": "corp"}))
        .unwrap();
    assert_eq!(updated.health.as_str(), "running");
    let hash_after = harness.engine().config_hash_of("beta").unwrap();
    assert_ne!(hash_before, hash_after, "upstream binding must enter the spec hash");

    // 改代理实体本身（host/port/凭据）→ 引用环境再次热应用
    let hash_proxy_before = harness.engine().config_hash_of("beta").unwrap();
    harness
        .manager
        .proxy_put(&serde_json::json!({"name": "corp", "host": "10.0.0.10", "port": 3128}))
        .unwrap();
    let hash_proxy_after = harness.engine().config_hash_of("beta").unwrap();
    assert_ne!(hash_proxy_before, hash_proxy_after, "editing the proxy entity must hot-apply");
}

#[test]
fn a_dangling_upstream_reference_in_the_state_file_refuses_to_load() {
    let harness = Harness::new();
    let _ = harness.manager.create(&serde_json::json!({"name": "beta"})).unwrap();
    // 手工把悬空引用写进状态文件（绕过 create/update 的校验）——账本里没有 ghost
    prepare_state(&harness, |state| {
        state.environments[0]["upstream"] = serde_json::json!("ghost");
    });
    let engine: Arc<dyn envboard_core_api::ProxyEngine> = Arc::new(FakeEngine::new());
    // Manager 不是 Debug：先取 Result 再拆错误
    let loaded = Manager::new(
        harness.manager.config().clone(),
        engine,
        Arc::new(SocketPortProbe),
        Arc::new(RealFiles),
        harness.clock.clone() as Arc<dyn envboard_core_api::ClockPort>,
        harness.logger.clone() as Arc<dyn envboard_core_api::LoggerPort>,
        Arc::new(JsonFileStateRepo::new(harness.manager.config().state_file()))
            as Arc<dyn StateRepo>,
        false,
    );
    let error = match loaded {
        Err(error) => error,
        Ok(_) => panic!("a dangling upstream reference must refuse to load"),
    };
    assert_eq!(error.code, ErrorCode::InvalidConfig);
    assert_eq!(error.field.as_deref(), Some("environment.upstream"));
}

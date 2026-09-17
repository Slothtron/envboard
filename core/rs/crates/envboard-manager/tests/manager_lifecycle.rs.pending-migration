//! 管理器的行为测试 —— **完全不依赖 mitmproxy**（「不装 mitmproxy 也能跑默认流水线」的验收条件）。
//!
//! 用的是"真文件系统 + 真 socket + FakeCore"的组合：状态持久化、flock、端口试绑
//! 这些恰好是最容易写错的部分，用内存替身反而测不到。只有进程表用内存替身，
//! 因为要精确构造"PID 复用"这种场景。
//!
//! 每个测试拿到一段互不重叠的端口区间（见 `harness`），互不打扰。

use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};

use envboard_core_api::{
    ClockPort, CoreCapabilities, CoreInfo, ErrorCode, InstanceHandle, InstanceHealth, InstanceSpec,
    LogLevel, LoggerPort, NullLogger, ProcessIdentity, ProxyCore,
};
use envboard_core_fake::{FakeCore, StatusMode};
use envboard_manager::infra::{RealFiles, SocketPortProbe};
use envboard_manager::{
    EnvView, JsonFileStateRepo, Manager, ManagerConfig, MemoryProcessTable, PersistedState,
    ProcessTable, StateRepo,
};

// --------------------------------------------------------------------------- //
// 测试脚手架
// --------------------------------------------------------------------------- //

/// 每个测试一段独立的端口窗口，避免并行测试互相抢端口。
static NEXT_WINDOW: AtomicU16 = AtomicU16::new(21_000);

/// 手动时钟 —— 让"状态文件新鲜度"完全确定，不靠 sleep 猜。
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

    /// 往前走时间 —— 用来跨过"收敛窗口"而仍然落在状态文件的 TTL 之内。
    fn advance(&self, seconds: u64) {
        self.now.fetch_add(seconds, Ordering::SeqCst);
    }
}

impl ClockPort for SettableClock {
    fn now_unix(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
}

/// 记录日志以便断言（例如"重试过端口"）。
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

/// 声明"实例是外部进程"的 core 包装 —— 用来测孤儿清理路径。
///
/// 用**能力声明**而不是 `if core == "fake"` 来区分，后者是抽象漏了的信号。
struct ExternalProcessCore {
    inner: FakeCore,
}

#[async_trait::async_trait]
impl ProxyCore for ExternalProcessCore {
    fn describe(&self) -> CoreInfo {
        self.inner.describe()
    }

    fn capabilities(&self) -> CoreCapabilities {
        CoreCapabilities {
            external_processes: true,
            ..self.inner.capabilities()
        }
    }

    async fn start(&self, spec: InstanceSpec) -> Result<InstanceHandle, envboard_core_api::Error> {
        self.inner.start(spec).await
    }

    async fn stop(&self, handle: &InstanceHandle) -> Result<(), envboard_core_api::Error> {
        self.inner.stop(handle).await
    }

    async fn probe(&self, handle: &InstanceHandle) -> InstanceHealth {
        self.inner.probe(handle).await
    }
}

struct Harness {
    manager: Manager,
    directory: std::path::PathBuf,
    clock: Arc<SettableClock>,
    logger: Arc<RecordingLogger>,
    repo: Arc<JsonFileStateRepo>,
    /// 只在 [`Harness::new`] 的形态下可用：测试要拨 FakeCore 的旋钮
    /// （状态回写模式之前是经"每实例选项"传的，那条通道已删除）。
    fake: Option<Arc<FakeCore>>,
}

impl Harness {
    fn new() -> Self {
        Self::build(|clock| {
            let core = Arc::new(FakeCore::new(clock));
            (Arc::clone(&core) as Arc<dyn ProxyCore>, Some(core))
        })
    }

    fn build(
        factory: impl FnOnce(Arc<dyn ClockPort>) -> (Arc<dyn ProxyCore>, Option<Arc<FakeCore>>),
    ) -> Self {
        let window = NEXT_WINDOW.fetch_add(40, Ordering::SeqCst);
        let directory =
            std::env::temp_dir().join(format!("envboard-test-{}-{window}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();

        let mut config = ManagerConfig::new(&directory);
        config.port_range = (window, window + 39);
        config.listen_host = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

        let clock = Arc::new(SettableClock::new(1_000_000));
        let processes = Arc::new(MemoryProcessTable::new());
        let logger = Arc::new(RecordingLogger::default());
        let repo = Arc::new(JsonFileStateRepo::new(config.state_file()));
        let (core, fake) = factory(Arc::clone(&clock) as Arc<dyn ClockPort>);
        let manager = Manager::new(
            config,
            core,
            Arc::new(SocketPortProbe),
            Arc::clone(&processes) as Arc<dyn ProcessTable>,
            Arc::new(RealFiles),
            Arc::clone(&clock) as Arc<dyn ClockPort>,
            Arc::clone(&logger) as Arc<dyn LoggerPort>,
            Arc::clone(&repo) as Arc<dyn StateRepo>,
            false, // 测试里不抢目录锁（要测锁另有专门用例）
        )
        .unwrap();

        Self {
            manager,
            directory,
            clock,
            logger,
            repo,
            fake,
        }
    }

    /// FakeCore 的测试旋钮（外部进程形态的 core 没有它）。
    fn fake(&self) -> &Arc<FakeCore> {
        self.fake
            .as_ref()
            .expect("this harness was built with a custom core; use Harness::new()")
    }

    /// 把时钟往前拨（跨过收敛窗口，但仍在状态文件 TTL 内）。
    fn advance(&self, seconds: u64) {
        self.clock.advance(seconds);
    }

    /// 在同一个 state_dir 上再起一个管理器（模拟重启 / 另一个进程）。
    ///
    /// 用哪张进程表、声不声明"实例是外部进程"，决定了 reconcile 走哪条路，
    /// 所以它们都是参数 —— 否则就没法构造"PID 复用"这种场景。
    fn manager_over(&self, processes: Arc<MemoryProcessTable>, external: bool) -> Manager {
        let core: Arc<dyn ProxyCore> = if external {
            Arc::new(ExternalProcessCore {
                inner: FakeCore::new(self.clock_arc()),
            })
        } else {
            Arc::new(FakeCore::new(self.clock_arc()))
        };
        Manager::new(
            self.manager.config().clone(),
            core,
            Arc::new(SocketPortProbe),
            processes as Arc<dyn ProcessTable>,
            Arc::new(RealFiles),
            self.clock_arc(),
            Arc::new(NullLogger),
            Arc::new(JsonFileStateRepo::new(self.manager.config().state_file()))
                as Arc<dyn StateRepo>,
            false,
        )
        .unwrap()
    }

    fn standalone(&self) -> Manager {
        self.manager_over(Arc::new(MemoryProcessTable::new()), false)
    }

    fn clock_arc(&self) -> Arc<dyn ClockPort> {
        Arc::clone(&self.clock) as Arc<dyn ClockPort>
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

/// 带额外字段建一个 beta（字段只能是契约里的一等字段）。
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

// --------------------------------------------------------------------------- //
// 端口与创建
// --------------------------------------------------------------------------- //

#[test]
fn create_allocates_a_free_port_in_range_and_persists_it() {
    let harness = Harness::new();
    let beta = create_beta(&harness);
    assert_in_range(&beta, &harness);
    assert_eq!(beta.desired, envboard_domain::Desired::Stopped);
    assert!(
        !beta.proxy_command.is_empty(),
        "the workbench must hand out a copyable line"
    );

    // 第二个环境必须拿到不同的端口
    let prod = harness
        .manager
        .create(&serde_json::json!({"name": "prod"}))
        .unwrap();
    assert_ne!(beta.listen.port, prod.listen.port);
    assert_in_range(&prod, &harness);

    // 端口是环境的身份：已经持久化
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

    // v1 的遗留字段现在响亮失败
    let legacy = harness
        .manager
        .create(&serde_json::json!({"name": "old", "dns_servers": ["10.0.0.53"]}))
        .unwrap_err();
    assert_eq!(legacy.field.as_deref(), Some("environment.dns_servers"));
}

// --------------------------------------------------------------------------- //
// 启停与健康
// --------------------------------------------------------------------------- //

#[tokio::test]
async fn start_and_stop_round_trip_updates_desired_state_and_health() {
    let harness = Harness::new();
    let beta = create_beta(&harness);

    let started = harness.manager.start("beta").await.unwrap();
    assert_eq!(started.desired, envboard_domain::Desired::Running);

    let health = harness.manager.health("beta").await.unwrap();
    assert_eq!(
        health,
        InstanceHealth::Running,
        "status file should be fresh and consistent"
    );

    // 端口真的在监听（真 socket，不是替身）
    assert!(
        std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::new(beta.listen.host, beta.listen.port),
            std::time::Duration::from_millis(500)
        )
        .is_ok(),
        "the instance must actually listen on its port"
    );

    let stopped = harness.manager.stop("beta").await.unwrap();
    assert_eq!(stopped.desired, envboard_domain::Desired::Stopped);
    assert_eq!(
        harness.manager.health("beta").await.unwrap(),
        InstanceHealth::Stopped
    );
    assert!(!std::path::Path::new(&harness.manager.config().status_file("beta")).exists());
}

#[tokio::test]
async fn stale_status_file_is_reported_as_unhealthy() {
    let harness = Harness::new();
    // FakeCore 只写一次、时间戳是"一小时前"
    harness.fake().set_status_mode(StatusMode::Stale);
    create_beta(&harness);
    harness.manager.start("beta").await.unwrap();

    match harness.manager.health("beta").await.unwrap() {
        InstanceHealth::Unhealthy { reason } => assert!(reason.contains("stale"), "got: {reason}"),
        other => panic!("expected unhealthy, got {other:?}"),
    }
}

#[tokio::test]
async fn missing_status_file_is_unhealthy_even_though_the_port_accepts() {
    let harness = Harness::new();
    harness.fake().set_status_mode(StatusMode::Absent);
    create_beta(&harness);
    harness.manager.start("beta").await.unwrap();

    // 这一条是"探活为辅"的关键证据：端口连得上，但没有状态文件 → 不能算 running。
    let view = harness.manager.get("beta").unwrap();
    assert!(
        std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::new(view.listen.host, view.listen.port),
            std::time::Duration::from_millis(500)
        )
        .is_ok()
    );
    match harness.manager.health("beta").await.unwrap() {
        InstanceHealth::Unhealthy { reason } => {
            assert!(reason.contains("missing"), "got: {reason}")
        }
        other => panic!("expected unhealthy, got {other:?}"),
    }
}

#[tokio::test]
async fn config_echo_mismatch_is_detected() {
    // 契约回显：宿主对未知/拼错的 `--set` 是静默忽略的，所以要比对"生效后的回显"。
    let harness = Harness::new();
    harness
        .manager
        .import_rules(
            "beta",
            "10.0.0.11 api.example.com\n10.0.0.12 b.example.com\n",
        )
        .unwrap();
    harness
        .fake()
        .set_status_mode(StatusMode::RulesCountMismatch);
    harness
        .manager
        .create(&serde_json::json!({"name": "beta", "rules": "beta"}))
        .unwrap();
    harness.manager.start("beta").await.unwrap();

    // 回执比对的失败**只在收敛窗口之外**才算失败：刚换完绑定/刚写完配置的那几秒，
    // 注入器可能还没轮到轮询。
    harness.advance(10);
    match harness.manager.health("beta").await.unwrap() {
        InstanceHealth::ConfigMismatch { reason } => {
            assert!(reason.contains("rules_count"), "got: {reason}")
        }
        other => panic!("expected config_mismatch, got {other:?}"),
    }
}

#[tokio::test]
async fn a_stale_config_hash_is_tolerated_inside_the_window_then_reported() {
    let harness = Harness::new();
    harness
        .fake()
        .set_status_mode(StatusMode::ConfigHashMismatch);
    create_beta(&harness);
    harness.manager.start("beta").await.unwrap();

    // 刚写完配置：实例可能还没轮询到 → 收敛中，仍算 running
    assert_eq!(
        harness.manager.health("beta").await.unwrap(),
        InstanceHealth::Running
    );

    // 超出收敛窗口（reload_interval + 3s）仍然对不上 → 这份配置根本没生效
    harness.advance(10);
    match harness.manager.health("beta").await.unwrap() {
        InstanceHealth::ConfigMismatch { reason } => {
            assert!(reason.contains("config_hash"), "got: {reason}")
        }
        other => panic!("expected config_mismatch, got {other:?}"),
    }
}

#[tokio::test]
async fn a_failed_hot_reload_is_unhealthy_even_though_the_proxy_keeps_running() {
    let harness = Harness::new();
    harness.fake().set_status_mode(StatusMode::ConfigError);
    create_beta(&harness);
    harness.manager.start("beta").await.unwrap();

    match harness.manager.health("beta").await.unwrap() {
        InstanceHealth::Unhealthy { reason } => {
            assert!(reason.contains("previous configuration"), "got: {reason}")
        }
        other => panic!("expected unhealthy, got {other:?}"),
    }
}

#[tokio::test]
async fn binding_an_unknown_rules_name_is_refused_when_it_is_written() {
    let harness = Harness::new();
    // 新语义下"链接缺失 = 不覆盖"，所以绑定一个不存在的名字会变成**静默失效** ——
    // 宁可在写入时拒绝。规则名必须在账本里（或物化文件已经在）。
    let error = harness
        .manager
        .create(&serde_json::json!({"name": "beta", "rules": "nope"}))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound);
    assert_eq!(error.field.as_deref(), Some("environment.rules"));
    assert!(
        harness.manager.get("beta").is_err(),
        "a rejected environment must not be persisted"
    );

    // 导入后立刻绑得上，且软链就位
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
    assert!(
        harness
            .manager
            .config()
            .env_agent_dir("beta")
            .join("envboard.rules")
            .exists()
    );
}

#[tokio::test]
async fn a_missing_rules_link_means_no_override_without_a_start_failure() {
    let harness = Harness::new();
    harness
        .manager
        .import_rules("beta", "10.0.0.11 api.example.com\n")
        .unwrap();
    harness
        .manager
        .create(&serde_json::json!({"name": "beta", "rules": "beta"}))
        .unwrap();

    let started = harness.manager.start("beta").await.unwrap();
    assert_eq!(started.rules_count, 1);
    assert!(!started.rules_missing);

    // 有人把链删了（或目标文件被移走）。这不是"状态坏了"，而是"这个环境不再覆盖
    // 任何域名"：实例不会因此崩，健康判定也不该把它说成配置不对 —— 但视图必须说清楚。
    let link = harness
        .manager
        .config()
        .env_agent_dir("beta")
        .join("envboard.rules");
    std::fs::remove_file(&link).unwrap();

    let missing = harness.manager.get("beta").unwrap();
    assert_eq!(missing.health.as_str(), "running");
    assert_eq!(missing.rules_count, 0);
    assert!(
        missing.rules_missing,
        "the view must say the binding is not in effect"
    );

    // `start` 也不该因此失败（新的语义：缺失即忽略，不是响亮失败）
    assert_eq!(harness.manager.start("beta").await.unwrap().rules_count, 1);

    // 再删一次，让对账去修：账本里有这条规则 → 重链
    std::fs::remove_file(&link).unwrap();
    let messages = harness.manager.reconcile_rules().unwrap();
    assert!(!messages.is_empty(), "the repair must be logged");
    let fixed = harness.manager.get("beta").unwrap();
    assert!(!fixed.rules_missing);
    assert_eq!(fixed.rules_count, 1);
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

    // 升级迁移：旧版本的状态文件里没有 `rules` 这一段 → 启动对账从物化目录回填。
    prepare_state(&harness, |state| state.rules.clear());
    let reopened = harness.standalone();
    assert_eq!(
        reopened.rules_list().unwrap(),
        vec!["beta".to_string()],
        "materialised files must be backfilled into the ledger"
    );
    assert_eq!(reopened.rules_read("beta").unwrap(), rendered);

    // 自愈：物化文件被改坏 → 用账本里的正文逐字节重建。
    std::fs::write(&path, "garbage\n").unwrap();
    let messages = reopened.reconcile_rules().unwrap();
    assert!(
        messages.iter().any(|line| line.contains("rebuilt")),
        "got {messages:?}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), rendered);

    // 规则正文以账本为准：物化文件没了也照样答得出来。
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

    // 让另一个程序占着它（真的占：起一个 listener，而不是改探针）
    let squatter = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();

    let error = harness.manager.start("beta").await.unwrap_err();
    assert_eq!(error.code, ErrorCode::PortConflict);

    // 端口没变、状态是 port_conflict、期望仍是 running（显示为"受阻"而不是"被放弃"）
    let view = harness.manager.get("beta").unwrap();
    assert_eq!(
        view.listen.port, port,
        "an explicit port must never be reassigned silently"
    );
    assert_eq!(view.health.as_str(), "port_conflict");
    assert_eq!(view.desired, envboard_domain::Desired::Running);
    drop(squatter);
}

#[tokio::test]
async fn auto_allocated_port_conflict_retries_once_with_a_new_port() {
    let harness = Harness::new();
    let beta = create_beta(&harness);

    // 在新端口上安排"恰好被别人抢走"
    let squatter = std::net::TcpListener::bind(("127.0.0.1", beta.listen.port)).unwrap();
    let started = harness.manager.start("beta").await.unwrap();

    assert_ne!(
        started.listen.port, beta.listen.port,
        "the retry must pick a new port"
    );
    assert_eq!(started.health.as_str(), "running");
    assert!(
        harness
            .log_lines()
            .iter()
            .any(|line| line.contains("retrying once")),
        "the retry must be visible in the log, got: {:?}",
        harness.log_lines()
    );
    drop(squatter);
}

#[test]
fn the_options_passthrough_is_gone_and_its_tombstone_is_accepted() {
    let harness = Harness::new();
    // 非空 options = 这份配置**确实依赖**过那个透传通道 → 写入时就响亮拒绝，
    // 并指出唯一可行的替代（把受影响的域名列进 insecure_hosts）。
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
        "the error must point at the replacement, got: {}",
        error.message
    );
    assert!(
        harness.manager.get("beta").is_err(),
        "a rejected environment must not be persisted"
    );

    // 空对象是墓碑：旧版本每个环境都写 `options: {}`，一律拒绝会让升级卡死。
    // 收下、忽略、**不再写出去**。
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
        "the tombstone must not be written back"
    );
}

#[tokio::test]
async fn insecure_hosts_are_persisted_hot_editable_and_written_to_the_config_file() {
    let harness = Harness::new();
    let created = create_beta_with(
        &harness,
        serde_json::json!({"insecure_hosts": ["365.kdocs.cn"]}),
    );
    assert_eq!(created.insecure_hosts, ["365.kdocs.cn"]);

    let started = harness.manager.start("beta").await.unwrap();
    assert_eq!(started.health.as_str(), "running");

    // 放行清单是**热**的：运行中改它不需要停机 —— 注入器轮询到 config.json 变化就重读。
    let updated = harness
        .manager
        .update(
            "beta",
            &serde_json::json!({"insecure_hosts": ["web.wps.cn", "365.kdocs.cn"]}),
        )
        .unwrap();
    assert_eq!(updated.insecure_hosts, ["365.kdocs.cn", "web.wps.cn"]);
    assert_eq!(updated.health.as_str(), "running", "the list is hot");

    // 落库 + 写进注入器读的那份配置
    let state = harness.saved();
    assert_eq!(
        state.find("beta").unwrap()["insecure_hosts"],
        serde_json::json!(["365.kdocs.cn", "web.wps.cn"])
    );
    let config: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            harness
                .manager
                .config()
                .env_agent_dir("beta")
                .join("config.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        config["insecure_hosts"],
        serde_json::json!(["365.kdocs.cn", "web.wps.cn"])
    );

    // 凭据是**停机**字段：实例只在启动时经 `--set proxyauth=…` 拿到它。
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
    // 视图 / SSE 广播都看得到它 → 绝不能回显凭据。
    assert!(
        !serde_json::to_string(&with_credentials.to_json())
            .unwrap()
            .contains("s3cret")
    );
    // 明文只落在 0600 的状态文件与实例启动参数里。
    assert_eq!(
        harness.saved().find("beta").unwrap()["proxy_password"],
        serde_json::json!("s3cret")
    );
}

// --------------------------------------------------------------------------- //
// 编辑（PATCH）：热改 vs 必须停机的身份变更
// --------------------------------------------------------------------------- //

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

    // 运行中改描述：允许，且**不会**重启实例（健康仍是 running）
    let view = harness
        .manager
        .update("beta", &serde_json::json!({"description": "灰度 v2"}))
        .unwrap();
    assert_eq!(view.description, "灰度 v2");
    assert_eq!(
        view.health.as_str(),
        "running",
        "a hot edit must not restart"
    );

    // 运行中换绑定：**允许** —— 管理器原子换链 + 重写 config.json，注入器下一次轮询
    // 就跟上了。这正是"规则名是账本里的记录、链是绑定状态的表达"换来的能力。
    let link = harness
        .manager
        .config()
        .env_agent_dir("beta")
        .join("envboard.rules");
    let bound = harness
        .manager
        .update("beta", &serde_json::json!({"rules": "beta"}))
        .unwrap();
    assert_eq!(bound.rules.as_deref(), Some("beta"));
    assert_eq!(bound.rules_count, 1, "the binding resolves immediately");
    assert!(!bound.rules_missing);
    assert_eq!(bound.health.as_str(), "running", "the binding is hot");
    assert_eq!(
        std::fs::read_link(&link).unwrap().file_name().unwrap(),
        "beta.rules"
    );

    // 再换一次：链被原子替换，条数跟着变
    let switched = harness
        .manager
        .update("beta", &serde_json::json!({"rules": "gamma"}))
        .unwrap();
    assert_eq!(switched.rules_count, 2);
    assert_eq!(
        switched.description, "灰度 v2",
        "unmentioned fields stay put"
    );
    assert_eq!(
        std::fs::read_link(&link).unwrap().file_name().unwrap(),
        "gamma.rules"
    );

    // 解绑：`rules: null` 就是"不覆盖"，链消失（与"没给这个字段"是两回事）
    let unbound = harness
        .manager
        .update("beta", &serde_json::json!({"rules": null}))
        .unwrap();
    assert!(unbound.rules.is_none());
    assert!(!unbound.rules_missing);
    assert!(!link.exists());
}

#[tokio::test]
async fn update_refuses_identity_changes_while_running_and_applies_them_when_stopped() {
    let harness = Harness::new();
    let beta = create_beta(&harness);
    harness.manager.start("beta").await.unwrap();

    // 运行中改端口 / 改名：**必须**是先停止状态（否则客户端配置、状态文件、
    // 进程记录会同时对不上，属于"看着成功了但到处都不一致"）
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

    // 停止后就能改：改名 + 换端口一起，且 `desired`/`auto_port` 一起搬到新名字下
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
    assert!(harness.manager.get("beta").is_err(), "the old name is gone");

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
        "the copyable line must follow the new port"
    );

    // 新名字不能再撞车
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

    // 端口被别人占着 → start 失败并留下永久标记
    let squatter = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    let error = harness.manager.start("beta").await.unwrap_err();
    assert_eq!(error.code, ErrorCode::PortConflict);
    assert_eq!(
        harness.manager.get("beta").unwrap().health.as_str(),
        "port_conflict"
    );

    // 把端口换到空位上：旧标记必须消失，否则界面会拿**新**端口号报冲突（在说谎）
    let free = min + 12;
    harness
        .manager
        .update("beta", &serde_json::json!({"listen": {"port": free}}))
        .unwrap();
    let view = harness.manager.get("beta").unwrap();
    assert_eq!(view.listen.port, free);
    assert_ne!(view.health.as_str(), "port_conflict");
    drop(squatter);

    // 换个说法验证同一件事：现在真的能起来了
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
// 删除与规则导入
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
fn removing_an_environment_cleans_up_its_agent_directory() {
    let harness = Harness::new();
    harness
        .manager
        .import_rules("beta", "10.0.0.11 api.example.com\n")
        .unwrap();
    harness
        .manager
        .create(&serde_json::json!({"name": "beta", "rules": "beta"}))
        .unwrap();

    let agent_dir = harness.manager.config().env_agent_dir("beta");
    assert!(agent_dir.join("config.json").exists());
    assert!(agent_dir.join("envboard.rules").exists());

    harness.manager.remove("beta").unwrap();
    assert!(
        !agent_dir.exists(),
        "the agent directory must go with the env"
    );
    // 规则库**不动**：删规则是 rules.delete 的事（它还被别的环境绑着也说不定）。
    assert_eq!(
        harness.manager.rules_list().unwrap(),
        vec!["beta".to_string()]
    );
    assert!(harness.manager.config().rules_path("beta").exists());
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
        assert_eq!(
            mode, 0o600,
            "rules files are imported state; keep them private"
        );
    }

    // 规则名是路径穿越的唯一防线
    let error = harness.manager.import_rules("../escape", "").unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidConfig);
}

// --------------------------------------------------------------------------- //
// reconcile
// --------------------------------------------------------------------------- //

#[tokio::test]
async fn reconcile_restores_desired_running_after_a_restart() {
    let harness = Harness::new();
    {
        // 第一个管理器：建环境并跑起来
        let first = harness.standalone();
        first.create(&serde_json::json!({"name": "beta"})).unwrap();
        first.start("beta").await.unwrap();
        assert_eq!(first.health("beta").await.unwrap(), InstanceHealth::Running);
        // 离开作用域 → core 被 drop → 监听任务中止、端口释放（等价于父进程死亡）
    }
    // `abort()` 是异步的：给它一拍，端口才真的空出来（生产里 PDEATHSIG 也是这个量级）。
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let reopened = harness.standalone();
    let report = reopened.reconcile().await.unwrap();
    assert!(
        report
            .actions
            .iter()
            .any(|(env, action)| env == "beta" && action == "start"),
        "desired=running must be restored, got {:?}",
        report.actions
    );
    assert_eq!(
        reopened.health("beta").await.unwrap(),
        InstanceHealth::Running
    );
}

#[tokio::test]
async fn reconcile_stops_orphans_but_never_touches_a_reused_pid() {
    let harness = Harness::new();
    create_beta(&harness);

    // 记录由**外部进程 core** 写入，所以这里直接构造一条"上次运行留下的身份"。
    let recorded = ProcessIdentity {
        pid: 4_242,
        starttime: 900,
        cmdline: vec![
            "mitmdump".into(),
            "-s".into(),
            "/home/u/.envboard/agent/envboard_mitmproxy.py".into(),
            "--set".into(),
            "listen_port=16301".into(),
        ],
    };

    // 场景 A：进程还在、身份完全匹配、期望是 stopped → 终止它
    let same = Arc::new(MemoryProcessTable::new().with_alive([recorded.clone()]));
    prepare_state(&harness, |state| {
        state
            .desired
            .insert("beta".into(), envboard_domain::Desired::Stopped);
        state.records.insert("beta".into(), recorded.clone());
    });
    let outcome_a = harness
        .manager_over(Arc::clone(&same), true)
        .reconcile()
        .await
        .unwrap();
    assert!(
        outcome_a
            .actions
            .iter()
            .any(|(env, action)| env == "beta" && action == "stop"),
        "got {:?}",
        outcome_a.actions
    );
    assert_eq!(same.terminated(), vec![recorded.pid]);

    // 场景 B：PID 相同但启动时刻不同（PID 复用）→ **绝不能杀**
    let reused = ProcessIdentity {
        starttime: recorded.starttime + 500,
        ..recorded.clone()
    };
    let different = Arc::new(MemoryProcessTable::new().with_alive([reused]));
    prepare_state(&harness, |state| {
        state
            .desired
            .insert("beta".into(), envboard_domain::Desired::Stopped);
        state.records.insert("beta".into(), recorded.clone());
    });
    let outcome_b = harness
        .manager_over(Arc::clone(&different), true)
        .reconcile()
        .await
        .unwrap();
    assert!(
        outcome_b
            .actions
            .iter()
            .any(|(env, action)| env == "beta" && action == "keep"),
        "got {:?}",
        outcome_b.actions
    );
    assert!(
        different.terminated().is_empty(),
        "a reused pid must never be killed"
    );
    assert!(
        outcome_b
            .warnings
            .iter()
            .any(|warning| warning == "pid_reuse_suspected")
    );
}

#[tokio::test]
async fn a_previous_generation_instance_is_restarted_by_reconcile() {
    let harness = Harness::new();
    let beta = create_beta(&harness);

    // 构造"上一代二进制拉起的实例"：进程还在、身份匹配，但状态文件里
    // **没有配置哈希回执** —— 说明它跑的根本不是"配置文件 + 固定名软链"这套通道。
    let status_file = harness.manager.config().status_file("beta");
    std::fs::create_dir_all(status_file.parent().unwrap()).unwrap();
    std::fs::write(
        &status_file,
        serde_json::json!({
            "env_name": "beta",
            "pid": 4_242,
            "listen": {"host": "127.0.0.1", "port": beta.listen.port},
            "rules_count": 0,
            "reload_interval_secs": 5,
            "core_version": "12.2.3",
            "agent_version": "0.2.0",
            "updated_at": 1_000_000,
        })
        .to_string(),
    )
    .unwrap();

    let recorded = ProcessIdentity {
        pid: 4_242,
        starttime: 900,
        cmdline: vec!["mitmdump".into()],
    };
    prepare_state(&harness, |state| {
        state
            .desired
            .insert("beta".into(), envboard_domain::Desired::Running);
        state.records.insert("beta".into(), recorded.clone());
    });

    let processes = Arc::new(MemoryProcessTable::new().with_alive([recorded]));
    let report = harness
        .manager_over(Arc::clone(&processes), true)
        .reconcile()
        .await
        .unwrap();
    assert!(
        report
            .actions
            .iter()
            .any(|(env, action)| env == "beta" && action == "restart"),
        "a previous-generation instance must be restarted, got {:?}",
        report.actions
    );
    assert_eq!(
        processes.terminated(),
        vec![4_242],
        "the old process must actually be stopped"
    );
}

#[tokio::test]
async fn reconcile_marks_port_conflict_instead_of_reallocating() {
    let harness = Harness::new();
    let beta = create_beta(&harness);

    // 期望运行、但端口被别人占着（真的被占：起一个 listener）
    let squatter = std::net::TcpListener::bind(("127.0.0.1", beta.listen.port)).unwrap();
    prepare_state(&harness, |state| {
        state
            .desired
            .insert("beta".into(), envboard_domain::Desired::Running);
    });

    let manager = harness.standalone();
    let report = manager.reconcile().await.unwrap();
    assert!(
        report
            .actions
            .iter()
            .any(|(env, action)| env == "beta" && action == "mark_conflict"),
        "got {:?}",
        report.actions
    );
    let view = manager.get("beta").unwrap();
    assert_eq!(view.health.as_str(), "port_conflict");
    assert_eq!(
        view.listen.port, beta.listen.port,
        "ports are identity; no silent reassignment"
    );
    drop(squatter);
}

/// 直接改状态文件 —— 用来构造"上一次运行时留下的状态"。
///
/// 注意：管理器把状态缓存在内存里（**状态只有一个写者**，这是有意的），
/// 所以改完必须**新建一个管理器**才看得到，不能指望当前实例热读。
fn prepare_state(harness: &Harness, prepare: impl FnOnce(&mut PersistedState)) {
    let mut state = harness.saved();
    prepare(&mut state);
    harness.repo.save(&state).unwrap();
}

#[test]
fn create_probes_only_until_the_first_free_port() {
    // 惰性探测的守门测试。**每次判定都是一次真实的 bind**，本机实测单次约 21ms
    // （WSL2 mirrored 模式；连 bind 到端口 0 也一样）—— 所以"先把区间探一遍"
    // 在 1000 个候选上要花约 21 秒，是不可接受的退化。这里断言：区间全空时只探 1 次。
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
        Arc::new(FakeCore::new(Arc::clone(&clock))),
        Arc::clone(&probe) as Arc<dyn envboard_manager::PortProbe>,
        Arc::new(MemoryProcessTable::new()) as Arc<dyn ProcessTable>,
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
        "probing must stop at the first free port, not materialize the whole range"
    );
    std::fs::remove_dir_all(&directory).ok();
}

/// 记录探测次数的探针：真实行为 + 计数。
#[derive(Debug, Default)]
struct CountingProbe {
    calls: std::sync::atomic::AtomicUsize,
}

impl envboard_manager::PortProbe for CountingProbe {
    fn is_free(&self, host: std::net::IpAddr, port: u16) -> bool {
        self.calls.fetch_add(1, Ordering::SeqCst);
        envboard_manager::PortProbe::is_free(&SocketPortProbe, host, port)
    }
}

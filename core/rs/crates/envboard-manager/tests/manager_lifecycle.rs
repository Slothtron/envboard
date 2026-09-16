//! 管理器的行为测试 —— **完全不依赖 mitmproxy**（「不装 mitmproxy 也能跑默认流水线」的验收条件）。
//!
//! 用的是"真文件系统 + 真 socket + FakeCore"的组合：状态持久化、flock、端口试绑
//! 这些恰好是最容易写错的部分，用内存替身反而测不到。只有进程表用内存替身，
//! 因为要精确构造"PID 复用"这种场景。
//!
//! 每个测试拿到一段互不重叠的端口区间（见 `harness`），互不打扰。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};

use envboard_core_api::{
    ClockPort, CoreCapabilities, CoreInfo, ErrorCode, InstanceHandle, InstanceHealth, InstanceSpec,
    LogLevel, LoggerPort, NullLogger, ProcessIdentity, ProxyCore,
};
use envboard_core_fake::FakeCore;
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
}

impl Harness {
    fn new() -> Self {
        Self::with_core_factory(|clock| Arc::new(FakeCore::new(clock)))
    }

    fn with_core_factory(factory: impl FnOnce(Arc<dyn ClockPort>) -> Arc<dyn ProxyCore>) -> Self {
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
        let manager = Manager::new(
            config,
            factory(Arc::clone(&clock) as Arc<dyn ClockPort>),
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
        }
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
    create_beta(&harness);
    // FakeCore 只写一次、时间戳是"一小时前"
    harness
        .manager
        .start_with_options("beta", &options(&[("fake_status", "stale")]))
        .await
        .unwrap();

    match harness.manager.health("beta").await.unwrap() {
        InstanceHealth::Unhealthy { reason } => assert!(reason.contains("stale"), "got: {reason}"),
        other => panic!("expected unhealthy, got {other:?}"),
    }
}

#[tokio::test]
async fn missing_status_file_is_unhealthy_even_though_the_port_accepts() {
    let harness = Harness::new();
    create_beta(&harness);
    harness
        .manager
        .start_with_options("beta", &options(&[("fake_status", "absent")]))
        .await
        .unwrap();

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
        .manager
        .create(&serde_json::json!({"name": "beta", "rules": "beta"}))
        .unwrap();
    harness
        .manager
        .start_with_options("beta", &options(&[("fake_status", "rules_count_mismatch")]))
        .await
        .unwrap();

    match harness.manager.health("beta").await.unwrap() {
        InstanceHealth::ConfigMismatch { reason } => {
            assert!(reason.contains("rules_count"), "got: {reason}")
        }
        other => panic!("expected config_mismatch, got {other:?}"),
    }
}

#[tokio::test]
async fn binding_a_missing_rules_file_fails_loudly_on_start() {
    let harness = Harness::new();
    // 绑定一个还没导入的规则名：**创建**不失败（否则一个环境坏了会连累整张列表），
    // 但视图里如实显示条数 0，`start` 才响亮失败。
    let view = harness
        .manager
        .create(&serde_json::json!({"name": "beta", "rules": "nope"}))
        .unwrap();
    assert_eq!(view.rules_count, 0);

    let error = harness.manager.start("beta").await.unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound);
    assert_eq!(error.field.as_deref(), Some("environment.rules"));

    // 导入后就能起来了
    harness
        .manager
        .import_rules("nope", "10.0.0.11 api.example.com\n")
        .unwrap();
    let started = harness.manager.start("beta").await.unwrap();
    assert_eq!(started.rules_count, 1);
    assert_eq!(started.health.as_str(), "running");
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

#[tokio::test]
async fn options_denylist_is_enforced_before_starting() {
    let harness = Harness::new();
    create_beta(&harness);
    let error = harness
        .manager
        .start_with_options("beta", &options(&[("listen_port", "9999")]))
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidConfig);
    assert_eq!(error.field.as_deref(), Some("instance.options.listen_port"));
    assert_eq!(
        harness.manager.health("beta").await.unwrap(),
        InstanceHealth::Stopped,
        "nothing must be started when the options are rejected"
    );
}

// --------------------------------------------------------------------------- //
// 编辑（PATCH）：热改 vs 必须停机的身份变更
// --------------------------------------------------------------------------- //

#[tokio::test]
async fn update_changes_the_description_live_but_a_new_binding_needs_a_restart() {
    let harness = Harness::new();
    harness
        .manager
        .import_rules("beta", "10.0.0.11 api.example.com\n")
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
    assert_eq!(
        harness.manager.health("beta").await.unwrap(),
        InstanceHealth::Running
    );

    // 运行中换绑定：**拒绝**。实例在启动时就固定了规则路径，运行中换绑定它看不见 ——
    // 允许的话就会变成"配置说绑了 beta、实例还在按空规则干活"。
    let error = harness
        .manager
        .update("beta", &serde_json::json!({"rules": "beta"}))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);
    assert!(error.message.contains("binding"), "got: {}", error.message);
    assert!(harness.manager.get("beta").unwrap().rules.is_none());

    // 停止后才绑得上，并且条数立刻算得出来
    harness.manager.stop("beta").await.unwrap();
    let bound = harness
        .manager
        .update("beta", &serde_json::json!({"rules": "beta"}))
        .unwrap();
    assert_eq!(bound.rules.as_deref(), Some("beta"));
    assert_eq!(bound.rules_count, 1, "the binding must resolve immediately");
    assert_eq!(bound.description, "灰度 v2", "unmentioned fields stay put");

    // 解绑：`rules: null` 在契约里就是"不覆盖"，与"没给这个字段"是两回事
    let unbound = harness
        .manager
        .update("beta", &serde_json::json!({"rules": null}))
        .unwrap();
    assert!(unbound.rules.is_none());
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

fn options(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

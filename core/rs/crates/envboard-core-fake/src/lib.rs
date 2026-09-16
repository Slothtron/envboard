//! `FakeCore` —— 只监听端口、不改写任何东西的 proxy core。
//!
//! 它的价值不在功能，而在**让管理器的测试完全不依赖 mitmproxy**：
//! 编排、端口分配、健康判定、reconcile、配置回执这些逻辑占了管理器的大部分代码，
//! 而它们与"谁来代理流量"无关。把 mitmproxy 从这条路径上摘掉，CI 快一个数量级，
//! 也让"抽象是否真的成立"变成可执行的断言 —— 如果管理器里出现
//! `if core == "mitmproxy"`，这个 crate 就编译不过或测试失败。
//!
//! 它刻意**不做**的事：不改写上连地址、不签证书、不解析规则去改流量。
//! 规则文件只被用来**如实回显条数**，`config.json` 只被用来**如实回显内容哈希** ——
//! 那两个字段正是管理器"配置是否真的落地"判据依赖的东西，替身必须给真值，
//! 否则测试会绿得没有内容。
//!
//! 测试旋钮是**显式的 setter**（`set_status_mode` / `set_bind_fail` / …），
//! 不再经"每实例选项"传递：那条通道已从契约里删除。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use envboard_core_api::{
    ClockPort, CoreCapabilities, CoreInfo, Error, ErrorCode, InstanceHandle, InstanceHealth,
    InstanceSpec, Listen, ProxyCore, StatusReport,
};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// 状态文件的回写行为 —— 用来构造管理器健康判定的各种路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusMode {
    /// 正常：如实回显配置哈希与规则条数，并周期刷新。
    #[default]
    Normal,
    /// 不写状态文件（模拟注入器启动失败/被杀）。
    Absent,
    /// 写一次，但时间戳是"很久以前"（模拟回写线程死掉）。
    Stale,
    /// 如实回写，但 `rules_count` 故意报错值（模拟"某个 `--set` 被静默忽略"）。
    RulesCountMismatch,
    /// 回报一个**对不上**的配置哈希（模拟注入器还没读进新配置）。
    ConfigHashMismatch,
    /// 完全不报配置哈希（模拟**上一代**注入器写的状态文件）。
    LegacyConfigHash,
    /// 回报 `config_error`（模拟热重载校验失败、实例保留旧快照）。
    ConfigError,
}

struct RunningInstance {
    task: JoinHandle<()>,
    /// 状态回写任务的句柄（`None` 表示该模式不回写）。
    writer: Option<JoinHandle<()>>,
    status_file: PathBuf,
}

/// 只监听端口的 proxy core。
pub struct FakeCore {
    clock: Arc<dyn ClockPort>,
    instances: Mutex<std::collections::BTreeMap<String, RunningInstance>>,
    /// 测试旋钮：让"绑定端口失败"这条路径可以被确定性地测到。
    bind_fail: Mutex<bool>,
    status_mode: Mutex<StatusMode>,
    reload_interval_secs: Mutex<u64>,
}

impl FakeCore {
    pub fn new(clock: Arc<dyn ClockPort>) -> Self {
        Self {
            clock,
            instances: Mutex::new(std::collections::BTreeMap::new()),
            bind_fail: Mutex::new(false),
            status_mode: Mutex::new(StatusMode::Normal),
            reload_interval_secs: Mutex::new(5),
        }
    }

    /// 模拟"端口已被占用"的启动失败。
    pub fn set_bind_fail(&self, fail: bool) {
        *self.bind_fail.lock().unwrap() = fail;
    }

    /// 选择状态文件的回写行为。
    pub fn set_status_mode(&self, mode: StatusMode) {
        *self.status_mode.lock().unwrap() = mode;
    }

    /// 状态文件里回显的轮询间隔。
    pub fn set_reload_interval(&self, seconds: u64) {
        *self.reload_interval_secs.lock().unwrap() = seconds;
    }

    fn key(env: &str, listen: Listen) -> String {
        format!("{env}@{}", listen.port)
    }
}

/// 卸载时**必须**收干净：监听任务与状态回写任务都要中止。
///
/// 这不只是整洁问题（禁止在实例停止后残留监听器/定时器）：
/// 残留的 accept 任务会一直占着端口，于是"管理器重启后端口应该空出来"这条
/// 语义在测试里永远不成立，会把人引向错误的结论。
impl Drop for FakeCore {
    fn drop(&mut self) {
        let instances = std::mem::take(&mut *self.instances.lock().unwrap());
        for (_, instance) in instances {
            instance.task.abort();
            if let Some(writer) = instance.writer {
                writer.abort();
            }
            let _ = std::fs::remove_file(&instance.status_file);
        }
    }
}

#[async_trait]
impl ProxyCore for FakeCore {
    fn describe(&self) -> CoreInfo {
        CoreInfo {
            name: "fake".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    fn capabilities(&self) -> CoreCapabilities {
        CoreCapabilities {
            listen: true,
            dynamic_certs: false,
            rewrite_upstream: false,
            // 它不真去放宽什么，但如实回显放行域名条数 —— 管理器的比对逻辑因此被覆盖。
            per_domain_insecure: true,
            shared_ca: false,
            flow_annotation: false,
            // 状态文件里如实回显 rules_count —— 因此管理器可以做该项比对。
            reports_rules_count: true,
            // 实例活在管理器进程**内部**：没有可清理的外部进程。
            external_processes: false,
        }
    }

    async fn start(&self, spec: InstanceSpec) -> Result<InstanceHandle, Error> {
        // 刻意在绑定之前检查：让"端口被占"这条路径可以被确定性地测到，
        // 不依赖测试机器上恰好有人占着某个端口。
        if *self.bind_fail.lock().unwrap() {
            return Err(Error::new(
                ErrorCode::PortConflict,
                format!("listen port {} is already in use", spec.listen.port),
            ));
        }

        let address = SocketAddr::new(spec.listen.host, spec.listen.port);
        let listener = TcpListener::bind(address).await.map_err(|error| {
            if error.kind() == std::io::ErrorKind::AddrInUse {
                Error::new(
                    ErrorCode::PortConflict,
                    format!("listen port {} is already in use", spec.listen.port),
                )
            } else {
                Error::new(
                    ErrorCode::InternalError,
                    format!("cannot bind {address}: {error}"),
                )
            }
        })?;

        // 接受并立刻断开：让"TCP 能连上 = listener 活着"这条探活成立。
        let accept_task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                drop(stream);
            }
        });

        let mode = *self.status_mode.lock().unwrap();
        let status = build_status(
            &spec,
            self.clock.now_unix(),
            mode,
            *self.reload_interval_secs.lock().unwrap(),
        )?;
        let writer = spawn_status_writer(&spec, mode, status, Arc::clone(&self.clock));

        self.instances.lock().unwrap().insert(
            Self::key(&spec.env, spec.listen),
            RunningInstance {
                task: accept_task,
                writer,
                status_file: spec.status_file.clone(),
            },
        );

        // `process: None` —— 实例活在管理器进程内，**没有**独立的 OS 进程身份。
        // 这正是 `external_processes = false` 的含义：管理器不该去 /proc 里找它，
        // 也不该把它写进 `records`（那是留给外部进程的孤儿清理判据）。
        Ok(InstanceHandle {
            env: spec.env,
            listen: spec.listen,
            process: None,
        })
    }

    async fn stop(&self, handle: &InstanceHandle) -> Result<(), Error> {
        let Some(instance) = self
            .instances
            .lock()
            .unwrap()
            .remove(&Self::key(&handle.env, handle.listen))
        else {
            return Err(Error::new(
                ErrorCode::NotFound,
                format!("no running fake instance for environment {:?}", handle.env),
            ));
        };
        instance.task.abort();
        if let Some(writer) = instance.writer {
            writer.abort();
        }
        // 状态文件跟着实例走：留着它会让管理器把一个已经死掉的实例判成"在跑"。
        let _ = std::fs::remove_file(&instance.status_file);
        Ok(())
    }

    async fn probe(&self, handle: &InstanceHandle) -> InstanceHealth {
        if self
            .instances
            .lock()
            .unwrap()
            .contains_key(&Self::key(&handle.env, handle.listen))
        {
            InstanceHealth::Running
        } else {
            InstanceHealth::Stopped
        }
    }
}

fn build_status(
    spec: &InstanceSpec,
    now: u64,
    mode: StatusMode,
    reload_interval_secs: u64,
) -> Result<StatusReport, Error> {
    // 规则条数读**注入器真正会读的那个固定名软链** —— 与真实注入器同一条路径，
    // 于是"换绑定 → 条数变化"这条链在替身上也是真的。
    let link = spec.rules_link();
    let rules_count = match std::fs::read_to_string(&link) {
        Ok(text) => envboard_rules::parse_hosts_text(&text).accepted(),
        Err(_) => 0,
    };
    let reported = match mode {
        StatusMode::RulesCountMismatch => rules_count + 1,
        _ => rules_count,
    };
    let updated_at = match mode {
        StatusMode::Stale => now.saturating_sub(3_600),
        _ => now,
    };
    let config_hash = match mode {
        StatusMode::LegacyConfigHash => String::new(),
        StatusMode::ConfigHashMismatch => "0".repeat(64),
        _ => std::fs::read(spec.config_file())
            .map(|bytes| envboard_core_api::sha256::hex(&bytes))
            .unwrap_or_default(),
    };
    let config_error = match mode {
        StatusMode::ConfigError => Some("config.json is not a valid AgentConfig".to_string()),
        _ => None,
    };

    Ok(StatusReport {
        env_name: spec.env.clone(),
        pid: std::process::id() as i32,
        listen: spec.listen,
        rules_path: Some(link.display().to_string()),
        rules_count: reported,
        insecure_hosts_count: spec.insecure_hosts.len(),
        config_hash,
        config_error,
        reload_interval_secs,
        core_version: env!("CARGO_PKG_VERSION").to_string(),
        agent_version: "fake".to_string(),
        updated_at,
        last_error: None,
        // 如实回显"管理器下发的启动契约键" —— 这样管理器的契约回显比对逻辑
        // 在 FakeCore 下同样被覆盖。
        options_echo: spec
            .launch_expected()
            .keys()
            .map(|key| (key.clone(), "ok".to_string()))
            .collect(),
    })
}

fn spawn_status_writer(
    spec: &InstanceSpec,
    mode: StatusMode,
    status: StatusReport,
    clock: Arc<dyn ClockPort>,
) -> Option<JoinHandle<()>> {
    if mode == StatusMode::Absent {
        return None;
    }
    let path = spec.status_file.clone();
    write_status(&path, &status);
    if mode == StatusMode::Stale {
        // 陈旧模式：写一次就不再刷新。
        return None;
    }

    Some(tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(500));
        loop {
            ticker.tick().await;
            let mut refreshed = status.clone();
            refreshed.updated_at = clock.now_unix();
            write_status(&path, &refreshed);
        }
    }))
}

/// 原子写：先写临时文件再 rename，避免管理器读到半个 JSON。
fn write_status(path: &std::path::Path, status: &StatusReport) {
    let Ok(raw) = status.to_json_vec() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, &raw).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use envboard_core_api::ManualClock;

    fn spec(port: u16) -> InstanceSpec {
        InstanceSpec {
            env: "beta".into(),
            listen: Listen::localhost(port),
            agent_dir: std::env::temp_dir().join(format!("envboard-fake-agent-{port}")),
            status_file: std::env::temp_dir().join(format!("envboard-fake-{port}.status.json")),
            shared_state_dir: std::env::temp_dir(),
            runtime_dir: std::env::temp_dir(),
            log_dir: None,
            insecure_hosts: Vec::new(),
            proxy_user: None,
            proxy_password: None,
        }
    }

    #[tokio::test]
    async fn starts_listens_and_reports_running() {
        let core = FakeCore::new(Arc::new(ManualClock::new(1_000)));
        let handle = core.start(spec(16_980)).await.unwrap();
        assert!(core.probe(&handle).await.is_running());
        core.stop(&handle).await.unwrap();
        assert_eq!(core.probe(&handle).await, InstanceHealth::Stopped);
    }

    #[tokio::test]
    async fn bind_failure_is_reported_as_port_conflict() {
        let core = FakeCore::new(Arc::new(ManualClock::new(1_000)));
        core.set_bind_fail(true);
        let error = core.start(spec(16_981)).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::PortConflict);
    }

    #[tokio::test]
    async fn real_port_collision_is_also_port_conflict() {
        let core = FakeCore::new(Arc::new(ManualClock::new(1_000)));
        let first = core.start(spec(16_982)).await.unwrap();
        let error = core.start(spec(16_982)).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::PortConflict);
        core.stop(&first).await.unwrap();
    }

    #[tokio::test]
    async fn status_mode_absent_writes_no_file() {
        let core = FakeCore::new(Arc::new(ManualClock::new(1_000)));
        core.set_status_mode(StatusMode::Absent);
        let spec = spec(16_983);
        let status_file = spec.status_file.clone();
        let handle = core.start(spec).await.unwrap();
        assert!(!status_file.exists());
        core.stop(&handle).await.unwrap();
    }

    #[tokio::test]
    async fn stop_removes_the_status_file() {
        let core = FakeCore::new(Arc::new(ManualClock::new(1_000)));
        let spec = spec(16_984);
        let status_file = spec.status_file.clone();
        let handle = core.start(spec).await.unwrap();
        assert!(status_file.exists());
        core.stop(&handle).await.unwrap();
        assert!(
            !status_file.exists(),
            "a stale status file makes a dead instance look alive"
        );
    }
}

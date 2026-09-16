//! `ProxyCore` 的 mitmproxy 实现。
//!
//! 它做四件事，顺序不能乱：
//!
//! 1. **物化注入器**（[`crate::injector`]）—— 版本随二进制走；
//! 2. **预物化共享 CA**（[`crate::ca`]）—— 必须在并发拉起任何实例**之前**完成；
//! 3. **按启动契约拼参数并 spawn** `mitmdump`，带上 `PR_SET_PDEATHSIG`；
//! 4. **等状态文件出现且新鲜**才算启动成功 —— 不以"进程起来了"为成功判据，
//!    因为 listener 先起、注入器随后才加载规则。

use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use envboard_core_api::{
    ClockPort, CoreCapabilities, CoreInfo, Error, ErrorCode, InstanceHandle, InstanceHealth,
    InstanceSpec, Listen, ProcessIdentity, ProxyCore, StatusReport,
};
use tokio::process::{Child, Command};

use crate::ca;
use crate::injector;
use crate::python::{self, PythonInterpreter};

pub struct MitmproxyCoreConfig {
    /// `mitmdump` 路径（默认按 PATH 解析）。core 可执行文件与解释器路径都不写死。
    pub core_bin: PathBuf,
    /// 显式配置的解释器；`None` 时按 [`python::discover`] 的规则探测。
    pub python: Option<PathBuf>,
    /// 注入器物化目录（`<state_dir>/agent/`）。每个环境一个子目录。
    ///
    /// 注入器读的 `config.json`、规则软链与它自己都在 `<agent_dir>/<env>/` 里；
    /// 轮询间隔与注解开关也在那份配置里，不再经 `--set` 下发。
    pub agent_dir: PathBuf,
    /// 等状态文件出现的上限（秒）。超时即判定启动失败。
    pub startup_timeout_secs: u64,
}

/// 内存环形缓冲的容量：够工作台看尾部，又不会无限长。
const LOG_RING_CAPACITY: usize = 1_000;

/// 回收任务的轮询间隔。子进程自己死掉时**必须**有人 `try_wait()` 把它收掉，
/// 否则它会以僵尸形态一直挂在进程表里（实测过）。
const REAP_INTERVAL: Duration = Duration::from_millis(500);

/// 实例输出去哪。两种形态对应两种启动方式，**不能混**（见 `start`）。
#[derive(Clone)]
enum LogSink {
    /// 内存环形缓冲：`log_dir = None` 时用管道读进来。**只有 core 自己读得到它**。
    Ring(Arc<Mutex<VecDeque<String>>>),
    /// 子进程直接写文件：落在 `<log_dir>`（状态目录下），
    /// 所以尾部由**管理器**统一读与轮转（见 `envboard-manager/src/logs.rs`），
    /// 这里不重复实现一份文件读取。
    File,
}

struct Running {
    child: Child,
    identity: ProcessIdentity,
    status_file: PathBuf,
    sink: LogSink,
}

/// 一个**已经退出**的实例留下的遗言：为什么死的 + 日志还在哪。
///
/// 留着它是为了两件事：工作台要说清"它死了、退出码是多少"，以及崩溃后的日志仍然可读
/// （这正是最需要日志的时刻）。
struct Terminal {
    exit: String,
    sink: LogSink,
}

/// 回收任务与主对象共享的状态。用 `Arc` 而不是 `&self`：任务要活得比调用长。
#[derive(Default)]
struct Shared {
    running: Mutex<BTreeMap<String, Running>>,
    terminal: Mutex<BTreeMap<String, Terminal>>,
    reaper_running: AtomicBool,
}

/// 锁中毒不该让整条链路崩掉：一个读线程 panic 后，若另一个线程 `unwrap()` 也 panic，
/// 管道就再也没人读了 —— 那正是我们要避免的"日志把代理卡死"。
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct MitmproxyCore {
    config: MitmproxyCoreConfig,
    clock: Arc<dyn ClockPort>,
    python: Mutex<Option<PythonInterpreter>>,
    shared: Arc<Shared>,
}

impl MitmproxyCore {
    pub fn new(config: MitmproxyCoreConfig, clock: Arc<dyn ClockPort>) -> Result<Self, Error> {
        if !config.core_bin.exists() {
            // 允许"PATH 解析"的形态：交给 Command 去报错，但字体要清楚
            if config.core_bin.components().count() > 1 {
                return Err(Error::invalid_config(
                    "core.bin",
                    format!("{} does not exist", config.core_bin.display()),
                ));
            }
        }
        Ok(Self {
            config,
            clock,
            python: Mutex::new(None),
            shared: Arc::new(Shared::default()),
        })
    }

    /// 解释器发现 + **版本一致性校验**。
    ///
    /// CA 的参数由生成方决定，用不同版本的 mitmproxy 生成会引入两套默认值 ——
    /// 这正是"同一份 CA、两条代码路径"的经典分叉点，所以不一致就响亮失败。
    /// 「谁来当 `mitmdump`」—— 裸命令名先经 PATH 解析（否则探测解释器时读不到 shebang）。
    fn resolved_core_bin(&self) -> PathBuf {
        python::resolve_command(&self.config.core_bin)
            .unwrap_or_else(|| self.config.core_bin.clone())
    }

    fn interpreter(&self) -> Result<PythonInterpreter, Error> {
        if let Some(cached) = lock(&self.python).clone() {
            return Ok(cached);
        }
        let interpreter = python::discover(&self.config.core_bin, self.config.python.as_deref())?;
        let core_version = self.core_version();
        if let Some(core_version) = core_version
            && core_version != interpreter.mitmproxy_version
        {
            return Err(Error::invalid_config(
                "core.python",
                format!(
                    "interpreter {} has mitmproxy {} but {} reports {} — CA parameters are decided \
                     by the generating side, so they must match",
                    interpreter.path.display(),
                    interpreter.mitmproxy_version,
                    self.config.core_bin.display(),
                    core_version
                ),
            ));
        }
        *lock(&self.python) = Some(interpreter.clone());
        Ok(interpreter)
    }

    /// `mitmdump --version` 里报的版本（拿不到就 `None`）。
    pub fn core_version(&self) -> Option<String> {
        let output = std::process::Command::new(&self.config.core_bin)
            .arg("--version")
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        python::parse_core_version(&text).map(|parsed| parsed.version)
    }

    fn key(env: &str, listen: Listen) -> String {
        format!("{env}@{}", listen.port)
    }

    /// 按启动契约拼参数。
    ///
    /// **只有 mitmproxy 自己的选项**：`-s` 指到本环境的注入器，`confdir` 指向共享目录，
    /// 监听地址由环境决定，`proxyauth` 只在启用了代理鉴权时下发。
    ///
    /// 其余一切（状态文件、规则绑定、放行域名清单、轮询间隔、注解开关）都走
    /// `<自身目录>/config.json` —— 那条通道让它们能**热生效**，而且没有"用户可控的
    /// `--set`"这种能把契约绕过去的入口。
    ///
    /// 凭据经命令行而不是环境变量：DSH 会清洗子进程环境里的凭据形状变量（本工作区
    /// 已确立的规矩），而且命令行更可观测、更好复现。
    /// 代价是 `ps` 能看到 `proxyauth=…`（记录进账本前会脱敏，见 `redact_cmdline`）。
    pub fn build_args(&self, spec: &InstanceSpec, injector_path: &Path) -> Vec<String> {
        let mut args: Vec<String> = vec!["-s".into(), injector_path.display().to_string()];
        let mut set = |key: &str, value: String| {
            args.push("--set".into());
            args.push(format!("{key}={value}"));
        };

        set("confdir", spec.shared_state_dir.display().to_string());
        set("listen_host", spec.listen.host.to_string());
        set("listen_port", spec.listen.port.to_string());
        if let Some(proxyauth) = spec.proxyauth() {
            set("proxyauth", proxyauth);
        }
        args
    }

    async fn wait_for_status(&self, path: &Path) -> Result<StatusReport, Error> {
        let deadline = std::time::Instant::now()
            + Duration::from_secs(self.config.startup_timeout_secs.max(1));
        let mut last: Option<String> = None;
        while std::time::Instant::now() < deadline {
            if let Ok(raw) = std::fs::read(path) {
                match StatusReport::from_json_slice(&raw) {
                    Ok(report) => {
                        // 新鲜度也要成立：否则读到的是上一轮留下的陈旧文件
                        let now = self.clock.now_unix();
                        if now.saturating_sub(report.updated_at) <= 15 {
                            return Ok(report);
                        }
                        last = Some("status file is stale".to_string());
                    }
                    Err(error) => last = Some(error.message),
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err(Error::new(
            ErrorCode::ConfigMismatch,
            format!(
                "instance did not report a fresh status file within {}s ({})",
                self.config.startup_timeout_secs,
                last.unwrap_or_else(|| "no status file".to_string())
            ),
        ))
    }

    /// 起一个回收任务（已有就复用）。
    ///
    /// 任务只做一件事：每 [`REAP_INTERVAL`] 对表里的子进程调一次 `try_wait()`。
    /// 返回 `Some(status)` 就是"它死了" —— 这时才把条目挪到 `terminal`（留下退出原因与
    /// 日志去向），并删掉状态文件。表空了任务就自己退出，下一次启动再起一个。
    fn ensure_reaper(&self) {
        let shared = Arc::clone(&self.shared);
        if shared.reaper_running.swap(true, Ordering::SeqCst) {
            return;
        }
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(REAP_INTERVAL).await;
                if reap_finished(&shared) {
                    break;
                }
            }
            shared.reaper_running.store(false, Ordering::SeqCst);
        });
    }
}

/// 收掉已经退出的子进程；返回"表已经空了"。
fn reap_finished(shared: &Shared) -> bool {
    let mut finished: Vec<(String, String)> = Vec::new();
    {
        let mut running = lock(&shared.running);
        for (key, entry) in running.iter_mut() {
            // `try_wait` 同步、不阻塞，并且**会把子进程 reap 掉** —— 这正是消僵尸的那一下。
            match entry.child.try_wait() {
                Ok(Some(status)) => finished.push((key.clone(), describe_exit(status))),
                Ok(None) => {}
                Err(error) => finished.push((
                    key.clone(),
                    format!("cannot collect the exit status: {error}"),
                )),
            }
        }
    }

    for (key, exit) in finished {
        if let Some(entry) = lock(&shared.running).remove(&key) {
            // 状态文件跟着实例走：留着它会让"新鲜"与"存活"两个判据互相打架。
            let _ = std::fs::remove_file(&entry.status_file);
            lock(&shared.terminal).insert(
                key,
                Terminal {
                    exit,
                    sink: entry.sink,
                },
            );
        }
    }

    lock(&shared.running).is_empty()
}

fn describe_exit(status: std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("the proxy core exited on its own with status {code}"),
        (None, Some(signal)) => format!("the proxy core was killed by signal {signal}"),
        (None, None) => "the proxy core exited on its own".to_string(),
    }
}

#[async_trait]
impl ProxyCore for MitmproxyCore {
    fn describe(&self) -> CoreInfo {
        CoreInfo {
            name: "mitmproxy".to_string(),
            version: self.core_version().unwrap_or_else(|| "unknown".to_string()),
        }
    }

    fn capabilities(&self) -> CoreCapabilities {
        CoreCapabilities {
            listen: true,
            dynamic_certs: true,
            rewrite_upstream: true,
            // 按域名放宽上游证书校验：注入器在 `tls_start_server` 里按 SNI 精确判定。
            per_domain_insecure: true,
            shared_ca: true,
            flow_annotation: true,
            // 注入器如实回显规则条数与选项核对结论 → 管理器的比对逻辑全部可用。
            reports_rules_count: true,
            // 实例是**独立的外部进程**：孤儿清理要读 /proc。
            external_processes: true,
        }
    }

    async fn start(&self, spec: InstanceSpec) -> Result<InstanceHandle, Error> {
        let interpreter = self.interpreter()?;
        // 每个环境一个目录：注入器、`config.json` 与规则软链都在一起，
        // 于是"注入器只认自己目录旁边的固定名"这条纪律才落得下来。
        let injector_path = injector::materialize(&spec.agent_dir)?;
        // 顺序要紧：CA 必须在**任何实例被拉起之前**就绪。
        let ca_info = ca::ensure(
            &interpreter.path,
            &spec.shared_state_dir,
            &interpreter.mitmproxy_version,
        )?;

        std::fs::create_dir_all(&spec.runtime_dir)?;

        // 日志去向（两种形态，**不能混**）：
        //
        // * 给了 `log_dir`（默认）：子进程的 stdout/stderr **直接接文件**。内核负责写盘，
        //   我们进程完全不在链路上 —— 于是"没人读管道 → 管道写满 → 子进程阻塞在写日志上
        //   → 整个代理挂住"这条路径从设计上不存在（实测：不读管道时第 213 个请求就卡死）。
        //   顺带的好处是日志跨重启留存，崩溃现场可追溯。
        // * `log_dir = None`：用管道读进有界环形缓冲（内存态，`--no-log-file`）。
        //   这条路径**必须**持续把管道读走，否则同样会卡死子进程。
        let args = self.build_args(&spec, &injector_path);
        let mut command = Command::new(self.resolved_core_bin());
        command
            .args(&args)
            .stdin(Stdio::null())
            // **必须**：Python 的 stdout/stderr 在不是 TTY 时（管道与文件**都是**）是块缓冲的。
            // 不设这个变量，注入器与 mitmproxy 的输出会攒到几 KB 才吐一次，工作台"看尾部"
            // 就长期是空的（实测踩到过）。
            .env("PYTHONUNBUFFERED", "1")
            // 父进程死掉时把子进程一起带走 —— Linux 上这是最可靠的做法（M0.5 spike 4a 实测）。
            .kill_on_drop(false);

        let sink = match spec.log_dir.as_ref() {
            Some(dir) => {
                std::fs::create_dir_all(dir)?;
                let path = dir.join(format!("{}.log", spec.env));
                // 追加而非截断：跨重启的历史要留着，运行标记负责分段。
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)?;
                let _ = writeln!(
                    file,
                    "--- envboard: env={} listen={} started={} ---",
                    spec.env,
                    spec.listen,
                    self.clock.now_unix()
                );
                let stderr_file = file.try_clone()?;
                command
                    .stdout(Stdio::from(file))
                    .stderr(Stdio::from(stderr_file));
                LogSink::File
            }
            None => {
                let ring: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
                command.stdout(Stdio::piped()).stderr(Stdio::piped());
                LogSink::Ring(ring)
            }
        };

        #[cfg(target_os = "linux")]
        unsafe {
            command.pre_exec(|| {
                // SAFETY: 只调用 async-signal-safe 的 prctl；失败也不阻止启动。
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0);
                Ok(())
            });
        }

        let mut child = command.spawn().map_err(|error| {
            Error::new(
                ErrorCode::InvalidConfig,
                format!("cannot spawn {}: {error}", self.config.core_bin.display()),
            )
        })?;
        let pid = child.id().map(|id| id as i32).unwrap_or(0);

        // 管道路径把两路输出读走（文件路径没有管道，`take()` 自然是 None）。
        if let LogSink::Ring(ring) = &sink {
            if let Some(stdout) = child.stdout.take() {
                spawn_log_reader(stdout, Arc::clone(ring));
            }
            if let Some(stderr) = child.stderr.take() {
                spawn_log_reader(stderr, Arc::clone(ring));
            }
        }

        if let Err(error) = self.wait_for_status(&spec.status_file).await {
            // 启动失败要把子进程收掉，否则留下占着端口的孤儿
            let mut child = child;
            let _ = child.start_kill();
            let _ = child.wait().await;
            let _ = std::fs::remove_file(&spec.status_file);
            return Err(error);
        }

        let identity = read_identity(pid)
            .unwrap_or(ProcessIdentity {
                pid,
                starttime: 0,
                cmdline: std::iter::once(self.resolved_core_bin().display().to_string())
                    .chain(args.iter().cloned())
                    .collect(),
            })
            // 记录之前脱敏：账本是长期留存的产物，凭据没有理由被复制进去。
            .redacted();

        let handle = InstanceHandle {
            env: spec.env.clone(),
            listen: spec.listen,
            process: Some(identity.clone()),
        };
        let key = Self::key(&spec.env, spec.listen);
        // 这一轮是全新实例：上一轮的"遗言"作废（否则 last_exit 会把旧退出码报给新实例）。
        lock(&self.shared.terminal).remove(&key);
        lock(&self.shared.running).insert(
            key.clone(),
            Running {
                child,
                identity,
                status_file: spec.status_file.clone(),
                sink,
            },
        );

        // 有人得替这个子进程收尸：它自己死掉时若无人 `try_wait()`，就会一直以僵尸形态
        // 挂在进程表里（实测），而且"PID + starttime 还在"会让判活误以为它还活着。
        self.ensure_reaper();

        let _ = ca_info;
        Ok(handle)
    }

    async fn stop(&self, handle: &InstanceHandle) -> Result<(), Error> {
        let key = Self::key(&handle.env, handle.listen);
        // 先摘出表：回收任务只处理表里的条目，这样两边不会抢同一个 `Child`。
        let Some(mut running) = lock(&self.shared.running).remove(&key) else {
            return Err(Error::new(
                ErrorCode::NotFound,
                format!(
                    "no running mitmproxy instance for environment {:?}",
                    handle.env
                ),
            ));
        };
        // 用户主动停的，不是崩溃：遗言不该留成 `last_exit`（否则下一次读健康会说"它崩了"）。
        lock(&self.shared.terminal).remove(&key);

        // 先温和：SIGTERM，给它一点时间做清理（状态文件、连接）。
        unsafe { libc::kill(running.identity.pid, libc::SIGTERM) };
        let graceful = tokio::time::timeout(Duration::from_secs(3), running.child.wait()).await;
        if graceful.is_err() {
            let _ = running.child.start_kill();
            let _ = running.child.wait().await;
        }
        let _ = std::fs::remove_file(&running.status_file);
        Ok(())
    }

    /// 进程**已经退出**时的说明（同步）：视图层不能 `await`，但需要给出准确结论。
    fn last_exit(&self, handle: &InstanceHandle) -> Option<String> {
        lock(&self.shared.terminal)
            .get(&Self::key(&handle.env, handle.listen))
            .map(|terminal| terminal.exit.clone())
    }

    fn logs_tail(&self, handle: &InstanceHandle, lines: usize) -> Vec<String> {
        let key = Self::key(&handle.env, handle.listen);
        let sink = {
            let running = lock(&self.shared.running);
            running.get(&key).map(|entry| entry.sink.clone())
        }
        // 实例已经死了也要能看日志 —— 崩溃现场正是最需要它的时候。
        .or_else(|| {
            lock(&self.shared.terminal)
                .get(&key)
                .map(|terminal| terminal.sink.clone())
        });
        match sink {
            Some(LogSink::Ring(ring)) => {
                let ring = lock(&ring);
                let start = ring.len().saturating_sub(lines.max(1));
                ring.iter().skip(start).cloned().collect()
            }
            // 文件模式的尾部由管理器读（文件在它的状态目录里）；这里返回空，
            // 管理器会接着去读 `<log_dir>/<env>.log`。
            Some(LogSink::File) | None => Vec::new(),
        }
    }

    async fn probe(&self, handle: &InstanceHandle) -> InstanceHealth {
        let key = Self::key(&handle.env, handle.listen);
        let status_file = {
            let running = lock(&self.shared.running);
            running.get(&key).map(|r| r.status_file.clone())
        };

        // 进程还活着吗？**按身份比**（PID + 启动时刻），否则 PID 复用会骗过我们。
        if let Some(expected) = handle.process.as_ref() {
            let alive = read_identity(expected.pid)
                .map(|current| {
                    current.pid == expected.pid && current.starttime == expected.starttime
                })
                .unwrap_or(false);
            if !alive {
                // 我们手里有它的身份（= 它是被拉起来、且没人叫它停），现在进程没了：
                // 那是**它自己死的**。回收任务还没来得及写下"遗言"时也必须这么报，
                // 否则会出现一瞬间的 `stopped` —— 而"我停的"是另一件事。
                return match self.last_exit(handle) {
                    Some(reason) => InstanceHealth::Failed { reason },
                    None => InstanceHealth::Failed {
                        reason: "the proxy core process is gone (nobody asked it to stop)"
                            .to_string(),
                    },
                };
            }
        } else if status_file.is_none() {
            return InstanceHealth::Stopped;
        }

        let Some(status_file) = status_file else {
            return InstanceHealth::Unhealthy {
                reason: "no status file recorded".into(),
            };
        };
        let Ok(raw) = std::fs::read(&status_file) else {
            return InstanceHealth::Unhealthy {
                reason: format!("status file {} is missing", status_file.display()),
            };
        };
        let report = match StatusReport::from_json_slice(&raw) {
            Ok(report) => report,
            Err(error) => {
                return InstanceHealth::Unhealthy {
                    reason: error.message,
                };
            }
        };
        if let Some(reason) = report.last_error.clone() {
            return InstanceHealth::Unhealthy { reason };
        }
        // 契约回显不通过 → 配置没落地。管理器还会比规则条数，这里先报最直接的。
        if let Some(mismatch) = report.options_mismatch() {
            return InstanceHealth::ConfigMismatch { reason: mismatch };
        }
        InstanceHealth::Running
    }
}

/// 把子进程的一路输出读到环形缓冲（`log_dir = None` 的管道路径专用）。
///
/// **不读走管道是有代价的**：管道写满（本机实测 64 KiB）后子进程会阻塞在写日志上 ——
/// 表现为实例莫名卡住、所有客户端一起挂。默认路径之所以改成文件直写，就是为了不让
/// 这条依赖存在。
///
/// 这里是兜底路径，所以三件事都要防：单行无上限会把内存吃光；写盘是阻塞调用，不能
/// 压在 async worker 上；锁中毒不能 `unwrap()`（一个读线程 panic 后另一个也跟着 panic，
/// 管道就再没人读了）。
fn spawn_log_reader<R>(reader: R, ring: Arc<Mutex<VecDeque<String>>>)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    /// 单行上限：超出的截断（日志里出现超长单行时，宁可丢尾部也不能吃光内存）。
    const MAX_LINE_BYTES: usize = 8 * 1024;

    tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut lines = BufReader::new(reader).lines();
        loop {
            let line = match lines.next_line().await {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(_) => break,
            };
            let line = if line.len() > MAX_LINE_BYTES {
                let mut cut = MAX_LINE_BYTES;
                while cut > 0 && !line.is_char_boundary(cut) {
                    cut -= 1;
                }
                format!(
                    "{}… [line truncated at {MAX_LINE_BYTES} bytes]",
                    &line[..cut]
                )
            } else {
                line
            };
            let mut ring = lock(&ring);
            if ring.len() >= LOG_RING_CAPACITY {
                ring.pop_front();
            }
            ring.push_back(line);
        }
    });
}

/// 读 `/proc/<pid>/stat` 的 state（第 3 个字段）与 starttime（第 22 个字段），以及 cmdline。
///
/// 解析要小心：第 2 个字段是 `(comm)`，**comm 里可以含空格与括号**，
/// 所以必须从**最后一个** `)` 之后开始数字段。`starttime` 是把
/// "同一个进程"与"同一个 PID"分开的唯一量；`state` 则把"活着"与
/// "已经死了、只等父进程回收"分开 —— 僵尸的 starttime 与活着时完全一样。
fn read_identity(pid: i32) -> Option<ProcessIdentity> {
    if pid <= 0 {
        return None;
    }
    let raw = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = &raw[raw.rfind(')')? + 1..];
    let mut fields = after_comm.split_whitespace();
    let state = fields.next()?.chars().next()?;
    if matches!(state, 'Z' | 'X' | 'x') {
        return None;
    }
    let starttime: u64 = fields.nth(18)?.parse().ok()?;

    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline"))
        .ok()
        .map(|raw| {
            raw.split(|byte| *byte == 0)
                .filter(|chunk| !chunk.is_empty())
                .map(|chunk| String::from_utf8_lossy(chunk).to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    // 记录之前脱敏：`--set proxyauth=user:password` 的值换成 `***`。
    Some(
        ProcessIdentity {
            pid,
            starttime,
            cmdline,
        }
        .redacted(),
    )
}

/// 便捷构造：从 PATH 找 `mitmdump`。
pub fn default_core_bin() -> PathBuf {
    PathBuf::from("mitmdump")
}

/// 供 CLI 展示：某个监听地址是否可用（不参与核心逻辑）。
pub fn host_of(listen: Listen) -> IpAddr {
    listen.host
}

#[cfg(test)]
mod tests {
    use super::*;
    use envboard_core_api::DEFAULT_LISTEN_HOST;

    fn core(agent_dir: PathBuf) -> MitmproxyCore {
        MitmproxyCore::new(
            MitmproxyCoreConfig {
                // 裸命令名（按 PATH 解析）—— 单元测试不 spawn，只校验参数拼装与能力声明
                core_bin: "mitmdump".into(),
                python: None,
                agent_dir,
                startup_timeout_secs: 2,
            },
            Arc::new(envboard_core_api::ManualClock::new(1_000)),
        )
        .unwrap()
    }

    fn spec() -> InstanceSpec {
        InstanceSpec {
            env: "beta".into(),
            listen: Listen::new(DEFAULT_LISTEN_HOST, 16_301),
            agent_dir: PathBuf::from("/tmp/agent/beta"),
            status_file: PathBuf::from("/tmp/runtime/beta.status.json"),
            shared_state_dir: PathBuf::from("/tmp/shared/confdir"),
            runtime_dir: PathBuf::from("/tmp/runtime"),
            log_dir: None,
            insecure_hosts: vec!["365.kdocs.cn".into()],
            proxy_user: None,
            proxy_password: None,
        }
    }

    #[test]
    fn args_carry_only_the_launch_contract() {
        let core = core(std::env::temp_dir());
        let args = core.build_args(&spec(), Path::new("/tmp/agent/beta/inj.py"));
        let joined = args.join(" ");

        assert!(
            joined.contains("-s /tmp/agent/beta/inj.py"),
            "injector must be passed via -s: {joined}"
        );
        for expected in [
            "confdir=/tmp/shared/confdir",
            "listen_host=127.0.0.1",
            "listen_port=16301",
        ] {
            assert!(
                joined.contains(expected),
                "missing {expected:?} in: {joined}"
            );
        }
        // 配置通道换成了 config.json：这些 envboard_* 键**必须**从命令行消失，
        // 否则就又有两个真相（命令行一份、配置文件一份）。
        for gone in [
            "envboard_rules",
            "envboard_status_file",
            "envboard_reload_interval",
            "envboard_annotate_flow",
            "envboard_env_name",
            "envboard_expect",
        ] {
            assert!(!joined.contains(gone), "{gone} must be gone from: {joined}");
        }
        // 没有用户可控的 --set
        assert!(!joined.contains("ssl_insecure"), "{joined}");
        assert_eq!(args.iter().filter(|arg| *arg == "--set").count(), 3);
    }

    #[test]
    fn proxy_auth_is_passed_as_proxyauth() {
        let core = core(std::env::temp_dir());
        let mut spec = spec();
        spec.proxy_user = Some("alice".into());
        spec.proxy_password = Some("s3cret".into());
        let args = core.build_args(&spec, Path::new("/tmp/agent/beta/inj.py"));
        let joined = args.join(" ");
        assert!(
            joined.contains("proxyauth=alice:s3cret"),
            "proxyauth must be assembled from the two fields: {joined}"
        );
        // 只给一边是非法状态（domain 层已拒绝），这里不应拼出半个凭据
        spec.proxy_password = None;
        let joined = core
            .build_args(&spec, Path::new("/tmp/agent/beta/inj.py"))
            .join(" ");
        assert!(!joined.contains("proxyauth"), "{joined}");
    }

    #[test]
    fn capabilities_declare_external_processes_and_rule_reporting() {
        let capabilities = core(std::env::temp_dir()).capabilities();
        assert!(
            capabilities.external_processes,
            "mitmdump instances are real processes"
        );
        assert!(capabilities.reports_rules_count);
        assert!(capabilities.per_domain_insecure);
        assert!(capabilities.rewrite_upstream);
        assert!(capabilities.shared_ca);
    }

    #[test]
    fn process_identity_reads_this_process() {
        let identity = read_identity(std::process::id() as i32).expect("self must be readable");
        assert!(identity.starttime > 0);
        assert!(!identity.cmdline.is_empty());
    }
}

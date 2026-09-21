//! envboard —— 唯一发布产物就是这个二进制；它唯一的行为是**启动工作台**。
//!
//! 形态（v3）：常驻管理器 + 进程内引擎 + 本地 HTTP API + 内嵌前端，**一个进程**。
//! 没有 CLI 子命令面：一切管理动作走工作台 UI 或本地 HTTP API（端点清单见 README），
//! 状态只有一个写者 —— 本进程（flock 拿不到就响亮失败，说明已有一个在跑）。
//!
//! 鉴权按绑定地址定档：回环默认免鉴权；非回环必须显式 `--token`（安全模型在
//! `envboard-web` 的 config.rs）。这里只翻译参数，不重新判定任何领域规则。

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;

use envboard_admin::AdminService;
use envboard_engine::{CaOutcome, EngineBackend, SharedCa};
use envboard_engine::{ClockPort, Error, ErrorCode, LoggerPort, ProxyEngine};
use envboard_manager::infra::{RealFiles, SocketPortProbe, StderrLogger, SystemClock};
use envboard_manager::{JsonFileStateRepo, Manager, ManagerConfig, StateRepo};
use envboard_web::{CaAssets, WebConfig, serve};

/// 日志体积的照看间隔：全进程**只起一个**照看者（挂在 web 层的 SSE 里就会
/// 每个客户端各干一遍 —— 这是编排职责，归宿主）。
const LOG_MAINTENANCE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Parser, Debug)]
#[command(
    name = "envboard",
    version,
    about = "envboard —— 多环境代理管理器与工作台：一个环境 = 一个实例 = 一个端口"
)]
struct Cli {
    /// 监听地址。回环（默认）免鉴权；非回环必须显式 `--token`。
    #[arg(long, default_value = "127.0.0.1:8900")]
    listen: String,

    /// 启用 token 鉴权。裸给（不带值）自动生成 128 bit 随机值并在启动日志打印
    /// 可点链接；`--token <T>` 用给定值。回环上可不给（默认免鉴权）；非回环必填。
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    token: Option<String>,

    /// 状态目录（环境账本、运行时状态、规则库、共享 CA 都在它下面）。
    ///
    /// 默认 `$ENVBOARD_STATE_DIR`，其次 `~/.envboard`。**读环境变量是适配器的职责**
    /// （core 禁止直接读环境变量）。
    #[arg(long)]
    state_dir: Option<PathBuf>,

    /// 随机分配端口的区间，形如 `16000-16999`。
    #[arg(long)]
    port_range: Option<String>,

    /// 抓包缓冲字节预算（每环境；默认 256 MiB，0 = 引擎默认）。
    #[arg(long, default_value_t = 256 * 1024 * 1024)]
    capture_budget: usize,
    /// 实例日志落盘目录（默认 `<state_dir>/logs`）。
    #[arg(long)]
    log_dir: Option<PathBuf>,

    /// 不落盘：日志只留在内存的有界环形缓冲里（随实例结束而消失）。
    #[arg(long)]
    no_log_file: bool,

    /// 单个环境日志文件的轮转上限（字节，`0` = 不轮转）。
    #[arg(long)]
    max_log_bytes: Option<u64>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // 错误码与字段路径是契约的一部分：入口只翻译，不重新判定。
            eprintln!(
                "envboard: {}: {}{}",
                error.code.as_str(),
                error.message,
                error
                    .field
                    .as_deref()
                    .map(|field| format!(" (field: {field})"))
                    .unwrap_or_default()
            );
            match error.code.retryability() {
                envboard_engine::Retryability::No => ExitCode::from(2),
                _ => ExitCode::from(1),
            }
        }
    }
}

fn run(cli: Cli) -> Result<(), Error> {
    let config = build_config(&cli)?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| Error::internal_error(format!("cannot start tokio runtime: {error}")))?;

    let clock: Arc<dyn ClockPort> = Arc::new(SystemClock);
    let logger: Arc<dyn LoggerPort> = Arc::new(StderrLogger);
    let (core, ca_assets) = build_engine(&config)?;

    let state_file = config.state_file();
    let manager = Manager::new(
        config,
        core,
        Arc::new(SocketPortProbe),
        Arc::new(RealFiles),
        Arc::clone(&clock),
        logger,
        Arc::new(JsonFileStateRepo::new(state_file)) as Arc<dyn StateRepo>,
        // 单实例锁：拿不到就响亮失败（状态只有一个写者）。
        true,
    )
    .map_err(|error| {
        if error.code == ErrorCode::Conflict {
            return Error {
                message: format!(
                    "{} — another envboard workbench already owns this state dir; stop it first",
                    error.message
                ),
                ..error
            };
        }
        error
    })?;

    let web_config = WebConfig::parse(&cli.listen, cli.token.clone(), &manager.config().state_dir)?;
    let manager = Arc::new(manager);
    runtime.block_on(async move {
        // 编排职责（宿主）：先 reconcile（崩溃自愈），再起 HTTP。
        let report = manager.reconcile().await?;
        for (env, action) in &report.actions {
            println!("reconcile: {action:<14} {env}");
        }
        // 每次只是给每个环境做一次 `stat`，只有超上限才真正轮转（copytruncate）。
        manager.maintain_logs();
        {
            let manager = Arc::clone(&manager);
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(LOG_MAINTENANCE_INTERVAL).await;
                    manager.maintain_logs();
                }
            });
        }
        // 组合根把管理器交给 Admin 门面；web 只认识门面（deps 门禁）。
        serve(
            Arc::new(AdminService::new(Arc::clone(&manager))),
            web_config,
            ca_assets,
        )
        .await
    })
}

/// v3 引擎构造：进程内纯库引擎，共享 CA 在 confdir 就绪（兼容既有 mitmproxy
/// 生成的 CA 文件，已装证书的客户端零感知）。
///
/// CA 被重新物化时**必须**把警告打出来 —— 已装证书的客户端要重装，这条不能静默。
/// 引擎构造同时带回共享 CA 的**公开面**（证书摘要 + 证书 PEM）交给 web 层：
/// 设置页展示与下载用。私钥不进 CaAssets，也就永远到不了 HTTP 面。
fn build_engine(config: &ManagerConfig) -> Result<(Arc<dyn ProxyEngine>, CaAssets), Error> {
    let (ca, outcome) = SharedCa::load_or_create(&config.confdir)?;
    if let CaOutcome::Regenerated {
        fingerprint,
        reason,
    } = &outcome
    {
        eprintln!("warning: shared CA was regenerated ({reason}); fingerprint {fingerprint}");
        eprintln!(
            "warning: clients must reinstall the CA from {}",
            config.confdir.display()
        );
    }
    let assets = CaAssets {
        info: ca.info().and_then(|info| serde_json::to_value(info).ok()),
        pem: Arc::new(ca.ca_cert_pem().into_bytes()),
    };
    Ok((Arc::new(EngineBackend::new(ca)), assets))
}

fn state_dir(cli: &Cli) -> Result<PathBuf, Error> {
    cli.state_dir
        .clone()
        .or_else(|| std::env::var_os("ENVBOARD_STATE_DIR").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".envboard")))
        .ok_or_else(|| {
            Error::invalid_config(
                "state_dir",
                "cannot determine a state dir; pass --state-dir",
            )
        })
}

fn build_config(cli: &Cli) -> Result<ManagerConfig, Error> {
    let mut config = ManagerConfig::new(state_dir(cli)?);
    config.capture_budget = cli.capture_budget;
    if let Some(dir) = cli.log_dir.clone() {
        config.log_dir = Some(dir);
    }
    if cli.no_log_file {
        config.log_dir = None;
    }
    if let Some(bytes) = cli.max_log_bytes {
        config.max_log_bytes = bytes;
    }
    if let Some(raw) = &cli.port_range {
        let (min, max) = raw.split_once('-').ok_or_else(|| {
            Error::invalid_config("port_range", format!("expected <min>-<max>, got {raw:?}"))
        })?;
        let min: u16 = min
            .trim()
            .parse()
            .map_err(|_| Error::invalid_config("port_range", format!("bad min port in {raw:?}")))?;
        let max: u16 = max
            .trim()
            .parse()
            .map_err(|_| Error::invalid_config("port_range", format!("bad max port in {raw:?}")))?;
        config.port_range = (min, max);
    }
    config.validate()?;
    Ok(config)
}

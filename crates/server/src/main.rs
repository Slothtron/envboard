//! envboard —— 唯一发布产物就是这个二进制；它唯一的行为是**启动工作台**。
//!
//! 形态（v3）：常驻管理器 + 进程内引擎 + 本地 HTTP API + 内嵌前端，**一个进程**。
//! 没有 CLI 子命令面：一切管理动作走工作台 UI 或本地 HTTP API（端点清单见 README），
//! 状态只有一个写者 —— 本进程（flock 拿不到就响亮失败，说明已有一个在跑）。
//!
//! 本文件只做三件事：**把命令行翻译成组合根入参**、**持有 tokio 运行时**、
//! **把装配好的宿主接到 web 传输面**。引擎装配与管理器构造在 `envboard-host`
//! （唯一组合根；判据由 `envboard-policy-tests` 的 `deps` 门禁在源文件面强制），
//! 领域规则不在这里重新判定。
//!
//! 鉴权按绑定地址定档：回环默认免鉴权；非回环必须显式 `--token`（安全模型在
//! `envboard-web` 的 config.rs）。

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;

use envboard_engine::{Error, Retryability};
use envboard_host::{CaBundle, HostOptions, Prepared, parse_port_range, prepare};
use envboard_web::{CaAssets, WebConfig, serve};

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
    /// （core 禁止直接读环境变量），所以这条回退链在 `envboard-host` 里。
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
                Retryability::No => ExitCode::from(2),
                _ => ExitCode::from(1),
            }
        }
    }
}

fn run(cli: Cli) -> Result<(), Error> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| Error::internal_error(format!("cannot start tokio runtime: {error}")))?;

    let prepared = prepare(host_options(&cli)?)?;
    // 装配期的响亮警告（例如共享 CA 被重新物化：客户端要重装）—— 原样打到 stderr。
    for warning in &prepared.warnings {
        eprintln!("warning: {warning}");
    }

    let web_config = WebConfig::parse(&cli.listen, cli.token.clone(), &prepared.state_dir)?;
    let ca_assets = take_ca_assets(&prepared);
    runtime.block_on(async move {
        // 编排职责在组合根：先 reconcile（崩溃自愈），再起 HTTP。
        let report = prepared.startup().await?;
        for (env, action) in &report.actions {
            println!("reconcile: {action:<14} {env}");
        }
        // web 只认识 Admin 门面（deps 门禁）。
        serve(Arc::clone(&prepared.admin), web_config, ca_assets).await
    })
}

/// CLI → 组合根入参。参数名与帮助文本属于界面形态，这里不掺任何领域判定。
fn host_options(cli: &Cli) -> Result<HostOptions, Error> {
    Ok(HostOptions {
        state_dir: cli.state_dir.clone(),
        port_range: match &cli.port_range {
            Some(raw) => Some(parse_port_range(raw)?),
            None => None,
        },
        capture_budget: cli.capture_budget,
        log_dir: cli.log_dir.clone(),
        no_log_file: cli.no_log_file,
        max_log_bytes: cli.max_log_bytes,
    })
}

/// 组合根的 CA 公开面 → web 层的资产载体。字段搬运，不引入新的判定。
fn take_ca_assets(prepared: &Prepared) -> CaAssets {
    let CaBundle { info, pem } = &prepared.ca;
    CaAssets {
        info: info.clone(),
        pem: Arc::clone(pem),
    }
}

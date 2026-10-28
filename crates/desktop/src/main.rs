//! envboard 桌面形态的二进制入口（发布物名就叫 `envboard`）。
//!
//! 只做三件事：**把命令行翻译成组合根入参**、**装配宿主**、**开界面**。
//! 引擎装配与宿主编排都在 `envboard-host`（唯一组合根，判据见 deps 门禁第 8 条），
//! 领域规则不在这里重新判定。
//!
//! 与已经下线的 web 宿主（`crates/server`）的两处不同：
//!
//! * 不起 HTTP 服务，也就没有 `--listen` / `--token`：界面进程内直连 Admin 门面；
//! * tokio 运行时由本文件的栈帧持有（日志照看与 reconcile 是常驻任务），界面只拿
//!   `Handle`；退出时按超时收尾 —— 照看循环无界，靠默认 Drop 会一直等。

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use gpui_kit::{AppContext as _, WindowBounds, px, size};

use envboard_desktop::{Host, Workbench};
use envboard_engine::Error;
use envboard_host::{HostOptions, parse_port_range, prepare};

/// 首次启动的窗口尺寸与最小尺寸：最小尺寸要容得下「侧栏 + 主从两栏」。
/// 数值与 dipswitch 同一档（同类工作台布局，实测可用）。
const DEFAULT_WINDOW_WIDTH: f32 = 1180.0;
const DEFAULT_WINDOW_HEIGHT: f32 = 760.0;
const MIN_WINDOW_WIDTH: f32 = 880.0;
const MIN_WINDOW_HEIGHT: f32 = 560.0;

/// 退出时给 tokio 运行时的收尾窗口：日志照看循环是无界的，超时后转入后台丢弃。
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Parser, Debug)]
#[command(
    name = "envboard",
    version,
    about = "envboard 工作台（桌面形态）：一个环境 = 一个实例 = 一个端口"
)]
struct Cli {
    /// 状态目录（环境账本、运行时状态、规则库、共享 CA 都在它下面）。
    ///
    /// 默认 `$ENVBOARD_STATE_DIR`，其次 `$HOME/.envboard`。与 web 宿主共用同一份状态，
    /// 所以**两边不能同时开着同一个目录**（单实例锁，拿不到就响亮失败）。
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
            // 错误码与字段路径是契约的一部分：入口只翻译，不重新判定
            // （与 web 宿主同一条退出码规则）。
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
    // 运行时由本栈帧持有，界面只拿 Handle：日志照看与 reconcile 都是常驻任务，
    // 不能压在 GPUI 主线程上。
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| Error::internal_error(format!("cannot start tokio runtime: {error}")))?;

    // 装配 + 单实例锁：拿不到锁必须在这里响亮失败，而不是开一个空白窗口。
    // 包一层 Arc：宿主编排要用它，界面的 Global 也要引用它交付的东西。
    let prepared = Arc::new(prepare(host_options(&cli)?)?);
    // 装配期的警告先打到 stderr；界面里的常驻提示属于设置屏（P2）。
    for warning in &prepared.warnings {
        eprintln!("warning: {warning}");
    }
    let host = Host::new(&prepared, runtime.handle().clone());

    // 宿主编排（reconcile + 日志照看）**不等它完成**：端口定态可能要几秒，
    // 界面先出来显示"取数中"才是对的顺序（也才是 Loading 态的真实用途）。
    runtime.handle().spawn(async move {
        match prepared.startup().await {
            Ok(report) => {
                for (env, action) in &report.actions {
                    println!("reconcile: {action:<14} {env}");
                }
                for warning in &report.warnings {
                    eprintln!("warning: {warning}");
                }
            }
            Err(error) => eprintln!("envboard: startup failed: {}", error.message),
        }
    });

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            // 必须先初始化 Component/Base，再创建任何组件视图。
            gpui_kit::init(cx);
            // 后端进 Global：视图经 cx.global::<Host>() 取门面，不各存一份。
            cx.set_global(host);

            // 自绘标题栏：隐藏系统标题栏，拖拽区与窗口按钮由 TitleBar 绘制。
            let mut options = gpui_kit::WindowOptions {
                window_bounds: Some(WindowBounds::centered(
                    size(px(DEFAULT_WINDOW_WIDTH), px(DEFAULT_WINDOW_HEIGHT)),
                    cx,
                )),
                window_min_size: Some(size(px(MIN_WINDOW_WIDTH), px(MIN_WINDOW_HEIGHT))),
                app_id: Some("envboard".to_owned()),
                ..gpui_kit::component::TitleBar::window_options()
            };
            // 保留窗口标题（任务栏 / 无障碍名称），但标题栏外观完全由组件绘制。
            options.titlebar = Some(gpui_kit::TitlebarOptions {
                title: Some("envboard".into()),
                ..gpui_kit::component::TitleBar::title_bar_options()
            });

            // open_window 会把根视图包进 Root，通知 / 对话框等框架能力依赖它。
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| Workbench::new(window, cx))
            })
            .expect("failed to open window");
        });

    // 退出：日志照看循环是无界的，靠 Runtime 的默认 Drop 会一直等下去，
    // 所以按超时收尾（超时后运行时转入后台丢弃，不阻塞进程退出）。
    runtime.shutdown_timeout(SHUTDOWN_TIMEOUT);
    Ok(())
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

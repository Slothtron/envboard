//! envboard-desktop —— Tauri 壳：桌面是**唯一发布形态**（ADR-006，web 控制台退场）。
//!
//! 数据面走 Tauri commands-IPC：壳进程内直接调用 `AdminService` 门面与组合根交付的
//! `CaControl`，**不起任何监听端口**。界面仍是同一份 `frontend/` SPA（视图层零改动，
//! 只有 `src/api/{client,stream}.ts` 两个数据层文件换了传输）。
//!
//! 分层与 ADR-002 一致：壳只认门面、词汇与组合根交付的公开操作面，不认识引擎装配。
//! 本 crate 只做四件事：**把命令行翻译成组合根入参**、**持有编排（prepare → startup）**、
//! **把门面接到 IPC 命令面**（`commands`/`ops`）、**把两条推送流接到 Channel**（`stream`）。
//!
//! 安全模型随传输换档：token/CSRF/Host 守卫是 HTTP 面的职责，随 web 退场；
//! IPC 面由 Tauri capabilities 圈禁（只对 main 窗开放），CSP 在 tauri.conf.json
//! （镜像原服务端严格 CSP + react-aria 样式 hash 豁免 + ipc: 通道）。
//! CA 私钥边界不变：永远不出组合根。
//!
//! 单实例：组合根的 `InstanceLock`（随 state-dir）—— 拿不到锁就弹错误框退出。
//! 不用 tauri-plugin-single-instance：它是应用级锁，会误伤「两个便携副本各带
//! 各的 state-dir 共存」这条既有语义。
//!
//! 后台运行（ADR-012）：窗口关闭 = 窗口与 WebView 一起销毁，进程与引擎线程常驻；
//! 托盘是回前台的入口（`tray` 模块重建窗口）。托盘不可用时降级为旧语义（关窗即退出）。
//! 窗口几何（位置 / 客户区大小 / 最大化）与主题同住 `ui.json`：启动与重建都会贴回
//! 原样（`geometry`），屏外坐标只恢复尺寸。

mod activation;
pub mod commands;
mod geometry;
pub mod ops;
mod prefs;
mod profile;
pub mod stream;
mod tray;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use clap::Parser;
use envboard_admin::AdminService;
use envboard_engine::{Error, Retryability};
use envboard_host::{CaControl, HostOptions, Prepared, prepare};
use tauri::tray::TrayIcon;
use tauri::{Manager, RunEvent, WindowEvent};
use tauri_plugin_dialog::DialogExt;

/// 壳的共享状态：门面 + CA 操作面 + 推送流注册表 + 后台态。
pub struct Shell {
    pub admin: Arc<AdminService>,
    pub ca: Option<CaControl>,
    /// 流 id → 取消通道的 sender。`unwatch` 丢弃 sender，任务在下一拍退出。
    pub streams: Mutex<HashMap<u64, tokio::sync::watch::Sender<bool>>>,
    next_stream_id: AtomicU64,
    /// 托盘是否就绪：决定「关窗 = 常驻」还是「关窗 = 退出」的降级开关。
    background: AtomicBool,
    /// 托盘句柄必须有人持有（引用计数，drop 即移除图标）。
    tray: Mutex<Option<TrayIcon<tauri::Wry>>>,
    /// 界面偏好（ui.json）：主题落盘在 state-dir，不依赖 WebView localStorage。
    pub ui_prefs: prefs::UiPrefsStore,
    /// 最近一次「正常态」窗口几何（内存）：`Moved`/`Resized` 只更新它，
    /// 关窗/退出时才写 ui.json（见 `geometry`）。
    window_geometry: Mutex<Option<prefs::WindowGeometry>>,
    /// 本进程的形态身份（正式版 / 便携 / 预览 / 开发，ADR-014）：托盘与窗口重建
    /// 要按它决定标题、tooltip 与 WebView2 数据目录。
    pub profile: profile::Profile,
}

impl Shell {
    /// 登记一条新流，返回 (id, 取消接收端)。
    pub fn register_stream(&self) -> (u64, tokio::sync::watch::Receiver<bool>) {
        let id = self.next_stream_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = tokio::sync::watch::channel(false);
        self.streams.lock().unwrap().insert(id, tx);
        (id, rx)
    }

    /// 窗口销毁时清空全部流：丢 sender 让任务在下一拍退出。
    /// 不依赖 `Channel::send` 失败兜底 —— Channel 的闭包持有 `Webview` clone。
    pub fn cancel_all_streams(&self) {
        self.streams.lock().unwrap().clear();
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "envboard",
    version,
    about = "envboard —— 多环境代理管理器与工作台：一个环境 = 一个实例 = 一个端口"
)]
struct Cli {
    /// 状态目录（环境账本、运行时状态、规则库、共享 CA 都在它下面）。
    ///
    /// 默认 `$ENVBOARD_STATE_DIR`，其次便携标记 / 平台默认目录（回退链在
    /// `envboard-host`）。
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

    /// 测试 / 无头场景：启动失败与装配警告不弹框（只走 stderr 与退出码）。
    /// 交互默认是弹框 —— windows 子系统没有控制台，不弹框就等于静默失败。
    #[arg(long)]
    no_dialog: bool,
}

/// 入口。`app_dir_name` 是产品身份，由二进制从自己的 `[[bin]] name` 取来传进
/// （`env!("CARGO_BIN_NAME")` 只在 bin 目标编译期可用，lib 里取不到）。
pub fn run(app_dir_name: &str) -> std::process::ExitCode {
    let cli = Cli::parse();
    // ADR-014：正式版 / 便携版 / 预览版 / 开发版用各自的状态目录、WebView2 数据目录
    // 与端口段，否则换个形态启动就会撞上正在跑的正式版（锁 / 激活事件 / 账本）。
    let profile = profile::Profile::current(app_dir_name);
    let app_name = profile.app_dir_name(app_dir_name);
    if let Some(tag) = profile.tag() {
        eprintln!("envboard: {tag} 形态，数据目录与正式版隔离（{app_name}）");
    }

    // ADR-012：同一 state-dir 的第二个实例只做激活 —— 在 Tauri 之前就能判定，
    // 不建窗口、不拿锁、不写状态。解析 state-dir 失败就照常启动（由 boot 报错）。
    let waiter = match host_options(&cli, &app_name, &profile) {
        Ok(options) => match activation::preflight(&options) {
            activation::Preflight::Activated => {
                eprintln!("envboard: 已有实例在运行，已请求它回到前台");
                return std::process::ExitCode::SUCCESS;
            }
            activation::Preflight::Owner(handle) => Some(handle),
            activation::Preflight::Unavailable => None,
        },
        Err(_) => None,
    };

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let handle = app.handle().clone();
            match boot(&handle, &cli, &app_name, &profile) {
                Ok(shell) => {
                    app.manage(shell);
                    // 托盘是后台运行的入口；创建失败只降级（关窗即退出），不挡启动。
                    tray::install(app, cli.no_dialog);
                    // 首窗也走壳自己的创建路径（config 里 create: false）：隐藏建窗 →
                    // 贴几何 → 首帧后再显示，否则会看到默认位置的窗口再跳到上次的位置。
                    if let Err(error) = tray::create_workbench(app.handle(), true) {
                        eprintln!("envboard: 创建工作台窗口失败：{error}");
                        return Err(error.into());
                    }
                    // 二次启动激活的等待线程：被 signal 就重建/聚焦工作台。
                    if let Some(owner) = waiter {
                        activation::spawn_waiter(app.handle().clone(), owner);
                    }
                    Ok(())
                }
                Err(error) => {
                    // windows 子系统没有控制台：启动失败必须弹框可见，
                    // 同时 stderr 留一份（debug 构建 / 从终端拉起时可读）。
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
                    if !cli.no_dialog {
                        handle
                            .dialog()
                            .message(format!(
                                "{}{}",
                                error.message,
                                error
                                    .field
                                    .as_deref()
                                    .map(|field| format!("\n(field: {field})"))
                                    .unwrap_or_default()
                            ))
                            .title("envboard 启动失败")
                            .kind(tauri_plugin_dialog::MessageDialogKind::Error)
                            .blocking_show();
                    }
                    std::process::exit(match error.code.retryability() {
                        Retryability::No => 2,
                        _ => 1,
                    });
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::status,
            commands::ui_prefs_get,
            commands::ui_prefs_set,
            commands::ca_overview,
            commands::ca_install_cert,
            commands::ca_uninstall_cert,
            commands::env_list,
            commands::env_get,
            commands::env_create,
            commands::env_patch,
            commands::env_delete,
            commands::env_start,
            commands::env_stop,
            commands::env_restart,
            commands::env_reallocate,
            commands::env_logs,
            commands::env_captures,
            commands::env_capture_detail,
            commands::env_capture_clear,
            commands::env_captures_export,
            commands::debug_get,
            commands::debug_start,
            commands::debug_stop,
            commands::har_list,
            commands::har_import,
            commands::har_get,
            commands::har_delete,
            commands::rules_list,
            commands::rules_import,
            commands::rules_get,
            commands::rules_delete,
            commands::proxies_list,
            commands::proxies_put,
            commands::proxies_delete,
            commands::inject_fault,
            stream::watch_environments,
            stream::watch_debug,
            stream::unwatch,
        ])
        .build(tauri::generate_context!());

    let app = match app {
        Ok(app) => app,
        Err(error) => {
            eprintln!("envboard: {error}");
            return std::process::ExitCode::from(1);
        }
    };

    // 后台运行（ADR-012）：
    // - 最后一个窗口销毁（code: None）且托盘就绪 ⇒ 拦住退出，进程与引擎常驻；
    // - 托盘「退出」走 `app.exit(0)`（code: Some），不拦；
    // - 窗口销毁时显式清空推送流，别指望 Channel send 失败兜底。
    app.run(|app, event| match event {
        RunEvent::ExitRequested {
            code: None, api, ..
        } => {
            let background = app
                .try_state::<Shell>()
                .is_some_and(|shell| shell.background.load(Ordering::Relaxed));
            if background && app.webview_windows().is_empty() {
                api.prevent_exit();
            }
        }
        // 关窗在销毁之前把「正常态」几何落盘（ui.json）。
        RunEvent::WindowEvent {
            event: WindowEvent::CloseRequested { .. },
            label,
            ..
        } if label == tray::MAIN_LABEL => {
            if let Some(window) = app.get_webview_window(tray::MAIN_LABEL) {
                geometry::save(app, &window);
            }
        }
        // 拖动 / 改尺寸只更新内存：事件很密，落盘留给关窗/退出。
        RunEvent::WindowEvent {
            event: WindowEvent::Moved(_) | WindowEvent::Resized(_),
            label,
            ..
        } if label == tray::MAIN_LABEL => geometry::track(app),
        // 托盘「退出」时窗口可能还开着：顺手记一次，下次启动回到原位。
        RunEvent::ExitRequested { code: Some(_), .. } => {
            if let Some(window) = app.get_webview_window(tray::MAIN_LABEL) {
                geometry::save(app, &window);
            }
        }
        RunEvent::WindowEvent {
            event: WindowEvent::Destroyed,
            label,
            ..
        } if label == tray::MAIN_LABEL => {
            if let Some(shell) = app.try_state::<Shell>() {
                shell.cancel_all_streams();
            }
        }
        _ => {}
    });

    // `App::run` 不返回（进程随事件循环退出），这里只为类型完整。
    std::process::ExitCode::SUCCESS
}

/// 壳加固（ADR-009）：禁 WebView2 默认右键菜单与浏览器加速键
/// （Ctrl+R/F5 刷新、Ctrl+P 打印、Ctrl+F 查找、缩放、DevTools 键）。
/// MS 文档明确**保留编辑键**（Ctrl+C/V/X/A/Z 等）——禁的是浏览器行为，
/// 不是输入能力。
///
/// wry 有这两个开关但 tauri 不透传，唯一 native 入口是 with_webview 拿
/// controller。时机 caveat：NavigationStarting 之后改 settings 按文档语义
/// 可能下次顶层导航才生效，故前端 main.tsx 有同语义 JS 守卫兜底首帧；
/// 这里每步 best-effort，失败不挡启动。窗口重建（托盘）后必须再调一次。
#[cfg(windows)]
fn harden_window(window: &tauri::WebviewWindow) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings3;
    use windows_core::Interface;

    let _ = window.with_webview(|webview| {
        // SAFETY: with_webview 契约在主线程调 COM；句柄只在本闭包内使用。
        unsafe {
            let Ok(core) = webview.controller().CoreWebView2() else {
                return;
            };
            let Ok(settings) = core.Settings() else {
                return;
            };
            let _ = settings.SetAreDefaultContextMenusEnabled(false);
            if let Ok(settings3) = settings.cast::<ICoreWebView2Settings3>() {
                let _ = settings3.SetAreBrowserAcceleratorKeysEnabled(false);
            }
        }
    });
}

/// 非 Windows：native 层无 WebView2，前端 JS 守卫覆盖右键与刷新键。
#[cfg(not(windows))]
fn harden_window(_window: &tauri::WebviewWindow) {}

/// 启动编排：组合根入参 → prepare → 装配警告 → startup（reconcile + 日志照看）。
/// 与退场的 server 宿主同序：先崩溃自愈，再让界面取数。
fn boot(
    handle: &tauri::AppHandle,
    cli: &Cli,
    app_name: &str,
    profile: &profile::Profile,
) -> Result<Shell, Error> {
    let prepared: Prepared = prepare(host_options(cli, app_name, profile)?)?;
    // 装配期的响亮警告（例如共享 CA 被重新物化：客户端要重装）——不能静默。
    if !prepared.warnings.is_empty() && !cli.no_dialog {
        handle
            .dialog()
            .message(prepared.warnings.join("\n"))
            .title("envboard 装配警告")
            .kind(tauri_plugin_dialog::MessageDialogKind::Warning)
            .blocking_show();
    }
    let report = tauri::async_runtime::block_on(prepared.startup())?;
    for (env, action) in &report.actions {
        eprintln!("reconcile: {action:<14} {env}");
    }
    Ok(Shell {
        admin: Arc::clone(&prepared.admin),
        ca: prepared.ca.clone(),
        streams: Mutex::new(HashMap::new()),
        next_stream_id: AtomicU64::new(1),
        background: AtomicBool::new(false),
        tray: Mutex::new(None),
        ui_prefs: prefs::UiPrefsStore::new(prepared.ui_prefs_file.clone()),
        window_geometry: Mutex::new(None),
        profile: profile.clone(),
    })
}

/// CLI → 组合根入参。参数名与帮助文本属于界面形态，这里不掺任何领域判定。
fn host_options(
    cli: &Cli,
    app_name: &str,
    profile: &profile::Profile,
) -> Result<HostOptions, Error> {
    Ok(HostOptions {
        state_dir: cli.state_dir.clone(),
        app_dir_name: Some(app_name.to_owned()),
        port_range: match &cli.port_range {
            Some(raw) => Some(parse_port_range(raw)?),
            // 开发版 / 便携版 / 预览版换一段端口：否则两个形态从同一个 16000-16999
            // 里挑，撞上正在跑的正式版环境就是启动失败（账本各自独立，分配不会互相避让）。
            None if profile.is_non_stable() => Some(profile::NON_STABLE_PORT_RANGE),
            None => None,
        },
        capture_budget: cli.capture_budget,
        log_dir: cli.log_dir.clone(),
        no_log_file: cli.no_log_file,
        max_log_bytes: cli.max_log_bytes,
    })
}

/// `--port-range` 的形状翻译（`<min>-<max>`，空白容忍）。顺序与区间合法性
/// 由 `ManagerConfig::validate` 裁决，这里只管形状。
fn parse_port_range(raw: &str) -> Result<(u16, u16), Error> {
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
    Ok((min, max))
}

#[cfg(test)]
mod tests {
    use super::{Cli, host_options, parse_port_range};
    use crate::profile::{NON_STABLE_PORT_RANGE, Profile};

    fn cli() -> Cli {
        Cli {
            state_dir: None,
            port_range: None,
            capture_budget: 0,
            log_dir: None,
            no_log_file: false,
            max_log_bytes: None,
            no_dialog: true,
        }
    }

    #[test]
    fn port_range_accepts_the_documented_shape() {
        assert_eq!(parse_port_range("16000-16999").unwrap(), (16000, 16999));
        // 空白容忍：`" 16000 - 16999 "` 是同一个意思。
        assert_eq!(parse_port_range(" 16000 - 16999 ").unwrap(), (16000, 16999));
    }

    #[test]
    fn port_range_rejects_bad_shapes_with_the_field_path_set() {
        for raw in ["16000", "16000:", "abc-16999", "16000-70000"] {
            let error = parse_port_range(raw).expect_err("shape must be rejected");
            assert_eq!(error.field.as_deref(), Some("port_range"), "got {error:?}");
        }
    }

    #[test]
    fn non_stable_profiles_default_to_a_disjoint_port_range() {
        for tag in ["dev", "preview", "portable"] {
            let profile = Profile::with_tag(Some(tag));
            let options = host_options(&cli(), "envboard", &profile).expect("options");
            assert_eq!(options.port_range, Some(NON_STABLE_PORT_RANGE), "tag={tag}");
        }

        // 正式版沿用引擎默认（`None` = ManagerConfig 自己推）。
        let release = Profile::with_tag(None);
        let options = host_options(&cli(), "envboard", &release).expect("options");
        assert_eq!(options.port_range, None);
    }

    #[test]
    fn explicit_port_range_wins_over_the_non_stable_default() {
        let explicit = Cli {
            port_range: Some("18000-18010".to_owned()),
            ..cli()
        };
        let dev = Profile::with_tag(Some("dev"));
        let options = host_options(&explicit, "envboard-dev", &dev).expect("options");
        assert_eq!(options.port_range, Some((18_000, 18_010)));
    }
}

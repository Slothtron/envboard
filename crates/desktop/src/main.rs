// GUI 子系统：双击启动时**不开控制台窗口**。
//
// 发布物是给启动器/资源管理器点开的窗口应用，默认的 CONSOLE 子系统会让 Windows
// 给每个实例配一个黑框（双击时一闪，或干脆一直挂着）。`debug_assertions` 下保持
// 控制台：`cargo run` 要看日志、崩溃要看回溯；打包（release）走 GUI。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! envboard 桌面形态的二进制入口（发布物名就叫 `envboard`）。
//!
//! 只做三件事：**组装组合根入参**、**装配宿主**、**开界面**。
//! 引擎装配与宿主编排都在 `envboard-host`（唯一组合根，判据见 deps 门禁第 8 条），
//! 领域规则不在这里重新判定。
//!
//! **没有命令行参数**。这是从启动器/程序坞点开的 GUI 应用，参数等于"藏在看不见的
//! 地方的配置"：用户永远拿不到，却要为它维护一套解析与帮助文本。所以端口区间、
//! 日志目录、抓包预算全部退回"代码里的默认值 + 状态目录布局"，要暴露的话走设置屏
//! （那里能看到、也能持久化）。唯一的外部输入是 `$ENVBOARD_STATE_DIR`、各平台的
//! 状态目录约定，以及 exe 同目录的便携标记 `envboard.portable`（绿色免安装分发靠
//! 它把数据放在 exe 旁边）—— 那三样是**环境与分发形态**的事，不是参数的事。
//!
//! 与已经下线的 web 宿主（`crates/server`）的两处不同：
//!
//! * 不起 HTTP 服务，也就没有 `--listen` / `--token`：界面进程内直连 Admin 门面；
//! * tokio 运行时由本文件的栈帧持有（日志照看与 reconcile 是常驻任务），界面只拿
//!   `Handle`；退出时按超时收尾 —— 照看循环无界，靠默认 Drop 会一直等。

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{AppContext as _, WindowBounds, px, size};

use envboard_desktop::{Host, UiPrefs, Workbench};
use envboard_engine::Error;
use envboard_host::{HostOptions, prepare};

/// 首次启动的窗口尺寸（与 dipswitch 同一档：同类工作台布局，实测可用）。
const DEFAULT_WINDOW_WIDTH: f32 = 1180.0;
const DEFAULT_WINDOW_HEIGHT: f32 = 760.0;
/// 最小尺寸的下限由**用满的界面**决定，不由"好看"决定：
///
/// * 宽度：环境表固定列 596 + 说明列下限 100 = 696，加侧栏 248 与内容区内边距
///   32 得 976；取 1000 留 24 余量。原来的 880 是按"侧栏 + 主从两栏"估的，
///   那个宽度下表格的操作列（启停/重启/删除的唯一入口）会被整个裁出视口。
/// * 高度：主从两栏的最小可用高度（工具栏 + 状态行 + 表头 + 数行数据）。
const MIN_WINDOW_WIDTH: f32 = 1000.0;
const MIN_WINDOW_HEIGHT: f32 = 560.0;

/// 退出时给 tokio 运行时的收尾窗口：日志照看循环是无界的，超时后转入后台丢弃。
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

/// 应用名 —— 只在这里出现一次，其余全部引它。
///
/// 取自 `Cargo.toml` 的 `[[bin]] name`（cargo 编译期注入的 `CARGO_BIN_NAME`），
/// 不在代码里重复一份字面量：它是**产品身份**，改名的唯一落点应该是 manifest。
/// 两个消费者：错误前缀，以及平台默认状态目录里 `<基础目录>/<这个名字>` 那一层。
const APP_NAME: &str = env!("CARGO_BIN_NAME");

fn main() -> ExitCode {
    // 双击启动的人没有控制台：崩溃与致命错误都必须落到**看得见**的地方。
    install_fatal_reporting();
    // 这个二进制**不解析命令行参数**，但旧文档或旧习惯里可能还带着 `--state-dir`
    // 之类。静默忽略会让人以为生效了，所以给一条提示——不阻断启动（可能只是从
    // 某个带参的快捷方式点开）。
    let extra: Vec<String> = std::env::args().skip(1).collect();
    if !extra.is_empty() {
        eprintln!(
            "{APP_NAME}: 不接受命令行参数（收到 {extra:?}），已忽略；\
             换状态目录请设 ENVBOARD_STATE_DIR"
        );
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // 错误码与字段路径是契约的一部分：入口只翻译，不重新判定
            // （与 web 宿主同一条退出码规则）。
            let detail = format!(
                "{}: {}{}",
                error.code.as_str(),
                error.message,
                error
                    .field
                    .as_deref()
                    .map(|field| format!(" (field: {field})"))
                    .unwrap_or_default()
            );
            eprintln!("{APP_NAME}: {detail}");
            // 从资源管理器双击时 stderr 句柄不存在，上面那行写进虚空 —— 用户只看
            // 到"闪退"。所以同一句话再弹一个原生对话框（终端/管道里不弹，见
            // `show_fatal_dialog`）。
            let hint = if error.field.as_deref() == Some("state_dir") {
                "\n\n换状态目录：设置环境变量 ENVBOARD_STATE_DIR 指向一个可写目录。"
            } else {
                ""
            };
            show_fatal_dialog(&format!("{APP_NAME} 无法启动：\n\n{detail}{hint}"));
            match error.code.retryability() {
                envboard_engine::Retryability::No => ExitCode::from(2),
                _ => ExitCode::from(1),
            }
        }
    }
}

/// 让**没有控制台**的启动方式（资源管理器双击、开始菜单、任务栏）也能看到崩溃。
///
/// 为什么需要：本二进制是 GUI 子系统进程，双击时根本没有控制台，stderr 也无处可去；
/// `open_window` 失败、线程 panic 这些路径原来都只留一行写到虚空的输出，症状就是
/// "窗口一闪就没了"。装一个 panic 钩子把它变成对话框，是这条链路上唯一能被用户
/// 看见的落点。
fn install_fatal_reporting() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|text| (*text).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_owned());
        show_fatal_dialog(&format!("{APP_NAME} 遇到内部错误并已停止：\n\n{payload}"));
    }));
}

/// 把一句致命错误弹成原生对话框 —— **只在 stderr 没人读时**。
///
/// 判的不是"句柄在不在"，而是"那句话到底有没有人看见"，三类情况：
///
/// * **附着控制台**（`GetConsoleMode` 成功）：终端里跑，`eprintln` 就在眼前，不弹；
/// * **重定向到文件/管道**（`GetFileType` 是 disk/pipe）：日志或 CI 收走了，不弹
///   （否则一条无人应答的模态框会把自动化卡死）；
/// * 其余（句柄为空 = 资源管理器双击；句柄指向别处的控制台 = `cmd start` 那样继承来的
///   句柄、但本进程并未附着）：**弹**。这正是"窗口一闪就没了"的那两种启动方式。
#[cfg(windows)]
fn show_fatal_dialog(message: &str) {
    use std::ffi::c_void;
    use std::ptr;

    // 只声明用到的四个符号：为"一次 MessageBox"引一个 `windows-sys` 直接依赖会
    // 牵动依赖门禁与锁文件，不划算。`HANDLE` 用 `*mut c_void` 表达。
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn GetConsoleMode(handle: *mut c_void, mode: *mut u32) -> i32;
        fn GetFileType(handle: *mut c_void) -> u32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(hwnd: *mut c_void, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }

    const STD_ERROR_HANDLE: u32 = u32::MAX - 11; // (DWORD)-12
    const INVALID_HANDLE_VALUE: *mut c_void = -1isize as *mut c_void;
    const FILE_TYPE_DISK: u32 = 1;
    const FILE_TYPE_PIPE: u32 = 3;
    const MB_OK: u32 = 0x0;
    const MB_ICONERROR: u32 = 0x10;
    const MB_SETFOREGROUND: u32 = 0x1_0000;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    // SAFETY: 三个查询只读句柄状态，缓冲区都在本栈帧上。
    let stderr = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    let has_reader = if stderr.is_null() || stderr == INVALID_HANDLE_VALUE {
        false
    } else {
        let mut mode = 0u32;
        let attached_console = unsafe { GetConsoleMode(stderr, &mut mode) } != 0;
        let redirected = matches!(
            unsafe { GetFileType(stderr) },
            FILE_TYPE_DISK | FILE_TYPE_PIPE
        );
        attached_console || redirected
    };
    if has_reader {
        return;
    }

    let body = wide(message);
    let caption = wide("envboard");
    // SAFETY: 两个缓冲区在调用期间有效（同步调用，返回后才释放）。
    unsafe {
        MessageBoxW(
            ptr::null_mut(),
            body.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

#[cfg(not(windows))]
fn show_fatal_dialog(_message: &str) {}

fn run() -> Result<(), Error> {
    // 运行时由本栈帧持有，界面只拿 Handle：日志照看与 reconcile 都是常驻任务，
    // 不能压在 GPUI 主线程上。
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| Error::internal_error(format!("cannot start tokio runtime: {error}")))?;

    // 装配 + 单实例锁：拿不到锁必须在这里响亮失败，而不是开一个空白窗口。
    // 包一层 Arc：宿主编排要用它，界面的 Global 也要引用它交付的东西。
    let prepared = Arc::new(prepare(host_options()?)?);
    // 装配期的警告先打到 stderr；界面里的常驻提示属于设置屏（P2）。
    for warning in &prepared.warnings {
        eprintln!("warning: {warning}");
    }
    let host = Host::new(&prepared, runtime.handle().clone());
    // 界面偏好（主题）：读不到就用默认（跟随系统），**永不拦启动**。
    let prefs = UiPrefs::load(&prepared.ui_prefs_file);

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
            Err(error) => eprintln!("{APP_NAME}: startup failed: {}", error.message),
        }
    });

    gpui_kit::application()
        // 完整图标目录，而不是组件默认的那 106 个：名字不在已注册的资产源里时，
        // GPUI 只会画出一个**空白**的 svg（没有报错、没有占位），按钮就成了点不到的幽灵
        // —— 走查里代理屏的「编辑」就是这样丢的（铅笔在完整目录里、不在默认集里）。
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            // 必须先初始化 Component/Base，再创建任何组件视图。
            gpui_kit::init(cx);
            // 后端进 Global：视图经 cx.global::<Host>() 取门面，不各存一份。
            cx.set_global(host);
            cx.set_global(prefs);
            // 主题在开窗之前落实：第一帧就是用户选的那一档，不先闪一下亮色。
            envboard_desktop::ui_prefs::apply(prefs.theme, None, cx);

            // 自绘标题栏：隐藏系统标题栏，拖拽区与窗口按钮由 TitleBar 绘制。
            let mut options = gpui_kit::WindowOptions {
                window_bounds: Some(WindowBounds::centered(
                    size(px(DEFAULT_WINDOW_WIDTH), px(DEFAULT_WINDOW_HEIGHT)),
                    cx,
                )),
                window_min_size: Some(size(px(MIN_WINDOW_WIDTH), px(MIN_WINDOW_HEIGHT))),
                app_id: Some(APP_NAME.to_owned()),
                ..gpui_kit::component::TitleBar::window_options()
            };
            // 保留窗口标题（任务栏 / 无障碍名称），但标题栏外观完全由组件绘制。
            options.titlebar = Some(gpui_kit::TitlebarOptions {
                title: Some(APP_NAME.into()),
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
/// 组合根入参：**只剩产品名与"其余全用默认"**。
///
/// 端口区间、抓包预算、日志目录都退回各自的默认值（默认值本来就只在
/// `ManagerConfig::new` 里定义一份，这里不重复）。要改它们就得有能改的地方 ——
/// 将来的设置屏，而不是命令行参数。
fn host_options() -> Result<HostOptions, Error> {
    Ok(HostOptions {
        // 组合根不知道产品叫什么 —— 名字由这里（产品二进制）从 Cargo.toml 取来传下去。
        app_dir_name: Some(APP_NAME.to_owned()),
        ..HostOptions::default()
    })
}

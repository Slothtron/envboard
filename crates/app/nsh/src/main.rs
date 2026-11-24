//! envboard 的窗口声明。照 `examples/gallery-rs` 的写法：`AppOptions` 只留平台
//! 建窗要的东西（title / size / app_id），主题、隐藏标题栏、Mica、阴影都在
//! `platform::Root` 上声明 —— 这样窗框仍是系统的（圆角、投影、三个 caption
//! 按钮、Snap Layouts、最大化变方都由 Windows 负责）。
//!
//! 主题从 `envboard_ui::THEME_JSON` 取：设计系统的 crate 是它唯一的源，
//! 这个二进制不另存一份 JSON。
//!
//! `NG_DARK=1` 固定深色外观（不依赖系统主题），用来出「夜黛」这一套的截图。
//!
//! **测试口径收在 `harness` 特征后面**：`NG_GOLDEN` / `NG_STRESS` 能让这个二进制不连
//! 宿主、铺 10k 条假环境。发布物是默认特征构建（`cargo build --release`），此时这些
//! 变量一律无效；要跑 golden / 压测请显式加 `--features harness`。

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(target_os = "windows")]
use nsh_core::desktop::{
    Backdrop, MainWindowOptions, TitleBarOptions, TitleBarStyle, WindowAttributes,
};
#[cfg(target_os = "windows")]
use nsh_core::platform::Appearance;
#[cfg(target_os = "windows")]
use nsh_core::prelude::{Points, Size, px};
#[cfg(target_os = "windows")]
use nsh_platform_windows as platform;

#[cfg(target_os = "windows")]
fn flag(name: &str) -> bool {
    envboard_nsh::harness_flag(name)
}

#[cfg(target_os = "windows")]
fn main() -> impl std::process::Termination {
    env_logger::init();

    // N（方案 §5.2）：单实例 preflight 必须在 `boot` 之前。同一 state-dir 已有
    // 实例时，第二个进程在这里握手：被接受就 `exit 0`（主实例会把窗口拉到
    // 前台），被拒或没应答就给出结论再 `exit 3` —— 而不是先撞 `InstanceLock`，
    // 把一句 `conflict` 甩给一个还没有窗口的进程（用户看到的是「双击没反应」）。
    // 主实例的管道句柄挂在 `instance` 上，必须活到 `run_app` 返回。
    let instance = envboard_nsh::single_instance::preflight();
    match &instance {
        envboard_nsh::single_instance::Preflight::Owner(guard) => {
            log::info!("单实例：本进程是主实例");
            // 等待线程立刻起（不等窗口）：这样次实例撞上「主实例正在 boot」
            // 也能马上拿到回执，真正的开窗由界面线程轮询完成。
            envboard_nsh::single_instance::spawn_waiter(guard);
        }
        envboard_nsh::single_instance::Preflight::Activated => {
            log::info!("单实例：已有实例接受了激活请求，本进程退出");
            return std::process::ExitCode::SUCCESS;
        }
        envboard_nsh::single_instance::Preflight::Rejected(why)
        | envboard_nsh::single_instance::Preflight::NoResponse(why) => {
            log::warn!("单实例：{why}");
            envboard_nsh::single_instance::prompt(
                &envboard_nsh::single_instance::activation_failed_message(why),
            );
            return std::process::ExitCode::from(3);
        }
        envboard_nsh::single_instance::Preflight::Unavailable { reason } => {
            // 机制不可用不拦着启动：互斥仍由 `InstanceLock` 兜底。
            log::info!("单实例 preflight 不可用：{reason}");
        }
    }

    let app_id = envboard_nsh::APP_ID;
    let theme = nsh_base::Theme::from_json(envboard_ui::THEME_JSON)
        .expect("the traditional-colour theme parses");

    // 数据面先起：组合根装配、崩溃自愈、日志照看都在建窗之前完成，窗口第一帧
    // 就能拿到真快照。golden 与压力口径（`NG_STRESS`）不启宿主 —— 它们只守像素与
    // 列表渲染。失败也不拦着开窗，错误会落在界面错误槽里。
    let synthetic = flag("NG_GOLDEN") || envboard_nsh::harness_var("NG_STRESS").is_some();
    if !synthetic {
        match envboard_nsh::boot_host() {
            Ok(()) => log::info!("宿主已就绪"),
            Err(error) => {
                let line = format!("{} · {}", error.code.as_str(), error.message);
                log::error!("宿主启动失败：{line}");
                // 迁移期最常见的冲突不是「自己开了两个」（那已被 preflight 拦住），
                // 而是旧版 envboard（Tauri 版）还占着同一份 state-dir —— 它用另一条
                // 管道名，preflight 看不见，这里收口成人话再退出。
                if error.code.as_str() == "conflict"
                    && error.message.contains("another envboard process holds")
                {
                    envboard_nsh::single_instance::prompt(
                        &envboard_nsh::single_instance::lock_held_message(&line),
                    );
                    return std::process::ExitCode::from(2);
                }
                envboard_nsh::record_boot_error(line);
            }
        }
    }

    let mut title_bar = TitleBarOptions::default();
    title_bar.style = TitleBarStyle::Hidden;
    title_bar.height = Some(px(40.0));

    let mut attributes = WindowAttributes::default();
    attributes.title_bar = Some(title_bar);
    attributes.backdrop = Some(Backdrop::Window);
    attributes.shadow = Some(true);
    // `attributes!` 的字段是「有没有提供」再套一层 Option，所以是双 Some。
    attributes.min_size = Some(Some(Size {
        width: Points(1120.0),
        height: Points(720.0),
    }));

    // golden 模式（`NG_GOLDEN=1`，只由 `tests/golden.rs` 拉起）：不启用 Mica、
    // 不带系统投影 —— 抓图要逐像素稳定，而 Mica 会把桌面壁纸混进来。
    // 这条只改「合成」，不改布局与令牌，所以 golden 仍然守得住版式回归。
    // 合成关掉（不吃壁纸、不带投影）：`NG_GOLDEN` 给 golden 用，`NG_PLAIN` 给
    // 人工抓真数据帧用 —— 两者都不改布局与令牌，差别只在宿主启不启。
    if flag("NG_GOLDEN") || flag("NG_PLAIN") {
        attributes.backdrop = Some(Backdrop::None);
        attributes.shadow = Some(false);
    }

    // T1（P6）托盘探针：关窗要改成隐藏，所以关窗这件事交给应用自己判
    // （`close_on_press = false` 之后平台只上报 closePress）。产品构建里
    // 这个函数恒为 false，窗口仍由系统直接关。
    #[cfg(feature = "harness")]
    if envboard_nsh::tray_probe::wants_close_press() {
        attributes.close_on_press = Some(false);
    }

    let mut window = MainWindowOptions::default();
    window.default_size = Some(Size {
        width: Points(1280.0),
        height: Points(840.0),
    });
    window.attributes = attributes;

    let mut root = platform::Root::of::<envboard_nsh::App>()
        .tokens(theme.into_tokens())
        .app_id(app_id)
        .main_window(window);
    if flag("NG_DARK") {
        root = root.appearance(Appearance::Dark);
    }

    platform::run_app(
        platform::AppSource::Rust(root),
        platform::AppOptions {
            title: envboard_nsh::WINDOW_TITLE.to_owned(),
            // 图标是一枚 .ico（16–256 九个尺寸）：任务栏、Alt+Tab、通知都用它。
            icon: envboard_nsh::icon_path(),
            app_id: app_id.to_owned(),
            size: (1280, 840),
            ..Default::default()
        },
    )
}

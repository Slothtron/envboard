//! envboard 的窗口声明。照 `examples/gallery-rs` 的写法：`AppOptions` 只留平台
//! 建窗要的东西（title / size / app_id），主题、隐藏标题栏、Mica、阴影都在
//! `platform::Root` 上声明 —— 这样窗框仍是系统的（圆角、投影、三个 caption
//! 按钮、Snap Layouts、最大化变方都由 Windows 负责）。
//!
//! 主题从 `envboard_ui::THEME_JSON` 取：设计系统的 crate 是它唯一的源，
//! 这个二进制不另存一份 JSON。
//!
//! `NG_DARK=1` 固定深色外观（不依赖系统主题），用来出「夜黛」这一套的截图。

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(target_os = "windows")]
use nsh_core::desktop::{Backdrop, MainWindowOptions, TitleBarOptions, TitleBarStyle, WindowAttributes};
#[cfg(target_os = "windows")]
use nsh_core::prelude::{px, Points, Size};
#[cfg(target_os = "windows")]
use nsh_core::platform::Appearance;
#[cfg(target_os = "windows")]
use nsh_platform_windows as platform;

#[cfg(target_os = "windows")]
fn flag(name: &str) -> bool {
    std::env::var(name).map(|v| v == "1").unwrap_or(false)
}

#[cfg(target_os = "windows")]
fn main() -> impl std::process::Termination {
    env_logger::init();

    let app_id = envboard_nsh::APP_ID;
    let theme = nsh_base::Theme::from_json(envboard_ui::THEME_JSON)
        .expect("the traditional-colour theme parses");

    // 数据面先起：组合根装配、崩溃自愈、日志照看都在建窗之前完成，窗口第一帧
    // 就能拿到真快照。golden 不启宿主（它只守像素），失败也不拦着开窗 ——
    // 错误会落在界面错误槽里，用户仍能看到界面并读到原因。
    if !flag("NG_GOLDEN") {
        match envboard_nsh::boot_host() {
            Ok(()) => log::info!("宿主已就绪"),
            Err(error) => {
                let line = format!("{} · {}", error.code.as_str(), error.message);
                log::error!("宿主启动失败：{line}");
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

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

    let app_id = "dev.myclaw.envboard";
    let theme = nsh_base::Theme::from_json(envboard_ui::THEME_JSON)
        .expect("the traditional-colour theme parses");

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
    if flag("NG_GOLDEN") {
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
            title: "envboard · 中国传统色".to_owned(),
            app_id: app_id.to_owned(),
            size: (1280, 840),
            ..Default::default()
        },
    )
}

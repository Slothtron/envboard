//! L0：无窗口、无 GPU 的界面逻辑验收。
//!
//! 走真实的工作台视图、真实的 Admin 门面、真实的引擎替身，不复制测试专用的近似视图。
//! 这类测试在**锁屏**、CI、没有显卡的机器上都能跑。
//!
//! ## 为什么不用 `gpui-kit/test-support`
//!
//! kit 的那条 feature 会牵出 `gpui-fast-platform`，它钉死 `cocoa =0.26.0`，与
//! `gpui-pre-macos` 已锁的 0.26.1 冲突，整棵依赖图解析不出来（2026-10-29 联网复验，
//! 错误原文与本仓 ADR-001 记录一致）。故直接给 `gpui-pre` 与 `gpui-pre-platform`
//! 开同名 feature，绕开 kit 的 feature 转发。
//!
//! 配套的像素级验收（L1）在 `examples/screenshot.rs`：Windows 没有无头 GPU 渲染器
//! （`current_headless_renderer()` 在 Windows 返回 `None`），只能开一个屏幕外的
//! 真窗口，用 `Window::render_to_image()` 回读 D3D11 staging texture。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use envboard_desktop::{Destination, Host, ThemePreference, UiPrefs, Workbench};
use envboard_engine_fake::FakeEngine;
use envboard_host::{HostOptions, prepare_with};
use gpui_kit::component::Theme;
use gpui_kit::{App, Modifiers, MouseButton, TestAppContext, TestDispatcher, VisualTestContext};
use gpui_kit::{point, px};

/// 装配界面需要的全局对象：一个真管理器 + 假引擎。
///
/// 引擎用 `FakeEngine`：它真的绑定端口、不代理流量，与真引擎同构，
/// 所以装配路径与生产完全一致（不复制构造序列）。
fn install_globals(cx: &mut App, state_dir: &Path, runtime: &tokio::runtime::Runtime) {
    gpui_kit::init(cx);

    let options = HostOptions {
        state_dir: Some(state_dir.to_path_buf()),
        port_range: Some((17_100, 17_199)),
        capture_budget: 0,
        log_dir: None,
        no_log_file: true,
        max_log_bytes: None,
    };
    let prepared =
        prepare_with(options, Arc::new(FakeEngine::new())).expect("应当能用假引擎装配出宿主");
    cx.set_global(Host::new(&prepared, runtime.handle().clone()));
    cx.set_global(UiPrefs::load(&prepared.state_dir));
}

/// 造一个只属于本次测试的临时状态目录（不留痕、不碰真账本）。
///
/// **落 `target/` 而不是系统 TEMP**：本机默认用户 `TEMP` 拒绝访问
/// （ADR-001「本机验证边界」记过这条），落到系统临时目录会让测试直接失败。
fn temp_state_dir(tag: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ui-test-state")
        .join(tag);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("应当能建临时状态目录");
    dir
}

#[test]
fn workbench_opens_on_environments_screen() {
    let state_dir = temp_state_dir("l0-open");
    let mut cx = TestAppContext::build(
        TestDispatcher::new(0),
        Some("workbench_opens_on_environments_screen"),
    );

    // tokio runtime 必须自己建：`Handle::current()` 要求有 reactor 在跑，
    // 而测试线程不是 tokio 线程。
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("应当能建 tokio runtime");

    cx.update(|cx| install_globals(cx, &state_dir, &runtime));

    // 真实工作台视图：五个屏在启动时一次建好，切屏状态可观察。
    let (view, vcx) = cx.add_window_view(Workbench::new);
    vcx.cx.run_until_parked();

    assert_eq!(
        vcx.update(|_, cx| view.read(cx).destination()),
        Destination::Environments,
        "默认应停在「环境」屏"
    );

    let _ = std::fs::remove_dir_all(&state_dir);
}

#[test]
fn sidebar_click_switches_screen() {
    let state_dir = temp_state_dir("l0-nav");
    let mut cx = TestAppContext::build(
        TestDispatcher::new(0),
        Some("sidebar_click_switches_screen"),
    );

    // tokio runtime 必须自己建：`Handle::current()` 要求有 reactor 在跑，
    // 而测试线程不是 tokio 线程。
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("应当能建 tokio runtime");

    cx.update(|cx| install_globals(cx, &state_dir, &runtime));

    let (view, vcx) = cx.add_window_view(Workbench::new);
    vcx.cx.run_until_parked();

    // 侧栏第二项「规则」的位置：标题栏下方第 48 像素（实测探针定位）。
    // 这里走**真实事件路径**（mouse down/up），不是直接改字段 ——
    // 切屏靠的是 SidebarMenuItem 的 on_click 回调。
    //
    // 坐标是**硬编码的**：GPUI 的测试上下文没有「按名字找元素」的能力
    // （gpui-kit 的 `click(id)` 需要元素注册 debug_selector，组件库没注册）。
    // 布局一旦变化，这个坐标就要重探 —— 改动侧栏结构时留意这里。
    let target = point(px(60.0), px(88.0));
    vcx.simulate_mouse_down(target, MouseButton::Left, Modifiers::none());
    vcx.simulate_mouse_up(target, MouseButton::Left, Modifiers::none());

    assert_eq!(
        vcx.update(|_, cx| view.read(cx).destination()),
        Destination::Rules,
        "点侧栏「规则」应切到规则屏"
    );

    let _ = std::fs::remove_dir_all(&state_dir);
}

/// 侧栏每一项的中心 y（实测探针：项高 36 px，第一项中心在 y≈52）。
///
/// 坐标是**硬编码的**：`SidebarMenuItem` 不注册 `debug_selector`，无头上下文里
/// 只能按位置点。改动侧栏（增删项、调间距）时必须重探这里 —— 上面那条
/// `sidebar_click_switches_screen` 同理。
const NAV_CA_Y: f32 = 200.0;
const NAV_SETTINGS_Y: f32 = 236.0;

/// 在侧栏某一项上真点一次（down + up 都派发，走的是组件自己的 on_click）。
fn click_sidebar(vcx: &mut VisualTestContext, y: f32) {
    let p = point(px(60.0), px(y));
    vcx.simulate_mouse_down(p, MouseButton::Left, Modifiers::none());
    vcx.simulate_mouse_up(p, MouseButton::Left, Modifiers::none());
    vcx.cx.run_until_parked();
}

#[test]
fn sidebar_click_opens_the_ca_screen() {
    let state_dir = temp_state_dir("l0-ca-nav");
    let mut cx = TestAppContext::build(
        TestDispatcher::new(0),
        Some("sidebar_click_opens_the_ca_screen"),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("应当能建 tokio runtime");
    cx.update(|cx| install_globals(cx, &state_dir, &runtime));

    let (view, vcx) = cx.add_window_view(Workbench::new);
    vcx.cx.run_until_parked();

    click_sidebar(vcx, NAV_CA_Y);
    assert_eq!(
        vcx.update(|_, cx| view.read(cx).destination()),
        Destination::Ca,
        "点侧栏「CA」应切到 CA 屏"
    );

    let _ = std::fs::remove_dir_all(&state_dir);
}

#[test]
fn theme_dropdown_applies_and_persists() {
    let state_dir = temp_state_dir("l0-theme");
    let mut cx = TestAppContext::build(
        TestDispatcher::new(0),
        Some("theme_dropdown_applies_and_persists"),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("应当能建 tokio runtime");
    cx.update(|cx| install_globals(cx, &state_dir, &runtime));

    let (_view, vcx) = cx.add_window_view(Workbench::new);
    vcx.cx.run_until_parked();

    // 进设置屏，再按 debug_selector 锚点找到主题下拉框（不硬编码它的坐标）。
    click_sidebar(vcx, NAV_SETTINGS_Y);
    let bounds = vcx
        .debug_bounds("theme-select")
        .expect("设置屏应渲染出带锚点的主题下拉框");
    let trigger = point(
        bounds.origin.x + bounds.size.width / 2.0,
        bounds.origin.y + bounds.size.height / 2.0,
    );

    // 打开下拉，点第二项「深色」（行高 32 px，第一项在底边 +16 处）。
    vcx.simulate_mouse_down(trigger, MouseButton::Left, Modifiers::none());
    vcx.simulate_mouse_up(trigger, MouseButton::Left, Modifiers::none());
    vcx.cx.run_until_parked();
    let second_item = point(trigger.x, bounds.origin.y + bounds.size.height + px(48.0));
    vcx.simulate_mouse_down(second_item, MouseButton::Left, Modifiers::none());
    vcx.simulate_mouse_up(second_item, MouseButton::Left, Modifiers::none());
    vcx.cx.run_until_parked();

    // 三处都要变：偏好全局、组件主题（真的换了皮肤）、落盘文件（下次启动沿用）。
    assert_eq!(
        vcx.update(|_, cx| cx.global::<UiPrefs>().theme),
        ThemePreference::Dark,
        "下拉选「深色」应改掉界面偏好"
    );
    assert!(
        vcx.update(|_, cx| Theme::global(cx).is_dark()),
        "组件主题应随之切到暗色皮肤"
    );
    let saved = std::fs::read_to_string(state_dir.join("ui.json")).expect("ui.json 应已写出");
    assert!(
        saved.contains("\"theme\": \"dark\""),
        "偏好要落盘，否则下次启动又回去：{saved}"
    );

    let _ = std::fs::remove_dir_all(&state_dir);
}

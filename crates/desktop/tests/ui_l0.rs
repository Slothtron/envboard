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

use envboard_desktop::{Destination, Filter, Host, ThemePreference, UiPrefs, Workbench};
use envboard_engine_fake::FakeEngine;
use envboard_host::{HostOptions, prepare_with};
use gpui_kit::component::Theme;
use gpui_kit::{
    App, AppContext as _, Modifiers, MouseButton, TestAppContext, TestDispatcher, VisualTestContext,
};
use gpui_kit::{point, px};

/// 装配界面需要的全局对象：一个真管理器 + 假引擎。
///
/// 引擎用 `FakeEngine`：它真的绑定端口、不代理流量，与真引擎同构，
/// 所以装配路径与生产完全一致（不复制构造序列）。
fn install_globals(cx: &mut App, state_dir: &Path, runtime: &tokio::runtime::Runtime) {
    gpui_kit::init(cx);

    let options = HostOptions {
        state_dir: Some(state_dir.to_path_buf()),
        // 状态目录是显式给的，平台默认那一级用不上，也就不需要应用名。
        app_dir_name: None,
        port_range: Some((17_100, 17_199)),
        capture_budget: 0,
        log_dir: None,
        no_log_file: true,
        max_log_bytes: None,
    };
    let prepared =
        prepare_with(options, Arc::new(FakeEngine::new())).expect("应当能用假引擎装配出宿主");
    cx.set_global(Host::new(&prepared, runtime.handle().clone()));
    cx.set_global(UiPrefs::load(&prepared.ui_prefs_file));
}

/// 造一个只属于本次测试的临时状态目录（不留痕、不碰真账本）。
///
/// **落 `target/` 而不是系统 TEMP**：本机默认用户 `TEMP` 拒绝访问
/// （ADR-001「本机验证边界」记过这条），落到系统临时目录会让测试直接失败。
///
/// **每次跑都用新目录**（名字带上进程号）：目录里躺着上一轮写下的
/// `ui.json`，它带着上一轮的界面偏好。留着的话，下一轮从"偏好已经是深色"
/// 起步 —— 那样测的就不再是"选深色 → 换皮肤"，而是"本来就是深色 → 皮肤却
/// 是浅的"（这条坑真实踩过：一次失败之后，主题那条用例开始稳定地假红）。
/// 逐个删旧目录并不稳妥：Windows 上文件被进程攥着时，`remove_file` 会成功
/// 但文件仍留在那儿（删除挂起），于是"清干净"这件事做不到。
fn temp_state_dir(tag: &str) -> PathBuf {
    // 唯一后缀用纳秒时间戳：进程号会被复用，那会让两次跑撞到同一个目录。
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ui-test-state")
        .join(format!("{tag}-{stamp}"));
    std::fs::create_dir_all(&dir).expect("应当能建临时状态目录");
    dir
}

/// 临时状态目录的守卫：**测试失败（panic）时也要清掉界面偏好**。
///
/// 之前清理写在测试体末尾，一旦断言失败就被跳过，脏的 `ui.json` 留给下一轮。
/// 守卫靠 `Drop` 收尾，panic 展开时照样跑。
///
/// 只删 `ui.json`：账本与锁文件由宿主攥着句柄，整目录删不掉，而它们对
/// "下一轮从什么偏好起步"没有任何影响。
struct StateDir(PathBuf);

impl StateDir {
    fn new(tag: &str) -> Self {
        Self(temp_state_dir(tag))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for StateDir {
    fn drop(&mut self) {
        // 路径取自布局表（不自己拼 `"ui.json"`）：布局字面量只有一处实现。
        let _ = std::fs::remove_file(self.0.join(envboard_host::layout::UI_PREFS_FILE));
    }
}

#[test]
fn workbench_opens_on_environments_screen() {
    let state = StateDir::new("l0-open");
    let state_dir = state.path();
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

    cx.update(|cx| install_globals(cx, state_dir, &runtime));

    // 真实工作台视图：五个屏在启动时一次建好，切屏状态可观察。
    let (view, vcx) = cx.add_window_view(Workbench::new);
    vcx.cx.run_until_parked();

    assert_eq!(
        vcx.update(|_, cx| view.read(cx).destination()),
        Destination::Environments,
        "默认应停在「环境」屏"
    );
}

#[test]
fn sidebar_click_switches_screen() {
    let state = StateDir::new("l0-nav");
    let state_dir = state.path();
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

    cx.update(|cx| install_globals(cx, state_dir, &runtime));

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
fn the_status_filter_switches_to_the_clicked_tier() {
    let state = StateDir::new("l0-filter");
    let state_dir = state.path();
    let mut cx = TestAppContext::build(
        TestDispatcher::new(0),
        Some("the_status_filter_switches_to_the_clicked_tier"),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("应当能建 tokio runtime");
    cx.update(|cx| install_globals(cx, state_dir, &runtime));

    let (view, vcx) = cx.add_window_view(Workbench::new);
    vcx.cx.run_until_parked();

    // 起点是「全部」。
    assert_eq!(
        vcx.update(|_, cx| view.read(cx).environments().read(cx).filter()),
        Filter::All,
        "默认应停在「全部」这一档"
    );

    // 分段控件是组件库画的（不注册 selector），只能靠外层锚点拿到矩形，
    // 再按**档宽**点到某一档。四档等宽，点第三档（已停止）的中心。
    let bounds = vcx
        .debug_bounds("env-filters")
        .expect("环境屏应渲染出带锚点的过滤分段控件");
    let tier_width = bounds.size.width / 4.0;
    let target = point(
        bounds.origin.x + tier_width * 2.5,
        bounds.origin.y + bounds.size.height / 2.0,
    );
    vcx.simulate_mouse_down(target, MouseButton::Left, Modifiers::none());
    vcx.simulate_mouse_up(target, MouseButton::Left, Modifiers::none());
    vcx.cx.run_until_parked();

    assert_eq!(
        vcx.update(|_, cx| view.read(cx).environments().read(cx).filter()),
        Filter::Stopped,
        "点分段控件第三档应切到「已停止」"
    );
}

#[test]
fn sidebar_click_opens_the_ca_screen() {
    let state = StateDir::new("l0-ca-nav");
    let state_dir = state.path();
    let mut cx = TestAppContext::build(
        TestDispatcher::new(0),
        Some("sidebar_click_opens_the_ca_screen"),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("应当能建 tokio runtime");
    cx.update(|cx| install_globals(cx, state_dir, &runtime));

    let (view, vcx) = cx.add_window_view(Workbench::new);
    vcx.cx.run_until_parked();

    click_sidebar(vcx, NAV_CA_Y);
    assert_eq!(
        vcx.update(|_, cx| view.read(cx).destination()),
        Destination::Ca,
        "点侧栏「CA」应切到 CA 屏"
    );
}

#[test]
fn theme_dropdown_applies_and_persists() {
    let state = StateDir::new("l0-theme");
    let state_dir = state.path();
    let mut cx = TestAppContext::build(
        TestDispatcher::new(0),
        Some("theme_dropdown_applies_and_persists"),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("应当能建 tokio runtime");
    cx.update(|cx| install_globals(cx, state_dir, &runtime));

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
    let saved = std::fs::read_to_string(state_dir.join(envboard_host::layout::UI_PREFS_FILE))
        .expect("界面偏好应已写出");
    assert!(
        saved.contains("\"theme\": \"dark\""),
        "偏好要落盘，否则下次启动又回去：{saved}"
    );
}

/// 数据通路：**快照真的进了表格**。
///
/// 此前 5 条 L0 全部只断言交互与状态迁移，没有一条覆盖"轮询把数据送到表里"。
/// 于是 L1 离屏夹具把 tokio 运行时提前关掉（表格永远停在骨架加载态）这件事，
/// 在 `cargo test` 里一直是绿的 —— 而拍出来的图看起来像"空账本"，没人会怀疑。
#[test]
fn the_snapshot_poller_fills_the_table() {
    use std::time::Duration;

    let state = StateDir::new("l0-poll");
    let state_dir = state.path();
    let mut cx = TestAppContext::build(
        TestDispatcher::new(0),
        Some("the_snapshot_poller_fills_the_table"),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("应当能建 tokio runtime");
    cx.update(|cx| install_globals(cx, state_dir, &runtime));

    let admin = cx.update(|cx| cx.global::<Host>().admin().clone());
    for name in ["poll-a", "poll-b", "poll-c"] {
        admin
            .create(&serde_json::json!({ "name": name }))
            .expect("应当能建环境");
    }

    let (view, vcx) = cx.add_window_view(Workbench::new);
    vcx.cx.run_until_parked();

    // `TestAppContext` 不驱动窗口换帧，`Workbench::new` 里"首帧后挂轮询器"那一步
    // 在无头上下文里不会自己发生（真实窗口那条路径由 L1 覆盖），这里显式补一次。
    let environments = vcx.update(|_, cx| view.read(cx).environments().clone());
    vcx.update(|window, cx| {
        environments.update(cx, |view, cx| view.attach(window, cx));
    });

    // 轮询首个 tick 立即触发；跨线程唤醒要主线程把执行器再跑几轮才落地。
    let mut rows = 0;
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(50));
        vcx.cx.run_until_parked();
        rows = vcx.update(|_, cx| {
            view.read(cx)
                .environments()
                .read(cx)
                .table()
                .read(cx)
                .delegate()
                .total()
        });
        if rows > 0 {
            break;
        }
    }
    assert_eq!(rows, 3, "快照轮询应当把三个环境送进表格");
}

/// 失败回执的三层信息：**契约错误码 + 中文主句 + 服务端英文原文**。
///
/// 走查 F10 的问题是"错误条里只有英文"。修法的判据不能停在"显示了中文"，
/// 得钉住三件事同时在场：码没被抹掉（排障要拿它查日志）、主句是中文且点名了
/// 动作与对象、原文被保留在第二行（"具体是哪个"只有它说得清）。
///
/// 用"删一个不存在的环境"造失败：服务端确定地回 `not_found`，不依赖假引擎
/// 的任何注入能力。
#[test]
fn a_failed_write_carries_code_hint_and_original_message() {
    use std::time::Duration;

    let state = StateDir::new("l0-error-notice");
    let state_dir = state.path();
    let mut cx = TestAppContext::build(
        TestDispatcher::new(0),
        Some("a_failed_write_carries_code_hint_and_original_message"),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("应当能建 tokio runtime");
    cx.update(|cx| install_globals(cx, state_dir, &runtime));

    let (view, vcx) = cx.add_window_view(Workbench::new);
    vcx.cx.run_until_parked();

    let environments = vcx.update(|_, cx| view.read(cx).environments().clone());
    vcx.update(|window, cx| {
        environments.update(cx, |view, cx| view.remove("nope".into(), window, cx));
    });

    // 回执要走一趟 tokio 再回到主线程：轮询到它出现为止。
    let mut notice = None;
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(50));
        vcx.cx.run_until_parked();
        notice = vcx.update(|_, cx| environments.read(cx).table().read(cx).delegate().error());
        if notice.is_some() {
            break;
        }
    }

    let notice = notice.expect("删除不存在的环境应当把错误写进表");
    assert_eq!(
        notice.code,
        Some("not_found"),
        "契约错误码要原样留着（排障查日志靠它）"
    );
    assert!(
        notice.summary.contains("删除 nope 失败"),
        "中文主句要说清哪个动作、哪个对象：{}",
        notice.summary
    );
    assert!(
        notice.summary.contains("目标不存在"),
        "主句要带该错误码的中文结论：{}",
        notice.summary
    );
    let detail = notice.detail_text().expect("英文原文必须保留在第二行");
    assert!(
        detail.starts_with("[not_found] "),
        "第二行要以错误码开头：{detail}"
    );
    assert!(
        !detail.trim_start_matches("[not_found] ").is_empty(),
        "原文不能是空的：{detail}"
    );
}

// --------------------------------------------------------------------------- #
// 键盘可达：状态栏与表格
// --------------------------------------------------------------------------- #

/// 起一张带 `names` 个环境的窗口，并等快照真的进表。
///
/// 与 `the_snapshot_poller_fills_the_table` 同一条装配路径：`TestAppContext`
/// 不驱动换帧，"首帧后挂轮询器"那一步得显式补一次。
fn open_workbench_with_envs<'a>(
    cx: &'a mut TestAppContext,
    names: &[&str],
) -> (gpui_kit::Entity<Workbench>, &'a mut VisualTestContext) {
    open_workbench(cx, names, false)
}

/// `rooted = true` 时按 `gpui_kit::open_window` 的方式把 Workbench 包进
/// gpui-component 的 `Root` —— **Sheet / Dialog 依赖这层 Root**，裸
/// `add_window_view(Workbench::new)` 下打开 Sheet 会直接 panic（ROOT_MISSING）。
fn open_workbench<'a>(
    cx: &'a mut TestAppContext,
    names: &[&str],
    rooted: bool,
) -> (gpui_kit::Entity<Workbench>, &'a mut VisualTestContext) {
    use std::time::Duration;

    let admin = cx.update(|cx| cx.global::<Host>().admin().clone());
    for name in names {
        admin
            .create(&serde_json::json!({ "name": name }))
            .expect("应当能建环境");
    }

    let (view, vcx) = if rooted {
        let mut built = None;
        let (_root, vcx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| Workbench::new(window, cx));
            built = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        (built.expect("Root 装配应当建出 Workbench"), vcx)
    } else {
        cx.add_window_view(Workbench::new)
    };
    vcx.cx.run_until_parked();
    let environments = vcx.update(|_, cx| view.read(cx).environments().clone());
    vcx.update(|window, cx| {
        environments.update(cx, |view, cx| view.attach(window, cx));
    });

    let mut rows = 0;
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(50));
        vcx.cx.run_until_parked();
        rows = vcx.update(|_, cx| {
            view.read(cx)
                .environments()
                .read(cx)
                .table()
                .read(cx)
                .delegate()
                .total()
        });
        if rows == names.len() {
            break;
        }
    }
    assert_eq!(rows, names.len(), "快照轮询应当把环境送进表格");
    (view, vcx)
}

/// 状态栏是**一个** Tab 落点：←/→ 在六项聚合之间走查，Enter 触发导航。
#[test]
fn the_status_bar_is_keyboard_reachable() {
    let state = StateDir::new("l0-status-keys");
    let mut cx = TestAppContext::build(
        TestDispatcher::new(0),
        Some("the_status_bar_is_keyboard_reachable"),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("应当能建 tokio runtime");
    cx.update(|cx| install_globals(cx, state.path(), &runtime));

    let (view, vcx) = open_workbench_with_envs(&mut cx, &["kbd-s1"]);

    // 真实入口是 F6（依赖"首帧给根视图焦点"）；无头上下文不跑首帧，
    // 所以直接落焦点 —— 后续的 ←/→/Enter/Esc 仍走真实键位路径。
    vcx.update(|window, cx| {
        view.update(cx, |view, cx| view.focus_status(window, cx));
    });
    vcx.cx.run_until_parked();
    assert_eq!(vcx.update(|_, cx| view.read(cx).status_active()), 0);

    vcx.simulate_keystrokes("right right");
    assert_eq!(
        vcx.update(|_, cx| view.read(cx).status_active()),
        2,
        "→ 应逐项移动"
    );

    // 越界夹在最后一项，不绕回、不丢焦点。
    vcx.simulate_keystrokes("right right right right right right");
    assert_eq!(
        vcx.update(|_, cx| view.read(cx).status_active()),
        5,
        "→ 到末项应停住"
    );

    // 回到「抓包」（下标 2），Enter 应走与点击相同的导航。
    vcx.simulate_keystrokes("left left left");
    vcx.simulate_keystrokes("enter");
    assert_eq!(
        vcx.update(|_, cx| view.read(cx).destination()),
        Destination::Debug,
        "Enter 应触发当前聚合项的导航"
    );

    // Esc 退出状态栏后，←/→ 不再被状态栏消费。
    vcx.simulate_keystrokes("escape right");
    assert_eq!(
        vcx.update(|_, cx| view.read(cx).status_active()),
        2,
        "Esc 之后状态栏不该再收方向键"
    );
}

/// 表格：↑/↓ 只移动高亮，Enter 才打开详情。
///
/// 这是 `SelectRow` 的语义拆分：鼠标单击仍然"点即开"，键盘浏览不弹面板。
#[test]
fn keyboard_selection_does_not_open_the_detail_sheet() {
    let state = StateDir::new("l0-kbd-table");
    let mut cx = TestAppContext::build(
        TestDispatcher::new(0),
        Some("keyboard_selection_does_not_open_the_detail_sheet"),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("应当能建 tokio runtime");
    cx.update(|cx| install_globals(cx, state.path(), &runtime));

    // Sheet 需要 component Root：走带 Root 的装配路径。
    let (view, vcx) = open_workbench(&mut cx, &["kbd-a", "kbd-b", "kbd-c"], true);
    let environments = vcx.update(|_, cx| view.read(cx).environments().clone());
    vcx.update(|window, cx| {
        environments.update(cx, |view, cx| view.focus_table(window, cx));
    });
    vcx.cx.run_until_parked();

    assert!(
        vcx.debug_bounds("env-detail-panel").is_none(),
        "初始不应有详情面板"
    );

    vcx.simulate_keystrokes("down down");
    assert!(
        vcx.debug_bounds("env-detail-panel").is_none(),
        "↑/↓ 只移动高亮，不该弹详情面板"
    );
    assert_eq!(
        vcx.update(|_, cx| environments.read(cx).selected())
            .map(|name| name.to_string()),
        Some("kbd-b".to_string()),
        "两次 ↓ 应停在第二个环境"
    );

    vcx.simulate_keystrokes("enter");
    assert!(
        vcx.debug_bounds("env-detail-panel").is_some(),
        "Enter 应打开当前行的详情"
    );
}

/// 表格行高回归：相邻行的行距必须一致。
///
/// 单测覆盖不到"往行内加了一个更高的元素"这类改动 —— 它不会让任何逻辑断言
/// 变红，只会让滚动换算与可见行数悄悄漂掉。这里量相邻两行的原点差（即真实
/// 行高），任何一行变高都会让两个行距不再相等。
#[test]
fn environment_rows_keep_a_uniform_height() {
    let state = StateDir::new("l0-row-height");
    let mut cx = TestAppContext::build(
        TestDispatcher::new(0),
        Some("environment_rows_keep_a_uniform_height"),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("应当能建 tokio runtime");
    cx.update(|cx| install_globals(cx, state.path(), &runtime));

    let (_view, vcx) = open_workbench_with_envs(&mut cx, &["row-a", "row-b", "row-c"]);

    // `debug_bounds` 只收 `&'static str`，而行的锚点名字是渲染时 format! 出来的。
    // 测试里泄漏三个短字符串换动态查找，比把生产代码改成"只给前三行贴静态锚点"干净。
    let rows: Vec<_> = (0..3)
        .map(|index| {
            let selector: &'static str = Box::leak(format!("env-row-{index}").into_boxed_str());
            vcx.debug_bounds(selector)
                .unwrap_or_else(|| panic!("第 {index} 行应带 debug 锚点"))
        })
        .collect();
    for row in &rows {
        assert!(row.size.height > px(0.0), "行不能是零高：{row:?}");
    }

    let top_pitch = rows[1].origin.y - rows[0].origin.y;
    let bottom_pitch = rows[2].origin.y - rows[1].origin.y;
    let ratio = top_pitch / bottom_pitch;
    assert!(
        (ratio - 1.0).abs() < 0.01,
        "相邻行距应一致（行高漂移会让滚动换算出错）：上 {top_pitch:?} vs 下 {bottom_pitch:?}"
    );

    let height_ratio = rows[0].size.height / rows[1].size.height;
    assert!(
        (height_ratio - 1.0).abs() < 0.02,
        "同列两行的内容高度应一致：{height_ratio}"
    );
}

/// F3 回归：删除当前选中环境后，指向它的详情面板必须一起消失。
///
/// 旧行为只清 `selected`，Sheet 仍挂着那个已不存在的环境名，`Detail::render`
/// 退化为一句「环境 X 已不存在」的空壳，而且每帧仍被 `touch_detail` 唤醒。
/// 判据用面板自己的 debug 锚点（`env-detail-panel`），与键盘那条用例同源。
#[test]
fn deleting_the_selected_environment_closes_its_detail_sheet() {
    let state = StateDir::new("l0-delete-sheet");
    let mut cx = TestAppContext::build(
        TestDispatcher::new(0),
        Some("deleting_the_selected_environment_closes_its_detail_sheet"),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("应当能建 tokio runtime");
    cx.update(|cx| install_globals(cx, state.path(), &runtime));

    // Sheet 依赖 component Root，走带 Root 的装配路径。
    let (view, vcx) = open_workbench(&mut cx, &["del-a", "del-b"], true);
    let environments = vcx.update(|_, cx| view.read(cx).environments().clone());

    vcx.update(|window, cx| {
        environments.update(cx, |view, cx| view.open_detail("del-a".into(), window, cx));
    });
    vcx.cx.run_until_parked();
    assert!(
        vcx.debug_bounds("env-detail-panel").is_some(),
        "先打开 del-a 的详情面板"
    );

    // 直接调 `remove` 走真实回执路径（确认弹窗的交互另由手工走查覆盖）。
    vcx.update(|window, cx| {
        environments.update(cx, |view, cx| view.remove("del-a".into(), window, cx));
    });
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        vcx.cx.run_until_parked();
        if vcx.debug_bounds("env-detail-panel").is_none() {
            break;
        }
    }
    assert!(
        vcx.debug_bounds("env-detail-panel").is_none(),
        "环境被删后不该留下一个指向它的空壳面板"
    );
    assert_eq!(
        vcx.update(|_, cx| environments.read(cx).selected()),
        None,
        "选中态也应一并收起"
    );
}

/// F2 回归：面板实体复用时，换环境不能把上一个环境的规则正文带过来。
///
/// 这里不依赖读盘时序（规则正文是异步的），只钉住 `rename` 的静态契约：
/// 换名字后「规则」状态必须作废、旧任务句柄必须清掉。`Detail` 是独立实体，
/// 通过 `Workbench` 暴露环境屏的 `open_detail` 连续换两次即可观察。
#[test]
fn switching_environments_resets_the_rules_panel() {
    let state = StateDir::new("l0-rules-rename");
    let mut cx = TestAppContext::build(
        TestDispatcher::new(0),
        Some("switching_environments_resets_the_rules_panel"),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("应当能建 tokio runtime");
    cx.update(|cx| install_globals(cx, state.path(), &runtime));

    let (view, vcx) = open_workbench(&mut cx, &["rule-a", "rule-b"], true);
    let environments = vcx.update(|_, cx| view.read(cx).environments().clone());

    // 连续两次 open_detail 复用同一个 Detail 实体：第二次就是走查里 A→B 的路径。
    vcx.update(|window, cx| {
        environments.update(cx, |view, cx| {
            view.open_detail("rule-a".into(), window, cx);
            view.open_detail("rule-b".into(), window, cx);
        });
    });
    vcx.cx.run_until_parked();

    assert!(
        vcx.debug_bounds("env-detail-panel").is_some(),
        "详情面板应当还开着（换环境不关面板）"
    );
    // 面板标题与内容都来自 Detail::name；暴露一个只读访问器给断言用。
    let shown = vcx.update(|_, cx| environments.read(cx).detail_name(cx));
    assert_eq!(
        shown.as_deref(),
        Some("rule-b"),
        "面板应指向新环境，而不是还留着 rule-a 的正文"
    );
}

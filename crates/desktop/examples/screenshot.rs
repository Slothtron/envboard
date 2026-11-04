//! L1：离屏渲染一帧，把像素读回来落成 PNG。
//!
//! 为什么是独立可执行文件而不是测试：
//! - Windows 的 `current_headless_renderer()` 返回 `None`（没有离屏 GPU 渲染器），
//!   所以 `HeadlessAppContext::capture_screenshot` 在本平台不可用；
//! - `VisualTestAppContext` 在 gpui-pre 0.3.9 里只在 macOS 导出；
//! - 构造 `App` 的 `App::new_app` 是 crate 私有，测试进程里拿不到真实平台。
//!
//! 所以走应用自己的启动路径：开一个**放在屏幕外**的真窗口，等它画完第一帧，
//! 调 `Window::render_to_image()`（test-support 下公开）读回像素。
//! 它在 Windows 上是 D3D11 staging texture 回读，**不经过屏幕捕获**，
//! 所以锁屏时画到自己的 swapchain 上仍然成立。
//!
//! 用法：
//! ```text
//! cargo run -p envboard-desktop --example screenshot -- [输出] [屏名] [主题] [数据] [--size WxH] [--wait N] [--click X,Y]…
//! ```
//!
//! - `屏名`：`environments` / `rules` / `proxies` / `debug` / `ca` / `settings`；
//!   切屏走**真实路径**（派发鼠标 down/up 命中组件自己的 on_click），不是直接改字段 ——
//!   摆拍出来的图不能证明界面真的能切过去。
//! - `主题`：`light` / `dark` / `system`（默认 `light`）。走的是 `main` 那条路
//!   （开窗之前 `ui_prefs::apply`），所以第一帧就是目标主题；**明暗两张都要拍**，
//!   暗色下才暴露得出"分组边界与窗口底色同色"这类只在一种主题里成立的问题。
//! - `数据`：`sample`（默认，铺一份演示数据）/ `empty`（空账本）。空态与有数据态的
//!   版式是两回事，只拍一种等于没走查另一半。
//! - `--size WxH`：开窗之后第一帧强制把窗口改成这个尺寸（逻辑像素）。
//!   `open_window` 请求的尺寸在本机不总是生效，尺寸直接决定"有没有被裁出视口"，
//!   所以走查时要钉住：默认夹具用 `1180x760`（= 产品默认窗口），
//!   极限用例用 `880x560`（= 产品最小窗口）。
//! - `--wait N`：首屏内容的就绪窗口，单位毫秒（默认 2500，够 1s 轮询把快照送进来）。
//! - `--click X,Y`：可重复，按顺序**每处一次点击**（逻辑像素，两次点击之间留 300ms）。
//!   用于打开详情面板、弹窗、下拉菜单这类要靠真实点击才出现的面。坐标来自上一张
//!   截图的量尺（物理像素 ÷ 缩放系数），改布局必须重量。

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use envboard_admin::AdminService;
use envboard_desktop::{Destination, Host, ThemePreference, UiPrefs, Workbench};
use envboard_engine_fake::FakeEngine;
use envboard_host::{HostOptions, prepare_with};
use gpui_kit::{App, AppContext as _, InputEvent as _, Window, px};
use tokio::runtime::Handle;

/// 装配界面需要的全局对象：一个真管理器 + 假引擎。
///
/// 引擎用 `FakeEngine`：它真的绑定端口、不代理流量，与真引擎同构，
/// 所以装配路径与生产完全一致（不复制构造序列）。
fn install_globals(cx: &mut App, state_dir: &Path, handle: &Handle) {
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
    cx.set_global(Host::new(&prepared, handle.clone()));
    cx.set_global(UiPrefs::load(&prepared.ui_prefs_file));
}

/// 演示数据：**只走公开的门面**（与界面点按钮走的是同一条路），不碰账本文件。
///
/// 名字、域名、凭证全部是假的（`example.com` 保留域 + `s3cret`）。抓包与调试会话
/// 不在这里开：`DebugScreen` 只在自己派发的那次点击后才会 adopt 会话，服务端单独开
/// 一个会话界面并不认领 —— 要拍"会话进行中"得用 `--click` 真点菜单。
fn seed(admin: &AdminService, handle: &Handle) {
    let note = |label: &str, outcome: std::result::Result<(), String>| match outcome {
        Ok(()) => {}
        Err(message) => eprintln!("seed {label} 失败：{message}"),
    };

    let corp_rules = "10.0.0.14 api.example.com\n10.0.0.15 static.example.com\n\
                      10.0.0.16 db.internal.example.com\n192.168.1.7 cdn.example.com\n\
                      # 演示用的注释行\n10.0.0.20 metrics.example.com\n";
    note(
        "import_rules corp",
        admin
            .import_rules("corp", corp_rules)
            .map(|_| ())
            .map_err(|error| error.message),
    );
    note(
        "import_rules legacy",
        admin
            .import_rules("legacy", "10.9.9.9 old.example.com\n")
            .map(|_| ())
            .map_err(|error| error.message),
    );
    note(
        "proxy_put corp-proxy",
        admin
            .proxy_put(&serde_json::json!({
                "name": "corp-proxy",
                "host": "127.0.0.1",
                "port": 7890,
                "user": "dev",
                "password": "s3cret",
            }))
            .map(|_| ())
            .map_err(|error| error.message),
    );
    note(
        "proxy_put lab-direct",
        admin
            .proxy_put(&serde_json::json!({
                "name": "lab-direct",
                "host": "10.1.2.3",
                "port": 8080,
            }))
            .map(|_| ())
            .map_err(|error| error.message),
    );

    // 三种健康态各留一个：运行中（绿）、失败（红 + 需处理）、期望运行但停着。
    for body in [
        serde_json::json!({
            "name": "dev",
            "description": "本地开发联调：主站与支付网关都走这里",
            "rules": "corp",
            "upstream": "corp-proxy",
        }),
        serde_json::json!({
            "name": "beta",
            "description": "预发灰度：这一条说明刻意写得很长，用来验证表格单元格的截断\
                 与列宽在长文案下的表现（长文案是常态，不是异常）",
            "rules": "corp",
        }),
        serde_json::json!({
            "name": "staging",
            "description": "上游回归验证",
            "rules": "legacy",
            "upstream": "lab-direct",
        }),
    ] {
        note(
            "create",
            admin
                .create(&body)
                .map(|_| ())
                .map_err(|error| error.message),
        );
    }
    // 「规则集缺失」那一档在这里造不出来：账本不允许删掉仍被绑定的规则集
    // （实测 `rules_delete legacy` 被服务端拒：`is bound to environment "staging"`），
    // 所以 `rules_missing` 只能靠状态目录被外部改动才出现 —— 属于 L1 的验证边界。

    for name in ["dev", "beta"] {
        note(
            "start",
            handle
                .block_on(admin.start(name))
                .map(|_| ())
                .map_err(|error| error.message),
        );
    }
    // beta 制造一条真实故障：期望运行、实际失败 —— 「需处理」这一档必须有样本才拍得到。
    if !admin.inject_fault("beta", "上游 TLS 握手失败") {
        eprintln!("seed inject_fault 未命中（beta 可能没在跑）");
    }
}

/// 要拍哪一屏。
///
/// 切屏走**真实路径**：点侧栏切屏（组件自己的 on_click），不是直接改字段。
/// 侧栏项不注册 `debug_selector`，只能按位置点 —— 坐标是实测的，改侧栏结构时要重探
/// （与 `crates/desktop/tests/ui_l0.rs` 里的常量同一处纪律）。
fn screen_by_name(name: &str) -> Destination {
    match name {
        "rules" => Destination::Rules,
        "proxies" => Destination::Proxies,
        "debug" => Destination::Debug,
        "ca" => Destination::Ca,
        "settings" => Destination::Settings,
        _ => Destination::Environments,
    }
}

/// 侧栏各项中心的 y（实测探针：项高 36 px，第一项中心在 y≈52）。
const NAV_Y: [f32; 6] = [52., 88., 124., 160., 200., 236.];

/// 主题档位。
fn theme_by_name(name: &str) -> ThemePreference {
    match name {
        "dark" => ThemePreference::Dark,
        "system" => ThemePreference::System,
        _ => ThemePreference::Light,
    }
}

/// 一帧一帧推进的拍摄脚本。
struct Script {
    /// 切屏的点击（第 2 步派发，走组件自己的 on_click）。
    nav: (f32, f32),
    /// 第 1 步强制改窗口尺寸（`--size`）：实测 `open_window` 请求的尺寸**不总是**
    /// 生效（同一份夹具，一次 1920×1369、一次 1264×581），所以尺寸要显式钉住，
    /// 并在抓图时把真实 bounds 打出来。
    resize: Option<(f32, f32)>,
    nav_pending: bool,
    clicks: VecDeque<(f32, f32)>,
    /// 出数判据：读**界面里**的环境行数（不是后端的），所以等到的是"数据真的走到了
    /// 表格"，而不是"后端有数据"。两者不等价 —— 轮询链路断掉时后端照样满。
    probe: Box<dyn FnMut(&mut App) -> usize>,
    /// 期望的行数下限（空账本模式为 0，立刻就绪）。
    expect_rows: usize,
    /// 就绪判定的结果：到点了还没出数就是**失败**，不是"照样出图"。
    ready: Rc<Cell<bool>>,
    /// 等待的硬超时：出不了数也要留下证据（那张图本身就是结论）。
    settle: Duration,
    /// 上一次动作之后重新计时：每次点击都要留一帧给界面重画。
    since: Instant,
    settled: bool,
    out: PathBuf,
}

/// 两次动作之间的间隔（够一帧重画 + 一点余量）。
const CLICK_GAP: Duration = Duration::from_millis(300);

enum Step {
    /// 什么都不做，再等一帧。
    Idle,
    /// 改窗口尺寸。
    Resize((f32, f32)),
    /// 派发一次点击。
    Click((f32, f32)),
    /// 收尾：抓这一帧并退出。
    Capture,
}

impl Script {
    fn step(&mut self, cx: &mut App) -> Step {
        if let Some(size) = self.resize.take() {
            return Step::Resize(size);
        }
        if self.nav_pending {
            self.nav_pending = false;
            self.since = Instant::now();
            return Step::Click(self.nav);
        }
        // **按墙钟 + 按出数等，不按帧数等**：离屏窗口的换帧可以远超 60fps，
        // 用帧数计等待会在 1s 轮询把快照送进来之前就抓拍 —— 实测抓到过一整屏骨架。
        if !self.settled {
            let rows = (self.probe)(cx);
            if rows >= self.expect_rows || self.since.elapsed() >= self.settle {
                self.settled = true;
                self.ready.set(rows >= self.expect_rows);
                eprintln!(
                    "shot: 界面环境行数 {rows}（期望 ≥{}），等了 {:?}",
                    self.expect_rows,
                    self.since.elapsed()
                );
            } else {
                return Step::Idle;
            }
        }
        if let Some(click) = self.clicks.front().copied() {
            if self.since.elapsed() < CLICK_GAP {
                return Step::Idle;
            }
            self.clicks.pop_front();
            self.since = Instant::now();
            return Step::Click(click);
        }
        // 最后一个动作之后同样要留一帧，否则抓到的是"点下去但还没重画"的那一帧。
        if self.since.elapsed() < CLICK_GAP {
            return Step::Idle;
        }
        Step::Capture
    }
}

/// 派发一次真实按下+抬起（命中测试用的是已画出的命中框）。
fn dispatch_click(window: &mut Window, cx: &mut App, at: (f32, f32)) {
    let target = gpui_kit::point(px(at.0), px(at.1));
    let down = gpui_kit::MouseDownEvent {
        button: gpui_kit::MouseButton::Left,
        position: target,
        modifiers: gpui_kit::Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    };
    let up = gpui_kit::MouseUpEvent {
        button: gpui_kit::MouseButton::Left,
        position: target,
        modifiers: gpui_kit::Modifiers::none(),
        click_count: 1,
    };
    window.dispatch_event(down.to_platform_input(), cx);
    window.dispatch_event(up.to_platform_input(), cx);
}

/// 把 `window` 的当前帧落盘。
fn capture(window: &mut Window, out: &Path) {
    match window.render_to_image() {
        Ok(image) => {
            if let Some(parent) = out.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match image.save(out) {
                Ok(()) => {
                    let bounds = window.bounds();
                    eprintln!(
                        "screenshot: {} (图像 {}x{}，窗口 bounds {:?}，缩放系数 {})",
                        out.display(),
                        image.width(),
                        image.height(),
                        bounds.size,
                        window.scale_factor()
                    );
                }
                Err(error) => eprintln!("写 PNG 失败：{error}"),
            }
        }
        Err(error) => eprintln!("render_to_image 失败：{error}"),
    }
}

/// 每帧推进脚本，直到抓图完成。
///
/// 每帧都 `refresh()`：脚本在"没有内容变化"的帧里也要继续推进（否则 `on_next_frame`
/// 不会再触发，等待会卡死）。
fn pump(window: &mut Window, script: Rc<RefCell<Script>>, captured: Rc<Cell<bool>>) {
    if captured.get() {
        return;
    }
    window.on_next_frame(move |window, cx| {
        // 先把这一步取出来：`match` 的临时借用会活到整个分支结束，
        // 分支里再 borrow 同一个 RefCell 就是 panic。
        let step = { script.borrow_mut().step(cx) };
        match step {
            Step::Idle => {}
            Step::Resize(size) => window.resize(gpui_kit::size(px(size.0), px(size.1))),
            Step::Click(at) => dispatch_click(window, cx, at),
            Step::Capture => {
                let out = script.borrow().out.clone();
                captured.set(true);
                capture(window, &out);
            }
        }
        if captured.get() {
            cx.quit();
            return;
        }
        window.refresh();
        pump(window, Rc::clone(&script), Rc::clone(&captured));
    });
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // 位置参数与开关参数分家：界面装配闭包要求 'static，所以这里一律**取值**
    // （Destination / ThemePreference / bool），不把 `args` 的借用带进去。
    let first_flag = args
        .iter()
        .position(|arg| arg.starts_with("--"))
        .unwrap_or(args.len());
    let positional: Vec<String> = args[..first_flag].to_vec();
    let flags: Vec<String> = args[first_flag..].to_vec();

    let out = positional.first().map(PathBuf::from).unwrap_or_else(|| {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../shots/workbench-offscreen.png")
    });
    let screen = positional
        .get(1)
        .map(String::as_str)
        .unwrap_or("environments");
    let theme = theme_by_name(positional.get(2).map(String::as_str).unwrap_or("light"));
    let seeded = positional.get(3).map(String::as_str).unwrap_or("sample") == "sample";

    let mut clicks: VecDeque<(f32, f32)> = VecDeque::new();
    let mut settle = Duration::from_millis(2500);
    let mut resize: Option<(f32, f32)> = None;
    // 窗口落位（默认屏幕外）。`--origin 0,0` 把它拉回屏内，用来验证
    // "看不见的窗口"与"看得见的窗口"在换帧回调上的差别。
    let mut origin: Option<(f32, f32)> = None;
    let mut index = 0;
    while index < flags.len() {
        match flags[index].as_str() {
            "--click" => {
                index += 1;
                let raw = flags.get(index).cloned().unwrap_or_default();
                let (x, y) = raw.split_once(',').unwrap_or(("0", "0"));
                let parse = |text: &str| text.trim().parse::<f32>().unwrap_or(0.0);
                clicks.push_back((parse(x), parse(y)));
            }
            "--wait" => {
                index += 1;
                settle = Duration::from_millis(
                    flags
                        .get(index)
                        .and_then(|raw| raw.trim().parse::<u64>().ok())
                        .unwrap_or(2500),
                );
            }
            "--origin" => {
                index += 1;
                let raw = flags.get(index).cloned().unwrap_or_default();
                if let Some((x, y)) = raw.split_once(',') {
                    let parse = |text: &str| text.trim().parse::<f32>().ok();
                    origin = Some((parse(x), parse(y))).and_then(|(x, y)| Some((x?, y?)));
                }
            }
            "--size" => {
                index += 1;
                let raw = flags.get(index).cloned().unwrap_or_default();
                if let Some((w, h)) = raw.split_once('x') {
                    let parse = |text: &str| text.trim().parse::<f32>().ok();
                    resize = Some((parse(w), parse(h))).and_then(|(w, h)| Some((w?, h?)));
                }
            }
            _ => {}
        }
        index += 1;
    }

    // 默认放在屏幕外：对用户不可见。
    let origin = origin.unwrap_or((-10_000.0, -10_000.0));

    let destination = screen_by_name(screen);
    let nav_index = Destination::ALL
        .iter()
        .position(|d| *d == destination)
        .unwrap_or(0);

    // 状态目录：走**产品自己的平台默认解析**（Windows `%LOCALAPPDATA%\envboard-demo`），
    // 而不是拼一条带 `..` 的构建目录路径 —— 后者会被设置屏原样显示，
    // 看起来像"应用把账本写进了源码树"。
    //
    // `-demo` 后缀保证演示数据永不落进真实账本（单实例锁也因此不冲突）；
    // 平台目录不可用时（精简容器 / CI 没有 LOCALAPPDATA）才回落到 `target/` 沙箱。
    let state_dir = envboard_host::options::platform_default_state_dir("envboard-demo")
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("target")
                .join("ui-shot-state")
        });
    let _ = std::fs::remove_dir_all(&state_dir);
    std::fs::create_dir_all(&state_dir).expect("应当能建状态目录");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("应当能建 tokio runtime");
    // **只把 Handle 交给闭包**。在 `move` 闭包里写 `&runtime` 会把 Runtime 本体挪进
    // 闭包，闭包函数体一返回就把运行时关掉（本夹具此前正是这样），此后任何 `spawn`
    // 的任务被 tokio 静默丢弃 —— 症状是"后端满数据、界面永远停在骨架加载态"，
    // 而拍出来的图看着像空态，谁也不会怀疑到装配顺序上。
    let handle = runtime.handle().clone();

    // 演示数据固定是 3 条环境；空账本模式期望 0（立刻算就绪）。
    let expect_rows_hint = if seeded { 3 } else { 0 };
    let captured = Rc::new(Cell::new(false));
    let result = captured.clone();
    let ready = Rc::new(Cell::new(false));
    let ready_result = ready.clone();
    // 闭包要用 state_dir，退出后还要清它 —— 所以给它一份 clone。
    let state_dir_for_run = state_dir.clone();

    gpui_kit::application()
        // 与产品入口同一个资产源：夹具与产品用不同的图标集，L1 就会漏掉
        // "图标名不在集合里 → 静默画空白"这类缺陷（代理屏「编辑」正是这样丢的）。
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            install_globals(cx, &state_dir_for_run, &handle);
            if seeded {
                let admin = cx.global::<Host>().admin().clone();
                seed(&admin, &handle);
                // 后端与界面分开报：只看到"界面 0 行"分不清是没数据还是没送达。
                match admin.snapshot() {
                    Ok((generation, rows)) => eprintln!(
                        "backend: generation={generation} rows={} ({})",
                        rows.len(),
                        rows.iter()
                            .map(|env| env.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    Err(error) => eprintln!("backend: snapshot 失败 {}", error.message),
                }
            }
            // 主题在开窗之前落实（与 `main.rs` 同一处顺序）：第一帧就是目标主题，
            // 而不是"先闪一帧浅色再换皮肤"。
            cx.set_global(UiPrefs { theme });
            envboard_desktop::ui_prefs::apply(theme, None, cx);

            // 放到屏幕外：对用户不可见，但仍然参与正常的换帧与合成。
            let (handle, root) = gpui_kit::open_window(
                gpui_kit::WindowOptions {
                    window_bounds: Some(gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds {
                        origin: gpui_kit::point(px(origin.0), px(origin.1)),
                        size: gpui_kit::size(px(1180.0), px(760.0)),
                    })),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| Workbench::new(window, cx)),
            )
            .expect("应当能建起窗口");

            // 出数判据：读**界面里**那张表的行数。走的是视图自己的状态，
            // 所以"后端有数据但界面没收到"这种断链会被它抓出来（本次就抓到了）。
            let probe = {
                let root = root.clone();
                Box::new(move |cx: &mut App| -> usize {
                    // 握着强引用，实体不会消失：直接 read。
                    let view = root.read(cx);
                    view.environments()
                        .read(cx)
                        .table()
                        .read(cx)
                        .delegate()
                        .total()
                })
            };
            let script = Rc::new(RefCell::new(Script {
                nav: (60.0, NAV_Y[nav_index]),
                resize,
                nav_pending: true,
                clicks,
                probe,
                expect_rows: expect_rows_hint,
                settle,
                since: Instant::now(),
                settled: false,
                ready: Rc::clone(&ready),
                out: out.clone(),
            }));
            handle
                .update(cx, |_, window, _| {
                    window.refresh();
                    pump(window, script, captured.clone());
                })
                .expect("应当能安排下一帧");
        });

    // 轮询循环是无界的，靠 Runtime 的默认 Drop 会一直等 —— 与 `main.rs` 同一处收尾。
    runtime.shutdown_timeout(Duration::from_secs(2));
    let _ = std::fs::remove_dir_all(&state_dir);
    // 出图不算成功：**数据没到界面**就是失败。这条把"骨架图看起来像空态"的
    // 假绿堵死（此前 L1 夹具的运行时被提前关掉，表格永远停在骨架，却照样出图）。
    if !result.get() || !ready_result.get() {
        if !ready_result.get() {
            eprintln!("shot: 超时未等到界面出数（期望 ≥{expect_rows_hint} 行）");
        }
        std::process::exit(1);
    }
}

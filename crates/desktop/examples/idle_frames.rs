//! L1：空闲静帧验收 —— 界面静止后，连续采样必须逐像素一致。
//!
//! 与 `screenshot.rs` 同一套装配（屏幕外真窗口 + `render_to_image` 的 D3D11
//! staging 回读，锁屏时也成立），区别只在最后一跳：不落 PNG，而是连续采样
//! 若干帧逐字节比较。
//!
//! 抓的是什么：动画没有收敛、渲染里混进了时间/随机量、轮询把某一屏改成
//! 每一拍都重排——这些都会让"静止"的界面在两次采样之间变样，而现有 L0
//! 测试（只测状态）和人工看图都发现不了。
//!
//! 不抓什么：平台 vsync 驱动的帧数。Windows 没有无头渲染器，窗口帧由平台
//! 推进，无法区分"应用请求的重绘"与"平台每帧都回调"；帧数预算要等平台层
//! 提供计数接口。这里只回答"画面有没有在变"。
//!
//! 用法：
//! ```text
//! cargo run -p envboard-desktop --example idle_frames -- [屏名] \
//!     [--wait 2500] [--samples 5] [--gap 100] [--spin]
//! ```
//!
//! - `屏名`：`environments` / `rules` / `proxies` / `debug` / `ca` / `settings`，
//!   切屏走真实点击（派发到组件自己的 on_click）。
//! - `--wait`：首屏就绪窗口（默认 2500ms，够 1s 轮询把快照送进来）。
//! - `--samples`：参与比较的帧数（默认 12；配 `--gap 100` 覆盖 1.2s，跨过一个
//!   轮询周期）。
//! - `--gap`：两次采样之间的间隔（默认 100ms）。
//! - `--warmup`：先丢几帧不比较（默认 3），吸收冷启动的一次性整窗重建。
//! - `--click X,Y`：可重复，按顺序**每处一次真实点击**（逻辑像素，两次点击之间
//!   留 300ms）。给了点击时：点击前先抓一帧做覆盖基线，点完等弹层动画落定再采样；
//!   第一次采样与基线做**覆盖断言**（差异区域之外必须逐像素一致）。
//! - `--spin`：反向验证用。每次采样前把窗口宽度在 1180/1181 之间切换，
//!   画面必然不同；验收必须报红，否则说明比较逻辑本身失效。
//!
//! 退出码：0 表示所有采样一致；1 表示有差异或渲染失败。

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use envboard_desktop::{Destination, Host, ThemePreference, UiPrefs, Workbench};
use envboard_engine_fake::FakeEngine;
use envboard_host::{HostOptions, prepare_with};
use gpui_kit::{App, AppContext as _, InputEvent as _, Window, px};
use tokio::runtime::Handle;

/// 装配界面需要的全局对象：一个真管理器 + 假引擎（与 screenshot.rs 同一份）。
fn install_globals(cx: &mut App, state_dir: &Path, handle: &Handle) {
    gpui_kit::init(cx);

    let options = HostOptions {
        state_dir: Some(state_dir.to_path_buf()),
        app_dir_name: None,
        port_range: Some((17_200, 17_299)),
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

/// 侧栏各项中心的 y（与 screenshot.rs 同一组实测探针）。
const NAV_Y: [f32; 6] = [52., 88., 124., 160., 200., 236.];

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

enum Step {
    Idle,
    Click((f32, f32)),
    /// 抓"点击前"的一帧存为覆盖断言的基线。
    PreCapture,
    Capture,
}

/// 对话框/弹层打开后的动画落定窗口：gpui-kit 的弹层过渡约 150–250ms。
const DIALOG_SETTLE: Duration = Duration::from_millis(600);

/// 两次点击之间的间隔（与 screenshot.rs 一致）：留一帧让上一次点击的结果画出来。
const CLICK_GAP: Duration = Duration::from_millis(300);

/// 采样脚本：切屏 → 等首屏就绪 → [抓基线 → 真实点击 → 等弹层落定] → 按间隔连续采样。
struct Script {
    nav: (f32, f32),
    nav_pending: bool,
    /// 导航点击之后的时间起点：`settle` 只认它，不随后续采样重置。
    started: Instant,
    /// 上一次动作之后的时间起点：只用于 `gap` / 点击间隔 / 弹层落定。
    since: Instant,
    settle: Duration,
    samples: usize,
    gap: Duration,
    /// 前几帧只更新基线、不参与比较：吸收冷启动的一次性整窗重建
    /// （字体/图标/GPU 管线首次就绪），它不是"空闲"要判的东西。
    warmup: usize,
    taken: usize,
    prev: Option<(u32, u32, Vec<u8>)>,
    /// 覆盖断言的基线：`--click` 打开弹层**前**的一帧。第一次采样时与当前帧比：
    /// 差异区域即弹层，区域外必须逐像素一致（z 序错 / 没盖住都在这里红）。
    baseline: Option<(u32, u32, Vec<u8>)>,
    /// 打开弹层的真实点击（`--click X,Y`，可重复）。
    clicks: std::collections::VecDeque<(f32, f32)>,
    clicked: bool,
    differs: usize,
    spin: bool,
    spin_wide: bool,
    done: Rc<Cell<bool>>,
    ok: Rc<Cell<bool>>,
}

impl Script {
    fn step(&mut self) -> Step {
        if self.nav_pending {
            self.nav_pending = false;
            self.started = Instant::now();
            self.since = self.started;
            return Step::Click(self.nav);
        }
        // 与 screenshot.rs 同一条纪律：按墙钟等，不按帧数等。离屏窗口换帧
        // 可能远超 60fps，按帧计会在 1s 轮询把快照送进来之前就采样。
        if self.started.elapsed() < self.settle {
            return Step::Idle;
        }
        if !self.clicks.is_empty() {
            // 先抓"点击前"的一帧做覆盖基线，再逐个派发点击。
            if self.baseline.is_none() {
                return Step::PreCapture;
            }
            if self.since.elapsed() < CLICK_GAP {
                return Step::Idle;
            }
            let at = self.clicks.pop_front().expect("刚判过非空");
            self.since = Instant::now();
            self.clicked = true;
            return Step::Click(at);
        }
        if self.clicked && self.since.elapsed() < DIALOG_SETTLE {
            // 弹层动画落定之前不采样：半透明过渡会让每一帧都不同。
            return Step::Idle;
        }
        if self.taken > 0 && self.since.elapsed() < self.gap {
            return Step::Idle;
        }
        Step::Capture
    }
}

/// 采一帧并与上一帧比较；返回 false 表示本次采样失败（渲染错误）。
fn sample(window: &mut Window, script: &Rc<RefCell<Script>>) -> bool {
    if script.borrow().spin {
        // 反向验证：改 1px 宽度，画面与尺寸都必然不同。
        let mut s = script.borrow_mut();
        let width = if s.spin_wide { 1181.0 } else { 1180.0 };
        s.spin_wide = !s.spin_wide;
        window.resize(gpui_kit::size(px(width), px(760.0)));
    }

    let image = match window.render_to_image() {
        Ok(image) => image,
        Err(error) => {
            eprintln!("idle: render_to_image 失败：{error}");
            return false;
        }
    };

    let mut s = script.borrow_mut();
    let (width, height) = (image.width(), image.height());
    let raw = image.as_raw().clone();
    // 覆盖断言（只做一次）：打开弹层后的第一帧 vs 点击前的基线。
    // 差异区域即弹层；区域外必须逐像素一致 —— z 序错、没盖住、把列表挤窄，
    // 都会在这里红。整屏差异 ≥80% 视为全屏遮罩，"区域外"为空集，跳过。
    if s.taken == s.warmup
        && let Some((bw, bh, base)) = s.baseline.take()
    {
        if (bw, bh) != (width, height) {
            eprintln!("cover: 基线与当前帧尺寸不同 {bw}x{bh} → {width}x{height}，跳过覆盖断言");
        } else {
            let (mut min_x, mut min_y, mut max_x, mut max_y) =
                (width as usize, height as usize, 0usize, 0usize);
            let mut changed = 0usize;
            let (base_px, now_px) = (base.as_chunks::<4>().0, raw.as_chunks::<4>().0);
            for (index, (a, b)) in base_px.iter().zip(now_px).enumerate() {
                if a == b {
                    continue;
                }
                changed += 1;
                let x = index % width as usize;
                let y = index / width as usize;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
            let total = width as usize * height as usize;
            if changed == 0 {
                eprintln!("cover: 打开后画面与点击前完全相同 —— 点击没有打开任何覆盖层");
                s.ok.set(false);
            } else if changed * 5 >= total * 4 {
                eprintln!(
                    "cover: 打开后整屏变化（{changed}/{total}），视为全屏遮罩，区域外断言为空集"
                );
            } else {
                let mut outside = 0usize;
                for (index, (a, b)) in base_px.iter().zip(now_px).enumerate() {
                    let x = index % width as usize;
                    let y = index / width as usize;
                    if (min_x..=max_x).contains(&x) && (min_y..=max_y).contains(&y) {
                        continue;
                    }
                    if a != b {
                        outside += 1;
                    }
                }
                if outside > 0 {
                    eprintln!(
                        "cover: 弹层区域 x{min_x}..{max_x} y{min_y}..{max_y} 之外仍有 {outside} 个像素变化 —— 覆盖不成立"
                    );
                    s.ok.set(false);
                } else {
                    eprintln!(
                        "cover: 覆盖成立（弹层 x{min_x}..{max_x} y{min_y}..{max_y}，区域外逐像素一致）"
                    );
                }
            }
        }
    }
    // warmup 期间只更新基线，不比较：冷启动的一次性整窗重建不算空闲问题。
    if s.taken >= s.warmup
        && let Some((pw, ph, prev)) = s.prev.take()
    {
        if (pw, ph) != (width, height) {
            eprintln!(
                "idle: 第 {} 帧尺寸变了 {pw}x{ph} → {width}x{height}（窗口被重排或采样期间换了尺寸）",
                s.taken + 1
            );
            s.differs += 1;
            s.ok.set(false);
        } else if prev != raw {
            // 差异区域：只报包围盒，不落 PNG（PNG 会掩盖持续变化的范围）。
            let (mut min_x, mut min_y, mut max_x, mut max_y) =
                (width as usize, height as usize, 0usize, 0usize);
            let mut changed = 0usize;
            let (prev_px, raw_px) = (prev.as_chunks::<4>().0, raw.as_chunks::<4>().0);
            for (index, (a, b)) in prev_px.iter().zip(raw_px).enumerate() {
                if a == b {
                    continue;
                }
                changed += 1;
                let x = index % width as usize;
                let y = index / width as usize;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
            eprintln!(
                "idle: 第 {} 帧与上一帧不同：{} 个像素变化（共 {}），区域 x{}..{} y{}..{}",
                s.taken + 1,
                changed,
                width as usize * height as usize,
                min_x,
                max_x,
                min_y,
                max_y
            );
            s.differs += 1;
            s.ok.set(false);
        }
    }
    s.prev = Some((width, height, raw));
    s.taken += 1;
    if s.taken >= s.warmup + s.samples {
        s.done.set(true);
    } else {
        s.since = Instant::now();
    }
    true
}

/// 每帧推进脚本，直到采样完成。
fn pump(window: &mut Window, script: Rc<RefCell<Script>>) {
    if script.borrow().done.get() {
        return;
    }
    window.on_next_frame(move |window, cx| {
        let step = {
            let mut s = script.borrow_mut();
            s.step()
        };
        if std::env::var_os("IDLE_FRAMES_VERBOSE").is_some() {
            let s = script.borrow();
            eprintln!(
                "frame: taken={} since={:?} gap={:?} settle={:?} clicks={} base={}",
                s.taken,
                s.since.elapsed(),
                s.gap,
                s.settle,
                s.clicks.len(),
                s.baseline.is_some()
            );
        }
        match step {
            Step::Idle => {}
            Step::Click(at) => dispatch_click(window, cx, at),
            Step::PreCapture => {
                // 点击前的基线帧：只存，不比较。
                match window.render_to_image() {
                    Ok(image) => {
                        let mut s = script.borrow_mut();
                        s.baseline = Some((image.width(), image.height(), image.as_raw().clone()));
                        s.since = Instant::now();
                    }
                    Err(error) => {
                        eprintln!("idle: render_to_image 失败：{error}");
                        script.borrow_mut().ok.set(false);
                        script.borrow().done.set(true);
                    }
                }
            }
            Step::Capture => {
                if !sample(window, &script) {
                    script.borrow_mut().ok.set(false);
                    script.borrow().done.set(true);
                }
            }
        }
        if script.borrow().done.get() {
            cx.quit();
            return;
        }
        window.refresh();
        pump(window, Rc::clone(&script));
    });
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let first_flag = args
        .iter()
        .position(|arg| arg.starts_with("--"))
        .unwrap_or(args.len());
    let positional: Vec<String> = args[..first_flag].to_vec();
    let flags: Vec<String> = args[first_flag..].to_vec();

    let screen = positional.first().map(String::as_str).unwrap_or("settings");
    let mut settle = Duration::from_millis(2500);
    let mut samples = 12usize;
    let mut gap = Duration::from_millis(100);
    let mut warmup = 3usize;
    let mut spin = false;
    let mut onscreen = false;
    let mut clicks = std::collections::VecDeque::new();
    let mut index = 0;
    while index < flags.len() {
        match flags[index].as_str() {
            "--wait" => {
                index += 1;
                settle = Duration::from_millis(
                    flags
                        .get(index)
                        .and_then(|raw| raw.trim().parse::<u64>().ok())
                        .unwrap_or(2500),
                );
            }
            "--samples" => {
                index += 1;
                samples = flags
                    .get(index)
                    .and_then(|raw| raw.trim().parse::<usize>().ok())
                    .unwrap_or(5)
                    .max(2);
            }
            "--gap" => {
                index += 1;
                gap = Duration::from_millis(
                    flags
                        .get(index)
                        .and_then(|raw| raw.trim().parse::<u64>().ok())
                        .unwrap_or(100),
                );
            }
            "--warmup" => {
                index += 1;
                warmup = flags
                    .get(index)
                    .and_then(|raw| raw.trim().parse::<usize>().ok())
                    .unwrap_or(3);
            }
            "--spin" => spin = true,
            "--onscreen" => onscreen = true,
            "--click" => {
                index += 1;
                let raw = flags.get(index).cloned().unwrap_or_default();
                let (x, y) = raw.split_once(',').unwrap_or(("0", "0"));
                let parse = |text: &str| text.trim().parse::<f32>().unwrap_or(0.0);
                clicks.push_back((parse(x), parse(y)));
            }
            _ => {}
        }
        index += 1;
    }

    let destination = screen_by_name(screen);
    let nav_index = Destination::ALL
        .iter()
        .position(|d| *d == destination)
        .unwrap_or(0);

    // 默认放屏幕外：对用户不可见。`--onscreen` 拉回屏内排障——实测两种位置的
    // 换帧行为不同，屏幕内的可见窗口在部分 DPI/前台组合下会停帧。
    let origin = if onscreen {
        (120.0, 80.0)
    } else {
        (-10_000.0, -10_000.0)
    };

    // 整个流程的墙钟上限：窗口一旦不再换帧，pump 不会再被调用，必须有人兜底
    // 退出，否则验收会挂死。
    let budget = settle
        + Duration::from_millis((clicks.len() as u64) * CLICK_GAP.as_millis() as u64)
        + Duration::from_millis(((warmup + samples) as u64) * gap.as_millis() as u64)
        + Duration::from_secs(10);

    // 状态目录用独立后缀，避免与 screenshot 夹具（`envboard-demo`）共用库。
    let state_dir = envboard_host::options::platform_default_state_dir("envboard-idle-frames")
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("target")
                .join("idle-frames-state")
        });
    let _ = std::fs::remove_dir_all(&state_dir);
    std::fs::create_dir_all(&state_dir).expect("应当能建状态目录");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("应当能建 tokio runtime");
    let handle = runtime.handle().clone();

    let done = Rc::new(Cell::new(false));
    let ok = Rc::new(Cell::new(true));
    let ok_result = Rc::clone(&ok);
    let state_dir_for_run = state_dir.clone();

    // 兜底：窗口不再换帧时 pump 永远不会被调用，靠墙钟线程退出。
    let finished = Arc::new(AtomicBool::new(false));
    {
        let finished = Arc::clone(&finished);
        let state_dir = state_dir.clone();
        std::thread::spawn(move || {
            std::thread::sleep(budget);
            if !finished.load(Ordering::Acquire) {
                eprintln!(
                    "idle: 超时（{budget:?} 内没有采完）：窗口停止换帧或被平台节流"
                );
                let _ = std::fs::remove_dir_all(&state_dir);
                std::process::exit(2);
            }
        });
    }

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            install_globals(cx, &state_dir_for_run, &handle);
            cx.set_global(UiPrefs {
                theme: ThemePreference::Light,
            });
            envboard_desktop::ui_prefs::apply(ThemePreference::Light, None, cx);

            let (handle, _root) = gpui_kit::open_window(
                gpui_kit::WindowOptions {
                    window_bounds: Some(gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds {
                        // 屏幕外只用于排障：对用户不可见，但可能被 DWM 停帧。
                        origin: gpui_kit::point(px(origin.0), px(origin.1)),
                        size: gpui_kit::size(px(1180.0), px(760.0)),
                    })),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| Workbench::new(window, cx)),
            )
            .expect("应当能建起窗口");

            let script = Rc::new(RefCell::new(Script {
                nav: (60.0, NAV_Y[nav_index]),
                nav_pending: true,
                started: Instant::now(),
                since: Instant::now(),
                settle,
                samples,
                gap,
                warmup,
                taken: 0,
                prev: None,
                baseline: None,
                clicks,
                clicked: false,
                differs: 0,
                spin,
                spin_wide: false,
                done: Rc::clone(&done),
                ok: Rc::clone(&ok),
            }));
            handle
                .update(cx, |_, window, _| {
                    window.refresh();
                    pump(window, Rc::clone(&script));
                })
                .expect("应当能安排下一帧");
        });

    runtime.shutdown_timeout(Duration::from_secs(2));
    finished.store(true, Ordering::Release);
    let _ = std::fs::remove_dir_all(&state_dir);

    let ok = ok_result.get();
    if !ok {
        eprintln!("idle: 验收失败（有采样帧不一致或渲染出错）");
        std::process::exit(1);
    }
    println!("idle: 通过（{screen} 屏，{samples} 帧一致）");
}

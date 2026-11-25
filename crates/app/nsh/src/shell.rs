//! envboard 的壳：标题栏、顶部导航、三个页面、一条全局状态条、一条轻提示。
//!
//! 空间与度量的分工照 `docs/envboard-设计系统.md`：标题栏 40、导航 52、
//! 页签 40、行 33、表头 28、控件 32、抽屉 480、状态条 28。颜色一律经
//! `tokens.rs`（→ `crates/ui/themes/default.json`），页面里没有 hex；
//! 状态条的计数一律经 `model`，不手写“8 环境 · 3 运行”这种会漂的字符串。
//!
//! M3：`envs` 是一个信号，初值来自 `AdminService::snapshot()`；B1 起循环挂在
//! `env_sync` 那棵受 `live::visible()` 门控的子树里，可见时 800ms 兜底一拍、
//! 隐藏时整个卸载。写操作自己刷一次（`actions`），所以点完立刻变，不靠下一拍。
//! `NG_GOLDEN=1` 时不启宿主、用 `model::fixture()`——golden 要的是**像素**，不是账本内容。

use nsh_core::element::ImageSrc;
use nsh_core::prelude::*;

use envboard_ui::primitives as ui;
use envboard_ui::{font as f, metrics as m, tokens as t};

use crate::actions::{self, Ctx};
use crate::brand;
use crate::live;
use crate::model::{self, Env};
use crate::pages;
use crate::prefs::{self, ThemeChoice};
use crate::service;

/// 测试口径一律走 `envboard_nsh::harness_*`：默认构建（发布物）里这些变量无效，
/// 只连真宿主、只渲染真数据。
fn flag(name: &str) -> bool {
    crate::harness_flag(name)
}

fn dark() -> bool {
    flag("NG_DARK")
}

/// golden 走 fixture：不启宿主、不碰真状态目录，基线只反映像素。
fn fixture_mode() -> bool {
    flag("NG_GOLDEN")
}

/// 压力口径（G3）：`NG_STRESS=10000` 让首帧直接铺 10k 行（同 `fixture` 的前八条）。
/// 与 golden 一样不启宿主 —— 量的是列表渲染，不是账本写入。
fn stress_rows() -> usize {
    crate::harness_var("NG_STRESS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// `NG_PAGE=env|rules|cert|capture`；`rules` 是环境页的「规则」子页签，
/// `capture` 是抓包子页（M5）。`NG_CAPTURE=1` 等价于 `NG_PAGE=capture`
/// （抓包基线自己一个开关，不必和页面开关叠着记）。
fn page_from_env() -> usize {
    match crate::harness_var("NG_PAGE").unwrap_or_default().as_str() {
        "cert" => 1,
        _ => 0,
    }
}

fn capture_from_env() -> bool {
    flag("NG_CAPTURE") || crate::harness_var("NG_PAGE").as_deref() == Some("capture")
}

fn section_from_env() -> usize {
    match crate::harness_var("NG_PAGE").unwrap_or_default().as_str() {
        "rules" => 1,
        _ => 0,
    }
}

fn selected_from_env(envs: &[Env]) -> Option<String> {
    match crate::harness_var("NG_SELECTED") {
        Some(v) if !v.is_empty() => return Some(v),
        _ => {}
    }
    // 旧口径是“选中第 3 行”，现在行随数据变，改成按名字选：proxy-c 是 fixture 里
    // 那个不健康的环境，抽屉基线就是它。
    envs.iter().nth(2).map(|e| e.name.clone())
}

fn drawer_from_env() -> bool {
    flag("NG_DRAWER")
}

/// 主题初值：`NG_DARK=1`（golden 深色用例）→ 夜黛；没有宿主（golden/压力）→ 跟随系统；
/// 有宿主就读用户的 `ui.json`。
fn initial_theme(synthetic: bool) -> ThemeChoice {
    if dark() {
        return ThemeChoice::Dark;
    }
    if synthetic {
        return ThemeChoice::System;
    }
    service::ui_prefs_file()
        .and_then(prefs::theme)
        .unwrap_or(ThemeChoice::System)
}

/// 标题栏右侧那行内核版本：有宿主报真的，没有就留 M1 起的那个占位字面
/// （golden 是按它拍的，换它等于重拍基线）。
fn core_line() -> String {
    match service::core_version() {
        Some((_, version)) => format!("core {version}"),
        None => "core 0.1.0".to_owned(),
    }
}

/// 一次轮询：快照（账本代次 + 全量）。代次没变就不动信号 —— 800ms 一拍的
/// `Signal::set` 会白跑一轮 diff，列表虽然一样也会闪一下。
/// 失败写进错误槽（状态条会跟着变）；第一次成功清掉它。
fn poll_once(cx: Ctx, generation: &std::cell::Cell<u64>, first: bool) {
    apply_snapshot(cx, generation, service::snapshot(), first);
}

fn poll_snapshot(
    cx: Ctx,
    generation: &std::cell::Cell<u64>,
    snapshot: Result<(u64, Vec<envboard_protocol::EnvView>), envboard_engine::Error>,
) {
    apply_snapshot(cx, generation, snapshot, false);
}

fn apply_snapshot(
    cx: Ctx,
    generation: &std::cell::Cell<u64>,
    snapshot: Result<(u64, Vec<envboard_protocol::EnvView>), envboard_engine::Error>,
    first: bool,
) {
    match snapshot {
        Ok((cursor, views)) => {
            if !first && cursor == generation.get() {
                return;
            }
            generation.set(cursor);
            let envs: Vec<Env> = views.into_iter().map(Env::from_view).collect();
            // 抽屉选中的环境被删了：顺手关掉，不留一个空抽屉。
            if let Some(sel) = cx.selected.get() {
                if !envs.iter().any(|e| e.name == sel) {
                    cx.drawer.set(false);
                    cx.selected.set(None);
                }
            }
            cx.envs.set(envs);
            cx.error.set(None);
        }
        Err(error) => {
            cx.error_title.set("数据面读取失败");
            cx.error
                .set(Some(format!("{} · {}", error.code.as_str(), error.message)));
        }
    }
}

#[component]
pub fn App() -> impl IntoElement {
    let fixture = fixture_mode();
    let stress = stress_rows();
    let synthetic = fixture || stress > 0;
    let initial: Vec<Env> = if synthetic {
        model::fixture_n(if stress > 0 { stress } else { 8 })
    } else {
        Vec::new()
    };
    let envs = signal(initial.clone());
    let page = signal(page_from_env());
    let section = signal(section_from_env());
    let filter = signal(0usize);
    let search = signal(String::new());
    // 抓包子页：默认关；`NG_CAPTURE`/`NG_PAGE=capture` 时直接落在第一个环境上
    // （golden 与截图不用点到入口才有内容可拍）。
    let capture_env = signal(if capture_from_env() {
        initial.first().map(|env| env.name.clone())
    } else {
        None
    });
    let selected = signal(if synthetic {
        selected_from_env(&envs.get())
    } else {
        None
    });
    let drawer = signal(drawer_from_env());
    let error = signal(None::<String>);
    let error_title = signal("操作未生效");
    let toast = signal(None::<String>);
    let ca = signal(None::<model::Ca>);
    let ca_missing = signal(false);
    let settings = signal(flag("NG_SETTINGS"));
    let confirm = signal(None::<actions::Confirm>);
    // 主题的初值：`NG_DARK` 优先（golden 的深色用例），否则读用户偏好，
    // 都没有就跟随系统 —— 与桌面形态同一份 `ui.json`、同一套回退。
    let theme = signal(initial_theme(synthetic));
    // 「关闭窗口时退出程序」：默认常驻（方案 §6.1 第 6 条）。有宿主时读用户的
    // `ui.json`；没有宿主（golden / 压力）时不碰文件，取默认。
    let close_to_tray = signal(
        if synthetic {
            true
        } else {
            service::ui_prefs_file()
                .and_then(prefs::close_to_tray)
                .unwrap_or(true)
        },
    );

    let cx = Ctx {
        envs,
        error,
        error_title,
        toast,
        selected,
        drawer,
        ca,
        ca_missing,
        settings,
        confirm,
        theme,
        capture_env,
        close_to_tray,
    };

    // 落地的外观：偏好是“跟随系统”时交给平台自己判，别在这里猜系统是深是浅。
    set_color_scheme(theme.get().appearance());

    if flag("NG_GOLDEN") || flag("NG_PLAIN") {
        // 没有宿主也要有证书页：样本值 + 直接置位，截图与 golden 才稳定。
        ca.set(Some(model::fixture_ca()));
    }

    if flag("NG_CONFIRM") {
        // 截图/基线用：直接把删除确认框摆出来（删的是样本里那个不健康的环境）。
        confirm.set(Some(actions::Confirm::delete_env("proxy-c")));
    }

    if !synthetic {
        // 没有宿主时界面不许假装“已连接”：把启动失败的那句话直接放进错误槽。
        if service::service().is_none() {
            error_title.set("宿主启动失败");
            let line = service::boot_error().unwrap_or("宿主未启动");
            error.set(Some(line.to_owned()));
        }
    }

    // B1：环境快照的循环不再挂在 `App()` 上，而是挂进 `env_sync` 那棵受可见性
    // 门控的子树（见下面的 `when(live::visible, ...)`）。隐藏时子树卸载，
    // `spawn` 随 owner 结束、定时器被 `TimerKey` 取回 —— 隐藏态零唤醒。
    let live_data = !synthetic && service::service().is_some();

    // T1（P6）托盘探针优先：它要测的是「托盘行为能不能成立」，与产品托盘
    // 同时挂会各挂一份图标。只在 `NG_TRAY_PROBE=1` 的 harness 构建里生效。
    #[cfg(feature = "harness")]
    let probe = crate::tray_probe::enabled();
    #[cfg(not(feature = "harness"))]
    let probe = false;
    if probe {
        #[cfg(feature = "harness")]
        crate::tray_probe::install();
    } else {
        // T2：产品托盘（关窗即隐藏、单击唤出、菜单计数、tooltip）。
        crate::tray::install(cx);
    }

    // N2（方案 §5.2）：二次启动的激活请求由等待线程**推**给界面（B3 起不再轮询），
    // 界面把窗口拉到前台。产品代码，不在 harness 后面 —— 否则发布物里「双击唤出」
    // 就只剩 N1 的「请自己找窗口」。
    crate::single_instance::install();

    view()
        .flex_1()
        .w_full()
        .h_full()
        .flex_col()
        // The window declares Mica (`backdrop: Window`), so the page fill is
        // translucent: 缟 at 0.80, 乌黑 at 0.80.
        .bg(|| t::page())
        .overflow_hidden()
        // B1：只在窗口真的可见时挂载的「活体」子树。它不画任何东西（0×0、脱离
        // 流），只承载环境快照循环；隐藏即卸载，循环与定时器一起消失。
        .child(when(
            move || live_data && live::visible(),
            move || env_sync(cx),
        ))
        .child(title_bar(cx))
        .child(app_header(page, cx))
        .child(
            view()
                .flex_1()
                .w_full()
                .relative()
                .overflow_hidden()
                .child(when(
                    move || page.get() == 0 && capture_env.get().is_none(),
                    move || pages::env_page(cx, filter, section, search),
                ))
                // 抓包子页占的是环境页的位置（它是环境的下钻，不是第四个顶层页）。
                .child(when(
                    move || page.get() == 0 && capture_env.get().is_some(),
                    move || pages::capture::page(cx),
                ))
                .child(when(
                    move || page.get() == 1,
                    move || pages::cert_page(cx),
                ))
                .child(when(
                    move || page.get() == 0 && cx.drawer.get(),
                    move || pages::drawer(cx),
                ))
                // 轻提示：成功路径的一句话，浮在内容之上、状态条之上，3.2 秒自己走。
                .child(when(
                    move || cx.toast.get().is_some(),
                    move || toast_pill(cx.toast.get().unwrap_or_default()),
                )),
        )
        .child(status_bar(cx, page))
        // 两个模态：平台原生的模态层（焦点隔离、Esc 走 `on_cancel`），
        // 遮罩与卡片由 `ui::dialog_shell` 画。挂在最外层，遮罩才盖得住状态条。
        .child(when(move || cx.settings.get(), move || {
            pages::settings_dialog(cx)
        }))
        .child(when(move || cx.confirm.get().is_some(), move || {
            pages::confirm_dialog(cx)
        }))
}

/// 环境快照的「活体」循环（B1，EB-DESIGN-006 §6.1）。
///
/// 只在 `live::visible()` 为真时被 `when` 挂载。挂载即同步读一次 —— 从托盘唤回
/// 的第一帧就是新鲜数据，不等下一拍；之后按可见态兜底拍轮询，代次没变不动信号。
///
/// 卸载（隐藏 / 最小化 / 锁屏 / 切虚拟桌面）时 owner 结束，`spawn` 取消，
/// NSH 把这条 `timer` 从队列里取回。**所以这里不需要任何「暂停」代码。**
fn env_sync(cx: Ctx) -> impl IntoElement {
    let generation = std::cell::Cell::new(0u64);
    poll_once(cx, &generation, true);
    spawn(async move {
        loop {
            timer(std::time::Duration::from_millis(live::SNAPSHOT_FALLBACK_MS)).await;
            match background(async { service::snapshot() }).await {
                Ok(snap) => poll_snapshot(cx, &generation, snap),
                Err(error) => cx.error.set(Some(format!("轮询任务失败：{error}"))),
            }
        }
    });
    // 不占位、不参与布局：它只是个承载循环的挂载点。
    view().absolute().w(px(0.0)).h(px(0.0))
}

fn toast_pill(message: String) -> impl IntoElement {
    view()
        .absolute()
        .bottom(px(14.0))
        .left(px(0.0))
        .right(px(0.0))
        .flex_row()
        .justify_center()
        .child(
            view()
                .h(px(30.0))
                .px(px(14.0))
                .flex_row()
                .items_center()
                .rounded(px(m::OVERLAY_RADIUS))
                .border_1()
                .border_color(|| t::card_stroke())
                .bg(|| t::card())
                .child(
                    text(message)
                        .text_size(px(f::CAPTION))
                        .font_family(f::FONT_TEXT)
                        .text_color(|| t::ink()),
                ),
        )
}

// ---- 标题栏 --------------------------------------------------------------
//
// 平台接管窗框（`title_bar: Hidden`, `height: 40`），三个 caption 按钮由
// Windows 画在这条的右端，所以这里只留 138 pt 的位并让其余可拖。

/// 品牌记号：直接用应用图标本身（32×32 RGBA 上采样到 16 pt），不是另画的替形。
/// 与任务栏、Alt-Tab 看到的是同一枚图，换图标只换 `brand`。
fn brand_mark() -> impl IntoElement {
    let src = ImageSrc::rgba(brand::MARK_PX, brand::MARK_PX, brand::MARK_RGBA)
        .size(Size::new(px(20.0), px(20.0)));
    image(src).w(px(20.0)).h(px(20.0)).rounded(px(4.0))
}

fn title_bar(cx: Ctx) -> impl IntoElement {
    view()
        .h(px(m::TITLE_BAR_H))
        .w_full()
        .flex_row()
        .items_center()
        .window_drag(true)
        .relative()
        .pl(px(14.0))
        .child(brand_mark())
        .child(ui::spacer(10.0))
        .child(
            text(brand::NAME)
                .text_size(px(f::BODY))
                .font_family(f::FONT_DISPLAY)
                .text_color(|| t::ink()),
        )
        .child(ui::spacer(8.0))
        .child(
            text(brand::TAGLINE)
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_TEXT)
                .text_color(|| t::ink_tertiary()),
        )
        .child(view().flex_1())
        .child(
            text(core_line())
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_MONO)
                .text_color(|| t::ink_tertiary()),
        )
        .child(ui::spacer(12.0))
        // 连接状态：轮询失败时这里立刻变橙并换字，不用等状态条被看见。
        .child(
            view()
                .h(px(20.0))
                .px(px(8.0))
                .flex_row()
                .items_center()
                .rounded(px(10.0))
                .border_1()
                .border_color(|| t::control_stroke())
                .child(view().w(px(5.0)).h(px(5.0)).rounded(px(2.5)).bg(move || {
                    if cx.error.get().is_some() {
                        t::caution()
                    } else {
                        t::success()
                    }
                }))
                .child(ui::spacer(5.0))
                .child(
                    text(move || {
                        if cx.error.get().is_some() {
                            "连接异常"
                        } else {
                            "已连接"
                        }
                    })
                    .text_size(px(f::CAPTION))
                    .font_family(f::FONT_TEXT)
                    .text_color(|| t::ink_secondary()),
                ),
        )
        .child(ui::spacer(8.0))
        .child(
            view()
                .h(px(20.0))
                .px(px(8.0))
                .flex_row()
                .items_center()
                .rounded(px(10.0))
                .bg(|| t::hover_fill())
                .child(
                    // 主题名随切随变（V-1）：标题栏报的是“现在是什么外观”，
                    // 不是启动时是什么外观。入口在顶栏的「设置」上，这条只是回显。
                    text(move || cx.theme.get().label())
                        .text_size(px(f::CAPTION))
                        .font_family(f::FONT_TEXT)
                        .text_color(|| t::ink_tertiary()),
                ),
        )
        .child(ui::spacer(12.0))
        // Windows 画三个 caption 按钮的位置，一个不差。
        .child(view().w(px(m::CAPTION_W)))
}

// ---- 顶栏（主导航） ------------------------------------------------------

fn app_header(page: Signal<usize>, cx: Ctx) -> impl IntoElement {
    view()
        .h(px(52.0))
        .w_full()
        .px(px(16.0))
        .flex_row()
        .items_center()
        .border_b_1()
        .border_color(|| t::divider())
        .bg(|| t::layer())
        .child(ui::nav_tab(
            "环境",
            move || page.get() == 0,
            move || page.set(0),
        ))
        .child(ui::spacer(2.0))
        .child(ui::nav_tab(
            "证书",
            move || page.get() == 1,
            move || page.set(1),
        ))
        .child(view().flex_1())
        .child(
            text("数据面 IPC 直调 AdminService · 不起监听端口")
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_TEXT)
                .text_color(|| t::ink_tertiary()),
        )
        .child(ui::spacer(10.0))
        // 设置入口放顶栏而不是标题栏：标题栏整条是拖动区，放在那里的按钮会被拖拽吃掉。
        .child(ui::btn_text("设置", t::ink_secondary, move || {
            cx.settings.set(true)
        }))
}

// ---- 状态栏 --------------------------------------------------------------
//
// 只留「需要人处理的信息」：装配告警（没有就一句正常）与事件丢弃。
// 中段告警用赭石/缃色（`caution_ink`），琥珀只做点与条。
//
// 错误三层里的第二层就在中段：轮询/写操作失败时它写的是 `code · message`，
// 常驻、跨页可见，不会因为抽屉打开或滚动被藏起来。

fn status_bar(cx: Ctx, page: Signal<usize>) -> impl IntoElement {
    view()
        .h(px(m::STATUS_H))
        .w_full()
        .flex_row()
        .items_center()
        .px(px(14.0))
        .border_t_1()
        .border_color(|| t::divider())
        .bg(|| t::layer())
        .child(
            text(move || {
                if page.get() == 0 {
                    let c = model::counts(&cx.envs.get());
                    format!("{} 环境 · {} 运行 · {} 停止", c.total, c.running, c.stopped)
                } else {
                    match cx.ca.get() {
                        Some(ca) if ca.trusted() => "CA 就绪 · 已信任".to_owned(),
                        Some(_) => "CA 就绪 · 未信任".to_owned(),
                        None => "CA 不可用".to_owned(),
                    }
                }
            })
            .text_size(px(f::CAPTION))
            .font_family(f::FONT_TEXT)
            .text_color(|| t::ink_secondary()),
        )
        .child(view().flex_1())
        .child(
            text(move || {
                if page.get() != 0 {
                    return match cx.error.get() {
                        Some(line) => line,
                        None => {
                            let trusted = cx.ca.get().is_some_and(|ca| ca.trusted());
                            if trusted {
                                "证书已导入当前用户 ROOT 存储"
                            } else {
                                "证书未导入：本机 HTTPS 会报不受信任"
                            }
                            .to_owned()
                        }
                    };
                }
                match cx.error.get() {
                    Some(line) => line,
                    None => model::attention_line(&cx.envs.get()),
                }
            })
            .text_size(px(f::CAPTION))
            .font_family(f::FONT_TEXT)
            .text_color(move || {
                if cx.error.get().is_some() {
                    t::critical()
                } else if page.get() == 0 {
                    t::caution_ink()
                } else if cx.ca.get().is_some_and(|ca| ca.trusted()) {
                    t::ink_secondary()
                } else {
                    t::caution_ink()
                }
            }),
        )
        .child(view().flex_1())
        .child(
            text(move || {
                if page.get() == 0 {
                    "宿主 IPC · 无监听端口".to_owned()
                } else {
                    cx.ca
                        .get()
                        .map(|ca| fingerprint_short(&ca.fingerprint))
                        .unwrap_or_else(|| "—".to_owned())
                }
            })
            .text_size(px(f::CAPTION))
            .font_family(f::FONT_MONO)
            .text_color(|| t::ink_tertiary()),
        )
}

/// 指纹只显示前 8 个十六进制字符（`9A671086…`）：完整 64 字符塞进 28 高的状态条，
/// 谁也读不出来，页里那行才是完整值。引擎给的是**无分隔**的大写十六进制。
fn fingerprint_short(fingerprint: &str) -> String {
    let hex: String = fingerprint
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(8)
        .collect();
    if hex.len() < 8 {
        return fingerprint.to_owned();
    }
    format!("{hex}…")
}

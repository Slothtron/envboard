//! envboard 的壳：标题栏、顶部导航、三个页面、一条全局状态条。
//!
//! 空间与度量的分工照 `docs/envboard-设计系统.md`：标题栏 40、导航 52、
//! 页签 40、行 33、表头 28、控件 32、抽屉 480、状态条 28。颜色一律经
//! `tokens.rs`（→ `crates/ui/themes/default.json`），页面里没有 hex；
//! 状态条的计数一律经 `model`，不手写“8 环境 · 3 运行”这种会漂的字符串。

use nsh_core::prelude::*;

use envboard_ui::primitives as ui;
use envboard_ui::{font as f, metrics as m, tokens as t};

use crate::brand;
use crate::model::{self, ENVS};
use crate::pages;

fn flag(name: &str) -> bool {
    std::env::var(name).map(|v| v == "1").unwrap_or(false)
}

fn dark() -> bool {
    flag("NG_DARK")
}

/// `NG_PAGE=env|rules|cert`；`rules` 是环境页的「规则」子页签。
fn page_from_env() -> usize {
    match std::env::var("NG_PAGE").unwrap_or_default().as_str() {
        "cert" => 1,
        _ => 0,
    }
}

fn section_from_env() -> usize {
    match std::env::var("NG_PAGE").unwrap_or_default().as_str() {
        "rules" => 1,
        _ => 0,
    }
}

fn selected_from_env() -> usize {
    std::env::var("NG_SELECTED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2)
}

fn drawer_from_env() -> bool {
    flag("NG_DRAWER")
}

#[component]
pub fn App() -> impl IntoElement {
    let page = signal(page_from_env());
    let section = signal(section_from_env());
    let filter = signal(0usize);
    let selected = signal(selected_from_env());
    let drawer = signal(drawer_from_env());

    view()
        .flex_1()
        .w_full()
        .h_full()
        .flex_col()
        // The window declares Mica (`backdrop: Window`), so the page fill is
        // translucent: 缟 at 0.80, 乌黑 at 0.80.
        .bg(|| t::page())
        .overflow_hidden()
        .child(title_bar())
        .child(app_header(page))
        .child(
            view()
                .flex_1()
                .w_full()
                .relative()
                .overflow_hidden()
                .child(when(
                    move || page.get() == 0,
                    move || pages::env_page(selected, drawer, filter, section),
                ))
                .child(when(move || page.get() == 1, move || pages::cert_page(dark())))
                .child(when(
                    move || page.get() == 0 && drawer.get(),
                    move || pages::drawer(selected, drawer),
                )),
        )
        .child(status_bar(page))
}

// ---- 标题栏 --------------------------------------------------------------
//
// 平台接管窗框（`title_bar: Hidden`, `height: 40`），三个 caption 按钮由
// Windows 画在这条的右端，所以这里只留 138 pt 的位并让其余可拖。

/// 品牌记号：与 `assets/envboard.ico` 同一图形 —— 靛青圆角方 + 两条白流（上右下左）。
/// 16 pt 画不出真正的箭头三角，用两条长杠示意；任务栏上是完整图标。
fn brand_mark() -> impl IntoElement {
    view()
        .w(px(16.0))
        .h(px(16.0))
        .rounded(px(3.0))
        .bg(|| t::accent())
        .flex_col()
        .items_center()
        .justify_center()
        .child(view().w(px(9.0)).h(px(1.5)).bg(|| t::on_accent()))
        .child(view().h(px(2.0)))
        .child(view().w(px(9.0)).h(px(1.5)).bg(|| t::on_accent()))
}

fn title_bar() -> impl IntoElement {
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
            text("core 0.1.0")
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_MONO)
                .text_color(|| t::ink_tertiary()),
        )
        .child(ui::spacer(12.0))
        .child(
            view()
                .h(px(20.0))
                .px(px(8.0))
                .flex_row()
                .items_center()
                .rounded(px(10.0))
                .border_1()
                .border_color(|| t::control_stroke())
                .child(view().w(px(5.0)).h(px(5.0)).rounded(px(2.5)).bg(|| t::success()))
                .child(ui::spacer(5.0))
                .child(
                    text("已连接")
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
                    text(move || if dark() { "夜黛" } else { "宣纸" }.to_owned())
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

fn app_header(page: Signal<usize>) -> impl IntoElement {
    view()
        .h(px(52.0))
        .w_full()
        .px(px(16.0))
        .flex_row()
        .items_center()
        .border_b_1()
        .border_color(|| t::divider())
        .bg(|| t::layer())
        .child(ui::nav_tab("环境", move || page.get() == 0, move || page.set(0)))
        .child(ui::spacer(2.0))
        .child(ui::nav_tab("证书", move || page.get() == 1, move || page.set(1)))
        .child(view().flex_1())
        .child(
            text("数据面 IPC 直调 AdminService · 不起监听端口")
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_TEXT)
                .text_color(|| t::ink_tertiary()),
        )
}

// ---- 状态栏 --------------------------------------------------------------
//
// 只留「需要人处理的信息」：装配告警（没有就一句正常）与事件丢弃。
// 中段告警用赭石/缃色（`caution_ink`），琥珀只做点与条。

fn status_bar(page: Signal<usize>) -> impl IntoElement {
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
                    let c = model::counts(&ENVS);
                    format!("{} 环境 · {} 运行 · {} 停止", c.total, c.running, c.stopped)
                } else {
                    "CA 就绪 · 未信任".to_owned()
                }
            })
            .text_size(px(f::CAPTION))
            .font_family(f::FONT_TEXT)
            .text_color(|| t::ink_secondary()),
        )
        .child(view().flex_1())
        .child(
            text(move || {
                if page.get() == 0 {
                    model::attention_line(&ENVS)
                } else {
                    "证书 1 张 · 当前用户 ROOT".to_owned()
                }
            })
            .text_size(px(f::CAPTION))
            .font_family(f::FONT_TEXT)
            .text_color(move || {
                if page.get() == 0 {
                    t::caution_ink()
                } else {
                    t::ink_tertiary()
                }
            }),
        )
        .child(view().flex_1())
        .child(
            text(move || {
                if page.get() == 0 {
                    "丢弃 0 · 12 req/s".to_owned()
                } else {
                    "SHA-256 · ECDSA P-256".to_owned()
                }
            })
            .text_size(px(f::CAPTION))
            .font_family(f::FONT_MONO)
            .text_color(|| t::ink_tertiary()),
        )
}

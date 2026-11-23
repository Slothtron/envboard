//! 八个原语，一次画好，页面只组合它们。
//!
//! 原语只接**内容与回调**：文字、宽度、是否选中（一个每帧求值的闭包）、点击后做什么。
//! 没有一处 `if health == ...` —— “不健康要不要变黄”是业务判断，在 `model::health` 里；
//! 到这里只剩 `Tone`（四档语气）与 `Shape`（五种可辨形状）。
//!
//! 每个原语的颜色都从 `tokens` 的名字里取，没有 hex，也没有“第 3 个灰度”。

use nsh_core::element::AdaptiveColor;
use nsh_core::prelude::*;

use crate::{font as f, metrics as m, tokens as t};

// ---- 语气与形状 ----------------------------------------------------------

/// 颜色的四档语气。只有这四档：多一档就没人记得住哪档该用在哪。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    /// 中性：已停止、说明性文字。
    Neutral,
    /// 正常：运行中。
    Success,
    /// 需处理：不健康、未生效、未导入。
    Caution,
    /// 坏了：启动失败、端口冲突、破坏性动作。
    Critical,
}

impl Tone {
    /// 点 / 条用的色。琥珀只在这一档出现，且只做形状，不做句子。
    fn dot(self) -> fn() -> AdaptiveColor {
        match self {
            Tone::Neutral => t::ink_tertiary,
            Tone::Success => t::success,
            Tone::Caution => t::caution,
            Tone::Critical => t::critical,
        }
    }

    /// 文字用的色。琥珀当文字只有 3.21:1，所以 Caution 的墨换成赭石。
    pub fn ink(self) -> fn() -> AdaptiveColor {
        match self {
            Tone::Neutral => t::ink_secondary,
            Tone::Success => t::success,
            Tone::Caution => t::caution_ink,
            Tone::Critical => t::critical,
        }
    }
}

/// 状态的四态各自带一个形状，红/橙永远不单独承担语义。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    /// 实心 6 pt 点：运行中。
    Dot,
    /// 空心 6 pt 环：已停止。
    Ring,
    /// 点 + 4 pt 横条：不健康。
    DotBar,
    /// 点 + 8 pt 竖条：端口冲突。
    DotVBar,
    /// 点 + 叹号：启动失败。
    DotBang,
}

fn marker(tone: Tone, shape: Shape) -> AnyElement {
    match shape {
        Shape::DotBar => view()
            .w(px(10.0))
            .h(px(m::BAR_W))
            .bg(move || (tone.dot())())
            .into_element(),
        Shape::DotVBar => view()
            .w(px(m::BAR_W))
            .h(px(9.0))
            .bg(move || (tone.dot())())
            .into_element(),
        Shape::DotBang => text("!")
            .text_size(px(10.0))
            .font_family(f::FONT_TEXT)
            .text_color(move || (tone.dot())())
            .into_element(),
        Shape::Dot | Shape::Ring => view().w(px(0.0)).into_element(),
    }
}

/// 点 · 形状 · 文字，`width` 宽（列表里传列宽，抽屉头里传自然宽）。
pub fn status_cell(tone: Tone, shape: Shape, label: &'static str, width: f32) -> impl IntoElement {
    let solid = !matches!(shape, Shape::Ring);
    view()
        .w(px(width))
        .flex_row()
        .items_center()
        .pl(px(12.0))
        .child(
            view()
                .w(px(6.0))
                .h(px(6.0))
                .rounded(px(3.0))
                .border_1()
                .border_color(move || {
                    if solid {
                        AdaptiveColor::TRANSPARENT
                    } else {
                        (tone.dot())()
                    }
                })
                .bg(move || {
                    if solid {
                        (tone.dot())()
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                }),
        )
        .child(marker(tone, shape))
        .child(view().w(px(6.0)))
        .child(
            text(label)
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_TEXT)
                .text_color(|| t::ink_secondary()),
        )
}

// ---- 表格 ----------------------------------------------------------------

pub fn spacer(w: f32) -> impl IntoElement {
    view().w(px(w))
}

pub fn vspacer(h: f32) -> impl IntoElement {
    view().h(px(h))
}

fn cell_base(
    label: impl IntoReactive<SharedString> + 'static,
    width: f32,
    font: &'static str,
    ink: fn() -> AdaptiveColor,
) -> impl IntoElement {
    view()
        .w(px(width))
        .flex_row()
        .items_center()
        .pl(px(12.0))
        .child(
            text(label)
                .text_size(px(f::CAPTION))
                .font_family(font)
                .text_color(move || ink()),
        )
}

pub fn head_cell(label: &'static str, width: f32) -> impl IntoElement {
    view()
        .w(px(width))
        .flex_row()
        .items_center()
        .pl(px(12.0))
        .child(
            text(label)
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_TEXT)
                .text_color(|| t::ink_tertiary()),
        )
}

/// 会长的那一列（环境名）：吃掉固定列剩下的宽度。
pub fn head_cell_grow(label: &'static str) -> impl IntoElement {
    view()
        .flex_1()
        .flex_row()
        .items_center()
        .pl(px(12.0))
        .child(
            text(label)
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_TEXT)
                .text_color(|| t::ink_tertiary()),
        )
}

pub fn cell(label: impl IntoReactive<SharedString> + 'static, width: f32) -> impl IntoElement {
    cell_base(label, width, f::FONT_TEXT, t::ink)
}

pub fn cell_grow(label: impl IntoReactive<SharedString> + 'static) -> impl IntoElement {
    view()
        .flex_1()
        .flex_row()
        .items_center()
        .pl(px(12.0))
        .child(
            text(label)
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_TEXT)
                .text_color(|| t::ink()),
        )
}

pub fn mono_cell(label: impl IntoReactive<SharedString> + 'static, width: f32) -> impl IntoElement {
    cell_base(label, width, f::FONT_MONO, t::ink_secondary)
}

/// 静默的次要列（规则名、最近活动）：三级墨，不和名字抢。
pub fn quiet_cell(label: impl IntoReactive<SharedString> + 'static, width: f32) -> impl IntoElement {
    cell_base(label, width, f::FONT_TEXT, t::ink_tertiary)
}

// ---- 行 ------------------------------------------------------------------

/// 一行的壳：3 pt 选中条 + 下边线 + 悬停/选中底 + 点击。
/// `is_sel` 每帧读一次，所以高亮跟着信号走，不用重建行。
pub fn row_shell<S, O>(is_sel: S, on: O) -> impl FnOnce(Vec<AnyElement>) -> AnyElement
where
    S: Fn() -> bool + Copy + 'static,
    O: Fn() + Copy + 'static,
{
    move |cells: Vec<AnyElement>| {
        view()
            .h(px(m::ROW_H))
            .w_full()
            .flex_row()
            .items_center()
            .border_b_1()
            .border_color(|| t::divider())
            .bg(move || {
                if is_sel() {
                    t::selection()
                } else {
                    t::layer()
                }
            })
            .hover(move |s| {
                if is_sel() {
                    s.bg(move || t::selection())
                } else {
                    s.bg(move || t::hover_fill())
                }
            })
            .on_press(move || on())
            .child(
                view()
                    .w(px(m::BAR_W))
                    .h(px(m::ROW_H))
                    .bg(move || {
                        if is_sel() {
                            t::accent()
                        } else {
                            t::layer()
                        }
                    }),
            )
            .children(cells)
            .into_element()
    }
}

// ---- 线、按钮、页签 ------------------------------------------------------

pub fn divider() -> impl IntoElement {
    view().w_full().h(px(1.0)).bg(|| t::divider())
}

/// 比 `divider` 重一档的线，用于卡内分区（如证书页安装区与信息区之间）。
pub fn toned_divider() -> impl IntoElement {
    view().w_full().h(px(1.0)).bg(|| t::control_stroke())
}

pub fn btn_primary<O: Fn() + Copy + 'static>(label: &'static str, on: O) -> impl IntoElement {
    view()
        .h(px(m::CONTROL_H))
        .px(px(14.0))
        .flex_row()
        .items_center()
        .rounded(px(m::RADIUS))
        .bg(|| t::accent())
        .hover(|s| s.bg(|| t::accent_hover()))
        .on_press(move || on())
        .child(
            text(label)
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_TEXT)
                .text_color(|| t::on_accent()),
        )
}

pub fn btn_subtle<O: Fn() + Copy + 'static>(label: &'static str, on: O) -> impl IntoElement {
    view()
        .h(px(m::CONTROL_H))
        .px(px(14.0))
        .flex_row()
        .items_center()
        .rounded(px(m::RADIUS))
        .border_1()
        .border_color(|| t::control_stroke())
        .bg(|| t::control_fill())
        .hover(|s| s.bg(|| t::hover_fill()))
        .on_press(move || on())
        .child(
            text(label)
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_TEXT)
                .text_color(|| t::ink()),
        )
}

/// 只当文字看的按钮：操作列、抽屉的 ✕、删除环境。24 高，四周 6 pt 留白刚好能点中。
pub fn btn_text<O: Fn() + Copy + 'static>(
    label: &'static str,
    ink: fn() -> AdaptiveColor,
    on: O,
) -> impl IntoElement {
    view()
        .h(px(24.0))
        .px(px(6.0))
        .flex_row()
        .items_center()
        .rounded(px(m::RADIUS))
        .hover(|s| s.bg(|| t::hover_fill()))
        .on_press(move || on())
        .child(
            text(label)
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_TEXT)
                .text_color(move || ink()),
        )
}

/// 筛选片：标签 + 计数，选中态由它自己的闭包决定颜色。
///
/// 计数可以是闭包（`pill_live`）：M3 起它是从真数据现算的，切换环境、启停之后
/// 同屏的四个计数必须一起动，不能等下次重开窗口。
pub fn pill<S, O>(label: &'static str, count: usize, is_sel: S, on: O) -> impl IntoElement
where
    S: Fn() -> bool + Copy + 'static,
    O: Fn() + Copy + 'static,
{
    pill_live(label, move || count, is_sel, on)
}

pub fn pill_live<S, O, C>(label: &'static str, count: C, is_sel: S, on: O) -> impl IntoElement
where
    S: Fn() -> bool + Copy + 'static,
    O: Fn() + Copy + 'static,
    C: Fn() -> usize + Copy + 'static,
{
    view()
        .h(px(26.0))
        .px(px(10.0))
        .flex_row()
        .items_center()
        .rounded(px(m::RADIUS))
        .border_1()
        .border_color(move || {
            if is_sel() {
                t::accent()
            } else {
                t::control_stroke()
            }
        })
        .bg(move || {
            if is_sel() {
                t::selection()
            } else {
                t::control_fill()
            }
        })
        .hover(move |s| {
            if is_sel() {
                s
            } else {
                s.bg(move || t::hover_fill())
            }
        })
        .on_press(move || on())
        .child(
            text(label)
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_TEXT)
                .text_color(move || {
                    if is_sel() {
                        t::accent_ink()
                    } else {
                        t::ink_secondary()
                    }
                }),
        )
        .child(view().w(px(6.0)))
        .child(
            text(move || count().to_string())
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_MONO)
                .text_color(move || {
                    if is_sel() {
                        t::accent_ink()
                    } else {
                        t::ink_tertiary()
                    }
                }),
        )
}

/// 顶部页签：文字 + 2 pt 下划线（当前页才有）。
pub fn nav_tab<S, O>(label: &'static str, is_sel: S, on: O) -> impl IntoElement
where
    S: Fn() -> bool + Copy + 'static,
    O: Fn() + Copy + 'static,
{
    view()
        .h(px(m::TAB_H))
        .px(px(12.0))
        .flex_col()
        .items_center()
        .justify_center()
        .on_press(move || on())
        .child(
            text(label)
                .text_size(px(f::BODY))
                .font_family(f::FONT_TEXT)
                .text_color(move || {
                    if is_sel() {
                        t::ink()
                    } else {
                        t::ink_tertiary()
                    }
                }),
        )
        .child(vspacer(6.0))
        .child(
            view().w(px(20.0)).h(px(2.0)).bg(move || {
                if is_sel() {
                    t::accent()
                } else {
                    t::page()
                }
            }),
        )
}

// ---- 详情件 --------------------------------------------------------------

pub fn section_label(label: &'static str) -> impl IntoElement {
    view().pb(px(6.0)).child(
        text(label)
            .text_size(px(f::CAPTION))
            .font_family(f::FONT_TEXT)
            .text_color(|| t::ink_tertiary()),
    )
}

/// 键（84 pt，二级墨）+ 值，行高 28。
pub fn kv(
    k: impl IntoReactive<SharedString> + 'static,
    v: impl IntoReactive<SharedString> + 'static,
    mono: bool,
) -> impl IntoElement {
    view()
        .h(px(28.0))
        .flex_row()
        .items_center()
        .child(
            view().w(px(84.0)).child(
                text(k)
                    .text_size(px(f::CAPTION))
                    .font_family(f::FONT_TEXT)
                    .text_color(|| t::ink_secondary()),
            ),
        )
        .child(view().flex_1().child(
            text(v)
                .text_size(px(f::CAPTION))
                .font_family(if mono { f::FONT_MONO } else { f::FONT_TEXT })
                .text_color(|| t::ink()),
        ))
}

/// 3 pt 语气条 + 标题 + 说明，卡里唯一的一块颜色。
pub fn alert(
    tone: Tone,
    title: impl IntoReactive<SharedString> + 'static,
    desc: impl IntoReactive<SharedString> + 'static,
) -> impl IntoElement {
    let bar: fn() -> AdaptiveColor = match tone {
        Tone::Neutral => t::control_stroke,
        Tone::Success => t::success,
        Tone::Caution => t::caution,
        Tone::Critical => t::critical,
    };
    let ink = tone.ink();
    view()
        .w_full()
        .flex_row()
        .rounded(px(m::RADIUS))
        .border_1()
        .border_color(|| t::control_stroke())
        .bg(|| t::control_fill())
        .overflow_hidden()
        .child(view().w(px(m::BAR_W)).h_full().bg(move || bar()))
        .child(
            view()
                .flex_1()
                .flex_col()
                .p(px(10.0))
                .child(
                    text(title)
                        .text_size(px(f::CAPTION))
                        .font_family(f::FONT_TEXT)
                        .text_color(move || ink()),
                )
                .child(vspacer(3.0))
                .child(
                    text(desc)
                        .text_size(px(f::CAPTION))
                        .font_family(f::FONT_TEXT)
                        .text_color(|| t::ink_tertiary()),
                ),
        )
}

/// 行内错误条：一次写操作失败后停在表格上方，右侧带「关闭」。
///
/// 与 `alert` 的差别不是颜色而是**位置与寿命**：它在内容流里（不是浮层），
/// 有明确的关闭入口，且不属于任何一张卡片——用户还没点之前它是空的。
pub fn error_bar<O>(
    title: impl IntoReactive<SharedString> + 'static,
    detail: impl IntoReactive<SharedString> + 'static,
    on_close: O,
) -> impl IntoElement
where
    O: Fn() + Copy + 'static,
{
    view()
        .w_full()
        .mx(px(16.0))
        .my(px(8.0))
        .flex_row()
        .rounded(px(m::RADIUS))
        .border_1()
        .border_color(|| t::control_stroke())
        .bg(|| t::control_fill())
        .overflow_hidden()
        .child(view().w(px(m::BAR_W)).h_full().bg(|| t::critical()))
        .child(
            view()
                .flex_1()
                .flex_row()
                .items_center()
                .p(px(10.0))
                .child(
                    view()
                        .flex_1()
                        .flex_col()
                        .child(
                            text(title)
                                .text_size(px(f::CAPTION))
                                .font_family(f::FONT_TEXT)
                                .text_color(|| t::critical()),
                        )
                        .child(vspacer(2.0))
                        .child(
                            text(detail)
                                .text_size(px(f::CAPTION))
                                .font_family(f::FONT_MONO)
                                .text_color(|| t::ink_secondary()),
                        ),
                )
                .child(btn_text("关闭", t::ink_tertiary, on_close)),
        )
}

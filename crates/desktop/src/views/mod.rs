//! 各屏视图。一屏一模块文件，共享的展示词汇在 `crate::display`。

pub mod ca;
pub mod catalog;
pub mod debug;
pub mod detail;
pub mod environments;
pub mod settings;

pub use ca::CaScreen;
pub use catalog::{ProxiesCatalog, RulesCatalog};
pub use debug::DebugScreen;
pub use detail::Detail;
pub use environments::Environments;
pub use settings::Settings;

use gpui_kit::component::ActiveTheme;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::{
    AnyElement, AnyView, App, FontWeight, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, Styled, Window, div,
};

use crate::errors::Notice;

/// 截断文本的悬浮提示：把被裁掉的那部分还给用户。
///
/// `Button` 自带 `tooltip(text)`，普通 `div` 只有 gpui 的
/// `tooltip(|window, cx| AnyView)` —— 这里统一出同一个闭包形状，
/// 免得每个表格单元格各写一遍 `Tooltip::new(…).build(…)`。
pub fn text_tooltip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |window, cx| Tooltip::new(text.clone()).build(window, cx)
}

/// 错误条：一条带图标的 `Alert`（而不是一行红字）。
///
/// 错误要能看见也要能读懂：图标给出"这是错误"的形状，表面与边框把它从正文里
/// 抬出来。文案分两行，分工见 [`crate::errors`]：标题是中文主句
/// （`启动 dev 失败：端口已被其他程序占用`），正文是契约错误码 + 服务端英文原文
/// （`[port_conflict] port 8080 already in use`）。**两行都要有**：
/// 只给中文会丢掉"到底是哪个端口"这类唯一信息，只给英文则是走查里那条 F10。
pub fn error_banner(id: &'static str, notice: Notice) -> AnyElement {
    let alert = match notice.detail_text() {
        Some(detail) => gpui_kit::component::alert::Alert::error(id, detail).title(notice.summary),
        // 没有原文的说明（界面自己判出来的）只有一句中文：放在正文位而不是标题位 ——
        // 标题是 `truncate` 的单行，长句子会被截断。
        None => gpui_kit::component::alert::Alert::error(id, notice.summary),
    };
    alert.small().into_any_element()
}

/// 空态：标题 + 下一步说明，居中一列。
///
/// 为什么不用组件库的 `Empty`：它自带虚线外框与内边距，是给**页面主体**用的；
/// 表格的空态在表格框内，再套一层框就成了框中框。形态与 `Empty` 一致
/// （图标位省略、标题 + 说明），但不带外框。
pub fn empty_state(cx: &App, id: &'static str, title: &str, description: &str) -> AnyElement {
    empty_state_with_action(cx, id, title, description, None)
}

/// 带下一步动作的空态。
///
/// 纯文字的空态只能"指路"（"点右上角的按钮"），而那个按钮往往在视线之外；
/// 把同一个动作就地放一份，用户不必先找到它再回来。动作由调用方给 ——
/// 空态的下一步是什么，只有那一屏知道。
pub fn empty_state_with_action(
    cx: &App,
    id: &'static str,
    title: &str,
    description: &str,
    action: Option<AnyElement>,
) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    div()
        .id(id)
        .flex()
        .flex_col()
        .size_full()
        .items_center()
        .justify_center()
        .gap_2()
        .p_6()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(SharedString::from(title.to_string())),
        )
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .child(SharedString::from(description.to_string())),
        )
        .children(action)
        .into_any_element()
}

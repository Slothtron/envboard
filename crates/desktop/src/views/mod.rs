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

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::ActiveTheme;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::{
    AnyElement, AnyView, App, Context, FontWeight, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, Styled, TestSupportExt as _, Window, div,
};

use crate::errors::{Action, Notice};

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

/// 模态弹窗里的校验原因。
///
/// 弹窗是模态的，表格上方那条错误条会被它挡住；校验失败又必须让弹窗**开着**，
/// 所以原因只能写在弹窗内部。状态用 `Rc<RefCell<…>>` 是因为弹窗内容闭包与
/// `on_ok` 回调各持一份，且 `on_ok` 里要写完原因再让弹窗层重画一帧。
///
/// 一个应用里同类表单只有一种"点了没反应"是不可接受的：新建环境最早用了这套
/// 写法（见 `views/environments.rs` 的注释），这里抽出来给规则导入、上游代理
/// 表单、按域名签发共用。
#[derive(Clone, Default)]
pub struct DialogProblem(Rc<RefCell<Option<SharedString>>>);

impl DialogProblem {
    /// 校验不通过：记下原因、让弹窗层重画一帧、`on_ok` 返回 `false`（不关闭）。
    pub fn reject(&self, window: &mut Window, reason: impl Into<SharedString>) -> bool {
        *self.0.borrow_mut() = Some(reason.into());
        window.refresh();
        false
    }

    /// 校验通过：清掉上一次的原因，`on_ok` 返回 `true`（关闭弹窗）。
    pub fn accept(&self) -> bool {
        *self.0.borrow_mut() = None;
        true
    }

    /// 渲染错误行（只在有问题时出现）。要挂在弹窗的 `children()` 上。
    pub fn render(&self, id: &'static str, cx: &App) -> Option<AnyElement> {
        let message = self.0.borrow().clone()?;
        Some(
            div()
                .id(id)
                .test_support()
                .text_xs()
                .text_color(cx.theme().danger)
                .child(message)
                .into_any_element(),
        )
    }
}

/// 写操作成功的轻提示（`已启动 dev`）。
///
/// 成功让用户看见"动作发生了"，失败仍走各屏的错误条 —— 失败要留住，成功可以
/// 自己消失。所有调用点共用同一个 id：连续操作时后一条替换前一条，不会堆一列
/// 过期的"已启动"。
pub fn push_success_toast<V: 'static>(
    action: Action,
    subject: Option<&str>,
    window: &mut Window,
    cx: &mut Context<V>,
    id: &'static str,
) {
    let Some(text) = action.success(subject) else {
        return;
    };
    window.push_notification(Notification::success(text).id1::<V>(id), cx);
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

#[cfg(test)]
mod tests {
    use super::DialogProblem;

    /// `accept()` 必须清掉上一次拒绝留下的原因：同一个弹窗二次提交时，
    /// 不能让旧错误继续挂在表单上。
    #[test]
    fn accepting_a_dialog_problem_clears_the_previous_reason() {
        let problem = DialogProblem::default();
        assert!(problem.0.borrow().is_none(), "默认没有原因");
        // reject 需要 Window 做 refresh，这里直接写内部状态验证 accept 的收尾。
        *problem.0.borrow_mut() = Some("名字不合法".into());
        assert!(problem.accept(), "accept 返回 true 表示可以关闭弹窗");
        assert!(problem.0.borrow().is_none(), "接受后原因必须清空");
    }
}

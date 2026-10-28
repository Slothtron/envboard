//! 根视图：标题栏 + 侧栏导航 + 内容区 + 状态栏。
//!
//! 骨架件全部取 `gpui_kit::component` 的现成组件（`TitleBar` / `Sidebar` /
//! `StatusBar` / `Switch`），本文件只做「目的地切换 + 挂各屏视图」这一层。
//!
//! 三条已核对的纪律：
//!
//! * 标题栏整体是系统拖拽区，里面的控件必须自己吞掉"按下"事件（坑 #1）；
//! * 必须先 `gpui_kit::init` 再创建组件视图（见 `main`）；
//! * 各段渲染先装箱成 `AnyElement` 再拼树，否则 `&mut self` / `&mut cx` 的借用
//!   会一路拖到整棵树拼完而撞上借用检查。

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, TestSupportExt as _, Window, div,
};

use gpui_kit::component::{
    ActiveTheme, Icon, IconName, Sizable, Theme, ThemeMode, TitleBar,
    sidebar::{Sidebar, SidebarMenu, SidebarMenuItem},
    status_bar::StatusBar,
    switch::Switch,
};

use crate::host::Host;
use crate::views::Environments;

/// 侧栏目的地。P0 落环境与详情面板，规则/代理在 P0.8 补，其余屏属于 P2。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    Environments,
    Rules,
    Proxies,
}

impl Destination {
    /// 全部目的地（侧栏按这个顺序画，状态栏的"当前屏"也从这里取标签）。
    const ALL: [Self; 3] = [Self::Environments, Self::Rules, Self::Proxies];

    pub fn label(self) -> SharedString {
        match self {
            Self::Environments => "环境".into(),
            Self::Rules => "规则".into(),
            Self::Proxies => "代理".into(),
        }
    }

    fn icon(self) -> Icon {
        let name = match self {
            // 三个图形都在组件默认图标集里（不在集合内的名字会静默画空白）。
            Self::Environments => IconName::LayoutDashboard,
            Self::Rules => IconName::FileText,
            Self::Proxies => IconName::Network,
        };
        Icon::new(name)
    }

    /// 这一屏是否已经落地（未落地的屏给出诚实的占位说明，不画假界面）。
    pub fn is_implemented(self) -> bool {
        matches!(self, Self::Environments)
    }
}

/// 工作台根视图。
pub struct Workbench {
    destination: Destination,
    environments: Entity<Environments>,
}

impl Workbench {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let environments = cx.new(|cx| Environments::new(window, cx));
        let this = Self {
            destination: Destination::Environments,
            environments,
        };
        // 首帧之后再挂轮询器：构造期不启动后台任务（骨架阶段也避开上下文未就位）。
        let environments = this.environments.clone();
        cx.on_next_frame(window, move |_, window, cx| {
            environments.update(cx, |view, cx| view.attach(window, cx));
        });
        this
    }

    /// 标题栏整体是拖拽区：控件在"按下"时就阻止冒泡，点击才能完成。
    fn render_title_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let is_dark = cx.theme().is_dark();
        let title = SharedString::from("envboard 工作台");

        TitleBar::new().child(
            div()
                .h_full()
                .flex()
                .items_center()
                .gap_2()
                .flex_1()
                .min_w_0()
                .pr_2()
                .child(div().text_sm().font_weight(FontWeight::BOLD).child(title))
                .child(
                    div()
                        .h_full()
                        .flex()
                        .items_center()
                        .on_mouse_down(MouseButton::Left, Self::intercept_title_bar_press)
                        .child(
                            Switch::new("dark-mode")
                                .small()
                                .checked(is_dark)
                                .label("深色")
                                .on_change(|checked, window, cx| {
                                    let mode = if *checked {
                                        ThemeMode::Dark
                                    } else {
                                        ThemeMode::Light
                                    };
                                    Theme::change(mode, Some(window), cx);
                                }),
                        ),
                ),
        )
    }

    fn intercept_title_bar_press(_: &MouseDownEvent, _: &mut Window, cx: &mut App) {
        cx.stop_propagation();
    }

    fn render_navigation(&self, cx: &mut Context<Self>) -> impl IntoElement {
        Sidebar::new("envboard-navigation").child(SidebarMenu::new().children(
            Destination::ALL.into_iter().map(|destination| {
                SidebarMenuItem::new(destination.label())
                    .icon(destination.icon())
                    .active(self.destination == destination)
                    .on_click(cx.listener(move |this, _, _, _| {
                        this.destination = destination;
                    }))
            }),
        ))
    }

    /// 内容区：环境屏是真视图，其余屏在落地前只显示诚实的占位说明。
    fn render_content(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.destination.is_implemented() {
            let heading = self.destination.label();
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .id("envboard-heading")
                        .test_support()
                        .aria_label(heading.clone())
                        .text_base()
                        .font_weight(FontWeight::BOLD)
                        .child(heading),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("这一屏属于 P0.8，还没落地 —— 这里不画假界面。"),
                )
                .into_any_element();
        }

        // 装箱必须发生在闭包里：`render` 返回的是带生命周期的 opaque 类型，
        // 让它逃出 `update` 就撞上借用检查（与 dipswitch 外壳同一手法）。
        self.environments
            .update(cx, |view, cx| view.render(window, cx).into_any_element())
    }

    fn render_status_bar(&self, cx: &App) -> AnyElement {
        let host = cx.global::<Host>();
        let warnings = host.warnings().len();
        let warning_note: SharedString = if warnings == 0 {
            "".into()
        } else {
            format!("{warnings} 条装配警告").into()
        };
        let (min, max) = host.port_range();
        let left: SharedString =
            format!("{} · 端口区间 {min}-{max}", host.state_dir().display()).into();
        let right = self.destination.label();

        StatusBar::new()
            .left(
                div()
                    .text_xs()
                    .truncate()
                    .text_color(cx.theme().muted_foreground)
                    .child(left),
            )
            .right(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(warning_note)
                    .child(right),
            )
            .into_any_element()
    }
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title_bar = self.render_title_bar(cx).into_any_element();
        let navigation = self.render_navigation(cx).into_any_element();
        let content = self.render_content(window, cx).into_any_element();
        let status_bar = self.render_status_bar(cx).into_any_element();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(title_bar)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(navigation)
                    .child(content),
            )
            .child(status_bar)
    }
}

#[cfg(test)]
mod tests {
    use super::Destination;

    #[test]
    fn only_shipped_screens_claim_to_be_implemented() {
        // 占位说明必须诚实：未落地的屏不许声称自己可用（P0.8 落地时逐个翻 true）。
        assert!(Destination::Environments.is_implemented());
        assert!(!Destination::Rules.is_implemented());
        assert!(!Destination::Proxies.is_implemented());
    }
}

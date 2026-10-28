//! 根视图：标题栏 + 侧栏导航 + 内容区 + 状态栏。
//!
//! 骨架件全部取 `gpui_kit::component` 的现成组件（`TitleBar` / `Sidebar` /
//! `StatusBar` / `Switch`），本文件只做「目的地切换 + 把宿主信息摆出来」这一层。
//!
//! 两条已核对的 Windows 纪律：
//!
//! * 标题栏整体是系统的拖拽区，放在里面的控件必须自己吞掉"按下"事件，
//!   否则只能收到按下、收不到抬起，点击永远不触发；
//! * 必须先 `gpui_kit::init`，再创建任何组件视图（见 `main`）。

use gpui_kit::{
    App, Context, FontWeight, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, TestSupportExt as _,
    Window, div,
};

use gpui_kit::component::{
    ActiveTheme, Icon, IconName, Sizable, Theme, ThemeMode, TitleBar,
    sidebar::{Sidebar, SidebarMenu, SidebarMenuItem},
    status_bar::StatusBar,
    switch::Switch,
};

/// 侧栏目的地。P0 只落环境一屏，规则/代理在 P0.8 补，其余屏属于 P2。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    Environments,
    Rules,
    Proxies,
}

impl Destination {
    /// 全部目的地（侧栏按这个顺序画，状态栏的"当前屏"也从这里取标签）。
    const ALL: [Self; 3] = [Self::Environments, Self::Rules, Self::Proxies];

    fn label(self) -> SharedString {
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
    fn is_implemented(self) -> bool {
        matches!(self, Self::Environments)
    }
}

/// 组合根交给界面的宿主信息。
#[derive(Debug, Clone)]
pub struct HostInfo {
    /// 状态目录（账本、规则库、共享 CA 都在它下面）。
    pub state_dir: SharedString,
    /// 随机分配端口的区间，形如 `16000-16999`。
    pub port_range: SharedString,
    /// 装配期的响亮警告（例如共享 CA 被重新物化：客户端要重装）。
    pub warnings: Vec<String>,
}

/// 工作台根视图。
pub struct Workbench {
    destination: Destination,
    host: HostInfo,
}

impl Workbench {
    pub fn new(_: &mut Window, _: &mut Context<Self>, host: HostInfo) -> Self {
        Self {
            destination: Destination::Environments,
            host,
        }
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

    /// 内容区：P0.2 只交付骨架，环境屏的数据与交互从 P0.3 起逐块进来。
    fn render_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let heading = self.destination.label();
        let body = if self.destination.is_implemented() {
            "取数与列表在 P0.3 接入（组合根已就位，界面尚未连上它）。"
        } else {
            "这一屏属于 P0.8，还没落地 —— 这里不画假界面。"
        };

        div()
            // 滚动属于交互态能力：容器要有 ElementId 才能挂 `overflow_*_scroll`。
            .id("envboard-content")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap_3()
            .p_4()
            .overflow_y_scroll()
            .child(
                div()
                    // 注册成可发现的测试目标：无头 UI 测试按这个 id 断言"当前是哪一屏"
                    // （本机被 OS 拒了输入注入，交互验收只能走框架内事件注入）。
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
                    .child(body),
            )
    }

    fn render_status_bar(&self, cx: &App) -> impl IntoElement {
        let warnings = self.host.warnings.len();
        let warning_note: SharedString = if warnings == 0 {
            "".into()
        } else {
            format!("{warnings} 条装配警告").into()
        };
        let left: SharedString = format!(
            "{} · 端口区间 {}",
            self.host.state_dir, self.host.port_range
        )
        .into();
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
    }
}

impl Render for Workbench {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 各段取好即装箱成 AnyElement：`impl IntoElement` 的返回类型会把 `&mut self`
        // 与 `&mut cx` 的借用一直拖到整棵树拼完，四段并排放就撞借用检查
        // （装箱切断借用，与 dipswitch 外壳同一手法）。
        let title_bar = self.render_title_bar(cx).into_any_element();
        let navigation = self.render_navigation(cx).into_any_element();
        let content = self.render_content(cx).into_any_element();
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
        // 占位说明必须诚实：未落地的屏不许声称自己可用（P0.3/P0.8 落地时逐个翻 true）。
        assert!(Destination::Environments.is_implemented());
        assert!(!Destination::Rules.is_implemented());
        assert!(!Destination::Proxies.is_implemented());
    }
}

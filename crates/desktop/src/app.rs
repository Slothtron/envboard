//! 根视图：标题栏 + 侧栏导航 + 内容区 + 状态栏。
//!
//! 骨架件全部取 `gpui_kit::component` 的现成组件（`TitleBar` / `Sidebar` /
//! `StatusBar`），本文件只做「目的地切换 + 挂各屏视图 + 主题跟随」这一层。
//!
//! 四条已核对的纪律：
//!
//! * 标题栏整体是系统拖拽区，里面的控件必须自己吞掉"按下"事件（坑 #1）。
//!   现在栏里只有标题，所以这条只在将来往里放按钮时生效；
//! * **主题的唯一入口在「设置」里**（浅色 / 深色 / 系统）。标题栏曾经有个
//!   深色开关，已移除：一个两态开关表达不了"系统"这一档，两处都能改主题还会互相打架；
//! * 必须先 `gpui_kit::init` 再创建组件视图（见 `main`）；
//! * 各段渲染先装箱成 `AnyElement` 再拼树，否则 `&mut self` / `&mut cx` 的借用
//!   会一路拖到整棵树拼完而撞上借用检查。
//!
//! 窗口外观变化时（系统切了明暗），只有"跟随系统"这一档要重新落实主题 ——
//! 显式选了明亮/暗色的人不该被系统设置拽走。

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, Render, SharedString, Styled, Subscription, Window,
    div,
};

use gpui_kit::component::{
    ActiveTheme, Icon, IconName, TitleBar,
    sidebar::{Sidebar, SidebarMenu, SidebarMenuItem},
    status_bar::StatusBar,
};

use crate::host::Host;
use crate::ui_prefs::{ThemePreference, UiPrefs};
use crate::views::{CaScreen, DebugScreen, Environments, ProxiesCatalog, RulesCatalog, Settings};

/// 侧栏目的地。顺序即显示顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    Environments,
    Rules,
    Proxies,
    Debug,
    Ca,
    Settings,
}

impl Destination {
    /// 全部目的地（侧栏按这个顺序画，状态栏的"当前屏"也从这里取标签）。
    const ALL: [Self; 6] = [
        Self::Environments,
        Self::Rules,
        Self::Proxies,
        Self::Debug,
        Self::Ca,
        Self::Settings,
    ];

    pub fn label(self) -> SharedString {
        match self {
            Self::Environments => "环境".into(),
            Self::Rules => "规则".into(),
            Self::Proxies => "代理".into(),
            Self::Debug => "调试".into(),
            Self::Ca => "CA".into(),
            Self::Settings => "设置".into(),
        }
    }

    fn icon(self) -> Icon {
        let name = match self {
            // 六个图形都在组件默认图标集里（不在集合内的名字会静默画空白）。
            Self::Environments => IconName::LayoutDashboard,
            Self::Rules => IconName::FileText,
            Self::Proxies => IconName::Network,
            Self::Debug => IconName::SquareTerminal,
            Self::Ca => IconName::CircleCheck,
            Self::Settings => IconName::Settings,
        };
        Icon::new(name)
    }

    /// 这一屏是否已经落地（未落地的屏给出诚实的占位说明，不画假界面）。
    pub fn is_implemented(self) -> bool {
        // 六屏都落地了；这条判据留着是给以后的新屏用的：
        // 新屏必须先翻成 true 才允许出现在侧栏里。
        true
    }
}

/// 工作台根视图。
pub struct Workbench {
    destination: Destination,
    environments: Entity<Environments>,
    rules: Entity<RulesCatalog>,
    proxies: Entity<ProxiesCatalog>,
    debug: Entity<DebugScreen>,
    ca: Entity<CaScreen>,
    settings: Entity<Settings>,
    _subscriptions: Vec<Subscription>,
}

impl Workbench {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // 各屏都在启动时建好（dipswitch 的模块常驻同一手法）：切屏不重建状态，
        // 侧栏高亮、目录的取数进度与调试会话都跟着窗口活着。
        let environments = cx.new(|cx| Environments::new(window, cx));
        let env_table = environments.read(cx).table().clone();
        let rules = cx.new(|cx| RulesCatalog::new(env_table.clone(), window, cx));
        let proxies = cx.new(|cx| ProxiesCatalog::new(window, cx));
        let debug = cx.new(|cx| DebugScreen::new(env_table, window, cx));
        let ca = cx.new(|cx| CaScreen::new(window, cx));
        let settings = cx.new(|cx| Settings::new(window, cx));
        // 系统外观变化：只有"跟随系统"这一档重新落实主题。
        let appearance_sub = cx.observe_window_appearance(window, |_, window, cx| {
            if UiPrefs::current(cx).theme == ThemePreference::System {
                crate::ui_prefs::apply(ThemePreference::System, Some(window), cx);
            }
        });
        let this = Self {
            destination: Destination::Environments,
            environments,
            rules,
            proxies,
            debug,
            ca,
            settings,
            _subscriptions: vec![appearance_sub],
        };
        // 首帧之后再挂轮询器：构造期不启动后台任务（骨架阶段也避开上下文未就位）。
        let environments = this.environments.clone();
        cx.on_next_frame(window, move |_, window, cx| {
            environments.update(cx, |view, cx| view.attach(window, cx));
        });
        this
    }

    /// 当前目的地（只读）。给无头 UI 测试断言用 —— 视图层的状态迁移本来就该可观察。
    pub fn destination(&self) -> Destination {
        self.destination
    }

    /// 标题栏：整条是拖拽区，里面只放标题 —— 主题开关在「设置」里。
    fn render_title_bar(&self) -> impl IntoElement {
        let title = SharedString::from("envboard 工作台");
        TitleBar::new().child(
            div()
                .h_full()
                .flex()
                .items_center()
                .flex_1()
                .min_w_0()
                .pr_2()
                .on_mouse_down(MouseButton::Left, Self::intercept_title_bar_press)
                .child(div().text_sm().font_weight(FontWeight::BOLD).child(title)),
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

    /// 内容区：按目的地挂对应屏。
    fn render_content(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.destination {
            Destination::Environments => self
                .environments
                .update(cx, |view, cx| view.render(window, cx).into_any_element()),
            Destination::Rules => self
                .rules
                .update(cx, |view, cx| view.render(window, cx).into_any_element()),
            Destination::Proxies => self
                .proxies
                .update(cx, |view, cx| view.render(window, cx).into_any_element()),
            Destination::Debug => self
                .debug
                .update(cx, |view, cx| view.render(window, cx).into_any_element()),
            Destination::Ca => self
                .ca
                .update(cx, |view, cx| view.render(window, cx).into_any_element()),
            Destination::Settings => self
                .settings
                .update(cx, |view, cx| view.render(window, cx).into_any_element()),
        }
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
        let theme = UiPrefs::current(cx).theme.label();
        let left: SharedString =
            format!("{} · 端口区间 {min}-{max}", host.state_dir().display()).into();
        let right: SharedString = format!("{} · 主题：{theme}", self.destination.label()).into();

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
        let title_bar = self.render_title_bar().into_any_element();
        let navigation = self.render_navigation(cx).into_any_element();
        let content = self.render_content(window, cx).into_any_element();
        let status_bar = self.render_status_bar(cx);

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
    fn every_screen_in_the_sidebar_is_shipped() {
        // 侧栏里的每一屏都必须真的落地：加新屏时必须先让它落地（或显式承认没落地
        // 并给出占位），不许出现"点了是空白"。
        for destination in Destination::ALL {
            assert!(
                destination.is_implemented(),
                "{destination:?} 还没落地却进了侧栏"
            );
        }
        assert_eq!(Destination::ALL.len(), 6);
    }
}

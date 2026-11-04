//! 根视图：标题栏 + 侧栏导航 + 内容区 + 状态栏。
//!
//! 骨架件全部取 `gpui_kit::component` 的现成组件（`TitleBar` / `Sidebar` /
//! `StatusBar`），本文件只做「目的地切换 + 挂各屏视图 + 主题跟随」这一层。
//!
//! 四条已核对的纪律：
//!
//! * 标题栏整体是系统拖拽区（`HTCAPTION`），**栏内的纯展示元素不能吞"按下"事件**：
//!   `WM_NCLBUTTONDOWN` 先给应用的处理器，被标记"已处理"就等于取消窗口拖动
//!   （gpui-pre-windows 的 `handle_nc_mouse_down_msg`）。反过来，将来往栏里放**可点控件**
//!   时必须让控件自己吞掉按下 —— 否则系统会开始拖窗口，控件只能收到按下、收不到抬起；
//!   现在栏里只有标题，所以第一半是当前纪律、第二半留给以后；
//! * **主题的唯一入口在「设置」里**（浅色 / 深色 / 系统）。标题栏曾经有个
//!   深色开关，已移除：一个两态开关表达不了"系统"这一档，两处都能改主题还会互相打架；
//! * 必须先 `gpui_kit::init` 再创建组件视图（见 `main`）；
//! * 各段渲染先装箱成 `AnyElement` 再拼树，否则 `&mut self` / `&mut cx` 的借用
//!   会一路拖到整棵树拼完而撞上借用检查。
//!
//! 窗口外观变化时（系统切了明暗），只有"跟随系统"这一档要重新落实主题 ——
//! 显式选了明亮/暗色的人不该被系统设置拽走。

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement, Render, Role, SharedString,
    StatefulInteractiveElement as _, Styled, Subscription, WeakEntity, Window, div,
};

use gpui_kit::component::{
    ActiveTheme, Icon, IconName, TitleBar,
    sidebar::{Sidebar, SidebarMenu, SidebarMenuItem},
    status_bar::StatusBar,
};
use gpui_kit::prelude::FluentBuilder as _;

use crate::host::Host;
use crate::ui_prefs::{ThemePreference, UiPrefs};
use crate::views::environments::{Filter, Overview};
use crate::views::text_tooltip;
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
    pub const ALL: [Self; 6] = [
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
    /// 状态栏画的上一次聚合（用于把重绘限制在"数字真的变了"时，见 `new`）。
    overview: Overview,
    /// 根视图的焦点：开窗后默认落在它上面。
    ///
    /// 没有它，**没有任何焦点**时键盘事件根本没有接收者（F6 也就无从谈起）；
    /// 它同时是 Esc 离开状态栏后的落点。不进 Tab 序列（`tab_stop(false)`）。
    root_focus: FocusHandle,
    /// 状态栏的键盘焦点：整条状态栏是**一个** Tab 落点，←/→ 在聚合项之间走查。
    status_focus: FocusHandle,
    /// 状态栏里键盘高亮的那一项（`STATUS_NAV` 的下标）。
    status_active: usize,
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
        // 状态栏的聚合与环境屏读**同一份快照**：只在数字变了时重画根视图。
        // 环境屏每拍都会 notify（哪怕快照没变），无脑跟着重画会把当前屏
        // （例如 CA 屏的信任探测）也拖进每秒一拍。
        let overview = environments.read(cx).table().read(cx).delegate().overview();
        let overview_sub = cx.observe(&environments, |this, envs, cx| {
            let overview = envs.read(cx).table().read(cx).delegate().overview();
            if this.overview != overview {
                this.overview = overview;
                cx.notify();
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
            overview,
            root_focus: cx.focus_handle().tab_stop(false),
            status_focus: cx.focus_handle().tab_stop(true),
            status_active: 0,
            _subscriptions: vec![appearance_sub, overview_sub],
        };
        // 首帧之后再挂轮询器：构造期不启动后台任务（骨架阶段也避开上下文未就位）。
        let environments = this.environments.clone();
        cx.on_next_frame(window, move |this, window, cx| {
            // 首帧先给键盘一个落点，再挂轮询器（轮询器动的是另一个实体）。
            this.root_focus.focus(window, cx);
            environments.update(cx, |view, cx| view.attach(window, cx));
        });
        this
    }

    /// 当前目的地（只读）。给无头 UI 测试断言用 —— 视图层的状态迁移本来就该可观察。
    pub fn destination(&self) -> Destination {
        self.destination
    }

    /// 环境屏实体（只读）。给无头 UI 测试断言用 —— 过滤档这类状态就该可观察。
    pub fn environments(&self) -> &Entity<Environments> {
        &self.environments
    }

    /// 状态栏当前键盘高亮的下标（只读）。给无头测试断言 ←/→ 的落点。
    pub fn status_active(&self) -> usize {
        self.status_active
    }

    /// 把焦点交给状态栏。真实入口是 F6；无头上下文不跑首帧（根视图拿不到初始
    /// 焦点），所以测试直接落焦点，走的仍是同一条 `status_focus`。
    pub fn focus_status(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.status_focus.focus(window, cx);
        cx.notify();
    }

    /// 标题栏：整条是拖拽区，里面只放标题 —— 主题开关在「设置」里。
    ///
    /// **这里不要挂 `on_mouse_down` + `stop_propagation`**。标题栏的整条内容区带着
    /// `WindowControlArea::Drag`，Windows 命中的是 `HTCAPTION`；而
    /// `WM_NCLBUTTONDOWN` 会**先**交给应用的事件处理器，一旦被标记为"已处理"，
    /// gpui-pre-windows 就直接吞掉这条消息、`DefWindowProc` 拿不到它，系统拖动
    /// 也就不会开始 —— 症状正是"按住标题栏拖不动窗口"。
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
                .child(div().text_sm().font_weight(FontWeight::BOLD).child(title)),
        )
    }

    fn render_navigation(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let host = cx.global::<Host>();
        Sidebar::new("envboard-navigation")
            .child(
                SidebarMenu::new().children(Destination::ALL.into_iter().map(|destination| {
                    SidebarMenuItem::new(destination.label())
                        .icon(destination.icon())
                        .active(self.destination == destination)
                        .on_click(cx.listener(move |this, _, _, _| {
                            this.destination = destination;
                        }))
                })),
            )
            // 侧栏底部：端口区间。状态目录留在状态栏（那儿是横向宽的地方，
            // 路径才读得全；侧栏这一栏窄，路径只能截断 —— 同一件事只说一处）。
            //
            // 用普通 `div` 而不是组件库的 `SidebarFooter`：那个组件带悬停底色
            // （它是为可点的页脚做的），纯说明文字不该有"可以点"的暗示。
            .footer(
                div().flex().flex_col().gap_1().child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(format!(
                            "端口区间 {}-{}",
                            host.port_range().0,
                            host.port_range().1
                        ))),
                ),
            )
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

    /// 状态栏聚合项点下去去哪：都是**同一份导航**（改目的地 / 切过滤档），
    /// 不复制环境屏内部的状态。
    fn navigate(&mut self, nav: StatusNav, cx: &mut Context<Self>) {
        match nav {
            StatusNav::Filter(filter) => {
                self.destination = Destination::Environments;
                self.environments
                    .update(cx, |view, cx| view.apply_filter(filter, cx));
            }
            StatusNav::Screen(destination) => self.destination = destination,
        }
        cx.notify();
    }

    fn render_status_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let host = cx.global::<Host>();
        let warnings = host.warnings().len();
        let theme = cx.theme();
        let focused = self.status_focus.is_focused(window);
        // 左段是**环境聚合**：与环境屏读同一份快照（`overview` 在观察回调里
        // 更新），所以状态栏上的数字与列表、过滤档永远一致。状态目录不再占
        // 这一格 —— 它是排障时才要的信息，而错误条本来就会点名路径。
        //
        // 整条聚合是**一个** Tab 落点（工具栏模型）：Tab 到达后用 ←/→ 在
        // 六项之间走查、Enter 触发与点击相同的导航、Esc 退出。没有这层，
        // 键盘用户根本到不了这里（每一项都只是可点 div）。
        let left = div()
            .id("status-aggregate")
            .flex()
            .items_center()
            .px_1()
            .rounded(theme.radius)
            .track_focus(&self.status_focus)
            .tab_group()
            .role(Role::Toolbar)
            .aria_label("状态栏：环境聚合")
            // 焦点环常驻一层边框（透明/实色切换）：只在聚焦时加边框会让
            // 整条状态栏左右跳 1px。
            .border_1()
            .border_color(if focused {
                theme.border
            } else {
                theme.background
            })
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.on_status_key(event, window, cx)
            }))
            .child(overview_segments(
                self.overview,
                focused.then_some(self.status_active),
                cx.weak_entity(),
                cx,
            ));
        // 右段只说「当前在哪一屏」。主题不在这里重复：它是设置页里改的东西，
        // 而且状态栏再来一句"主题：浅色"只占地方。
        let right: SharedString = self.destination.label();

        // 装配警告是"这一份宿主跑得不对"的信息：它必须**看得见**。
        // 原来把它渲染成一段空字符串（警告数为 0 时也占位），于是警告真的
        // 出现时状态栏右侧只是多了一段没有形状的文字。
        let warning_note: AnyElement = if warnings == 0 {
            div().into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .gap_1()
                .text_color(cx.theme().warning)
                .child(Icon::new(IconName::TriangleAlert).size_3())
                .child(SharedString::from(format!("{warnings} 条装配警告")))
                .into_any_element()
        };

        StatusBar::new()
            .left(left)
            .right(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(warning_note)
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(right),
                    ),
            )
            .into_any_element()
    }

    /// 状态栏获得焦点后的键位：←/→ 换项、Enter/Space 触发、Esc 退出。
    ///
    /// 不用 Action + keymap：整条状态栏只有这一组键，且必须**只在自己
    /// 有焦点时**生效 —— `on_key_down` 天然满足这一点，动作绑定反而要
    /// 额外维护 context。
    fn on_status_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let last = STATUS_NAV.len() - 1;
        match event.keystroke.key.as_str() {
            "left" => {
                self.status_active = self.status_active.saturating_sub(1);
                cx.notify();
                cx.stop_propagation();
            }
            "right" => {
                self.status_active = (self.status_active + 1).min(last);
                cx.notify();
                cx.stop_propagation();
            }
            "enter" | "space" => {
                let nav = STATUS_NAV[self.status_active.min(last)];
                self.navigate(nav, cx);
                cx.stop_propagation();
            }
            "escape" => {
                // 回到根焦点而不是 blur：blur 之后整窗没有键盘落点，
                // F6 之外什么都按不动；根焦点是"离开某一块"的稳定归宿。
                self.root_focus.focus(window, cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }
}

/// 状态栏聚合项点下去做什么。
#[derive(Debug, Clone, Copy)]
enum StatusNav {
    /// 进环境屏并切到某一档（数字与那一档的行数是同一判据）。
    Filter(Filter),
    /// 切到某一屏（抓包 → 调试、规则 → 规则屏、证书 → CA …）。
    Screen(Destination),
}

/// 六项聚合的导航目标，**顺序即显示顺序**（键盘 ←/→ 与鼠标点击共用）。
const STATUS_NAV: [StatusNav; 6] = [
    StatusNav::Filter(Filter::Running),
    StatusNav::Filter(Filter::Attention),
    StatusNav::Screen(Destination::Debug),
    StatusNav::Screen(Destination::Rules),
    StatusNav::Screen(Destination::Proxies),
    StatusNav::Screen(Destination::Ca),
];

/// 状态栏左段：环境聚合（运行中/总数、需处理、抓包、规则、上游、证书）。
///
/// 每一项都是入口：数字说了"有多少"，点下去就把你带到决定它的那一屏/那一档。
/// `active` 是键盘高亮的下标（`None` = 状态栏没有焦点）。
/// 「需处理」「证书未安装」用警告色：状态栏是常驻的，这两件事是**现在就该
/// 看一眼**的；其余计数保持次要色 —— 一条状态栏全是颜色，就没有重点了。
fn overview_segments(
    overview: Overview,
    active: Option<usize>,
    view: WeakEntity<Workbench>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let (certificate, certificate_tone) = match cx.global::<Host>().ca() {
        Some(control) if control.trust().is_installed() => ("已信任".to_string(), muted),
        Some(_) => ("未安装".to_string(), theme.warning),
        None => ("未知".to_string(), muted),
    };
    let attention_tone = if overview.attention > 0 {
        theme.warning
    } else {
        muted
    };
    let dot = || div().text_color(muted).child("·").into_any_element();

    div()
        .flex()
        .items_center()
        .gap_2()
        .text_xs()
        .child(status_item(
            "status-running",
            format!("运行中 {} / {}", overview.running, overview.total),
            muted,
            "运行中 / 环境总数；点开环境屏的「运行中」档",
            StatusNav::Filter(Filter::Running),
            active == Some(0),
            view.clone(),
            cx,
        ))
        .child(dot())
        .child(status_item(
            "status-attention",
            format!("需处理 {}", overview.attention),
            attention_tone,
            "失败类健康或期望与实际不一致的环境；点开「需处理」档",
            StatusNav::Filter(Filter::Attention),
            active == Some(1),
            view.clone(),
            cx,
        ))
        .child(dot())
        .child(status_item(
            "status-capture",
            format!("抓包 {} / {}", overview.capture, overview.total),
            muted,
            "开着抓包的环境数 / 总数；点进调试屏",
            StatusNav::Screen(Destination::Debug),
            active == Some(2),
            view.clone(),
            cx,
        ))
        .child(dot())
        .child(status_item(
            "status-rules",
            format!("规则 {} / {}", overview.rules_bound, overview.total),
            muted,
            "绑定了规则集的环境数 / 总数；点进规则屏",
            StatusNav::Screen(Destination::Rules),
            active == Some(3),
            view.clone(),
            cx,
        ))
        .child(dot())
        .child(status_item(
            "status-upstream",
            format!("上游 {}", overview.upstream),
            muted,
            "经上游代理的环境数；点进代理屏",
            StatusNav::Screen(Destination::Proxies),
            active == Some(4),
            view.clone(),
            cx,
        ))
        .child(dot())
        .child(status_item(
            "status-certificate",
            format!("证书 {certificate}"),
            certificate_tone,
            "共享 CA 的系统信任状态；点进 CA 屏",
            StatusNav::Screen(Destination::Ca),
            active == Some(5),
            view,
            cx,
        ))
        .into_any_element()
}

/// 一段可点的状态栏聚合：数字 + 悬停提示 + 点击导航（+ 键盘高亮）。
#[allow(clippy::too_many_arguments)]
fn status_item(
    id: &'static str,
    text: String,
    tone: Hsla,
    hint: &'static str,
    nav: StatusNav,
    active: bool,
    view: WeakEntity<Workbench>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    div()
        .id(id)
        .px_1()
        .rounded(theme.radius)
        .text_color(tone)
        .role(Role::Button)
        .aria_label(SharedString::from(text.clone()))
        .cursor_pointer()
        .when(active, |this| this.bg(theme.secondary))
        .hover(move |style| style.bg(theme.secondary))
        .tooltip(text_tooltip(hint))
        .on_click(move |_, _, cx| {
            let _ = view.update(cx, |view, cx| view.navigate(nav, cx));
        })
        .child(SharedString::from(text))
        .into_any_element()
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title_bar = self.render_title_bar().into_any_element();
        let navigation = self.render_navigation(cx).into_any_element();
        let content = self.render_content(window, cx).into_any_element();
        let status_bar = self.render_status_bar(window, cx);

        div()
            // 根节点要有 id 才进 dispatch tree：F6（聚焦状态栏）挂在它上面，
            // 焦点在任何一个子控件里都能冒泡到这里。
            .id("workbench-root")
            .track_focus(&self.root_focus)
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            // F6 把焦点交给状态栏。为什么是 F6 而不是 Tab：表格把 Tab 绑给了
            // "下一列"（组件的上下文键位），Tab 出不了表格；F6 是桌面应用里
            // "跳到状态区"的惯例键，也不会和输入框/表格的键位抢。
            //
            // 用 action（键位见 `register_key_bindings`）而不是 on_key_down：
            // 后者依赖焦点链，而"没有任何焦点"正是最需要 F6 的时刻。
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

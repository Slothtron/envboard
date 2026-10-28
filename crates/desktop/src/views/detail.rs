//! 环境详情面板：`Sheet`（可拖宽、非遮罩）+ `TabBar` 三页签。
//!
//! 为什么是 `Sheet` 而不是自己拼一列：组件已经把"停靠侧栏 + 可拖宽 + 焦点陷阱 +
//! Esc 关闭 + 标题/页脚槽"做完了，自己写只会写出一套更差的（P0.3 之前这里打算
//! 手写 `clamp(520,48vw,860)` 宽度函数，现成件把那段整个省掉）。
//!
//! 面板是**独立实体**：它读同一份表数据（`Entity<TableState<EnvTable>>`），
//! 快照到了由环境屏 `touch` 一下刷新；写操作仍回到环境屏去发（一处真相）。
//!
//! 概览页遵守两条既有纪律：凭据**永不回显**（代理鉴权只给布尔），
//! 代理设置命令用服务端回显的 `proxy_command`，桌面不许自己重算。

use gpui_kit::component::ActiveTheme;
use gpui_kit::component::description_list::DescriptionList;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::table::TableState;
use gpui_kit::{
    AnyElement, App, Context, Entity, Font, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, Styled, Window, div, rems,
};

use envboard_protocol::{Desired, EnvView};

use crate::display::{health_label, is_public_listen, listen_address, needs_attention};
use crate::views::environments::EnvTable;

/// 面板页签。顺序与 web 形态一致：概览 / 配置 / 规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailTab {
    Overview,
    Config,
    Rules,
}

impl DetailTab {
    const ALL: [Self; 3] = [Self::Overview, Self::Config, Self::Rules];

    fn label(self) -> &'static str {
        match self {
            Self::Overview => "概览",
            Self::Config => "配置",
            Self::Rules => "规则",
        }
    }

    /// 页签内容是否已落地（未落地的页签显示诚实占位，不画假表单）。
    fn is_implemented(self) -> bool {
        matches!(self, Self::Overview)
    }
}

/// 详情面板实体。
pub struct Detail {
    name: SharedString,
    tab: DetailTab,
    table: Entity<TableState<EnvTable>>,
}

impl Detail {
    pub fn new(
        name: SharedString,
        table: Entity<TableState<EnvTable>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Self {
        // 页签切换由 TabBar 的 on_click 直接改本实体（无状态组件，不必再订阅）。
        Self {
            name,
            tab: DetailTab::Overview,
            table,
        }
    }

    pub fn name(&self) -> &SharedString {
        &self.name
    }

    /// 换到另一个环境（同一面板实体复用，避免每次选中都重建页签状态）。
    pub fn rename(&mut self, name: SharedString, _: &mut Window, cx: &mut Context<Self>) {
        if self.name != name {
            self.name = name;
            cx.notify();
        }
    }

    pub fn tab(&self) -> DetailTab {
        self.tab
    }

    pub fn select_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(tab) = DetailTab::ALL.get(index) {
            self.tab = *tab;
            cx.notify();
        }
    }

    /// 当前要展示的环境：从同一份表数据里按名字取（快照换代后名字仍有效）。
    fn env(&self, cx: &App) -> Option<EnvView> {
        self.table.read(cx).delegate().find_by_name(&self.name)
    }

    /// 等宽字体只给机器值（环境名、端口、命令、计数）。
    fn mono(&self, cx: &App) -> Font {
        Font {
            family: cx.theme().mono_font_family.clone(),
            ..Default::default()
        }
    }
}

impl Render for Detail {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(env) = self.env(cx) else {
            // 环境被删掉了：面板说清楚，不要留一份旧数据继续显示。
            return div()
                .id("detail-gone")
                .flex()
                .flex_col()
                .size_full()
                .gap_2()
                .p_4()
                .child(format!("环境 {} 已不存在。", self.name))
                .into_any_element();
        };
        let tabs = DetailTab::ALL
            .into_iter()
            .map(|tab| Tab::from(tab.label()))
            .collect::<Vec<_>>();
        let selected = DetailTab::ALL
            .iter()
            .position(|tab| *tab == self.tab)
            .unwrap_or(0);
        let body = match self.tab {
            tab if tab.is_implemented() => self.render_overview(&env, cx),
            tab => placeholder(tab, cx),
        };

        div()
            .id("detail-panel")
            .flex()
            .flex_col()
            .size_full()
            .gap_3()
            .p_4()
            .child(
                TabBar::new("detail-tabs")
                    .underline()
                    .children(tabs)
                    .selected_index(selected)
                    .on_click(cx.listener(|this, index, _, cx| {
                        this.select_tab(*index, cx);
                    })),
            )
            .child(body)
            .into_any_element()
    }
}

impl Detail {
    /// 概览页：全部走 `DescriptionList`（现成的 KV 组件），值可以是元素，
    /// 所以机器值用等宽字体、需处理/对外服务用主题色，都不用另造一套排版。
    fn render_overview(&self, env: &EnvView, cx: &App) -> AnyElement {
        let mono = self.mono(cx);
        let muted = cx.theme().muted_foreground;
        let theme = cx.theme();

        let status = SharedString::from(format!(
            "{} · 期望 {}",
            health_label(&env.health),
            if env.desired == Desired::Running {
                "运行"
            } else {
                "停止"
            }
        ));
        let listen = SharedString::from(listen_address(env));
        let rules = SharedString::from(match (&env.rules, env.rules_missing) {
            (Some(name), true) => format!("{name}（账本里没有这份规则，当前不覆盖任何域名）"),
            (Some(name), false) => format!("{name} · {} 条", env.rules_count),
            (None, _) => "未绑定".to_string(),
        });
        let upstream = SharedString::from(match &env.upstream {
            Some(name) if env.proxy_auth_enabled => format!("{name}（带鉴权，凭据不回显）"),
            Some(name) => format!("{name}（无鉴权）"),
            None => "直连（不经上游代理）".to_string(),
        });
        let attention = needs_attention(env);
        let public = is_public_listen(env);
        let description = if env.description.is_empty() {
            "（无）".to_string()
        } else {
            env.description.clone()
        };

        DescriptionList::horizontal()
            .columns(1)
            .bordered(true)
            .label_width(rems(9.))
            .item("状态", status, 1)
            .item(
                "需处理",
                yes_no(
                    attention,
                    "是",
                    "否",
                    if attention { theme.warning } else { muted },
                    cx,
                ),
                1,
            )
            .item(
                "监听",
                div().font(mono.clone()).child(listen).into_any_element(),
                1,
            )
            .item(
                "对外服务",
                yes_no(
                    public,
                    "是（绑在非回环地址）",
                    "否（仅回环）",
                    if public { theme.warning } else { muted },
                    cx,
                ),
                1,
            )
            .item("规则", rules, 1)
            .item("上游代理", upstream, 1)
            .item(
                "抓包",
                SharedString::from(if env.capture { "开启" } else { "关闭" }),
                1,
            )
            .item(
                "代理设置命令",
                // 命令用服务端回显的 `proxy_command`，桌面**不许自己重算**（D-10 的纪律）。
                div()
                    .font(mono)
                    .text_sm()
                    .child(SharedString::from(env.proxy_command.clone()))
                    .into_any_element(),
                1,
            )
            .item("说明", SharedString::from(description), 1)
            .into_any_element()
    }
}

/// 是/否一行：带主题色（需处理与对外服务这两项值得被看见，但不靠图标）。
fn yes_no(flag: bool, yes: &str, no: &str, color: gpui_kit::Hsla, _: &App) -> AnyElement {
    div()
        .text_color(color)
        .child(SharedString::from(if flag { yes } else { no }))
        .into_any_element()
}

/// 未落地页签的诚实占位（P0.6 填配置、P0.7 填规则）。
fn placeholder(tab: DetailTab, cx: &App) -> AnyElement {
    let milestone = match tab {
        DetailTab::Config => "P0.6",
        DetailTab::Rules => "P0.7",
        DetailTab::Overview => "",
    };
    div()
        .id(SharedString::from(format!(
            "detail-placeholder-{}",
            tab.label()
        )))
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(format!(
            "「{}」页签属于 {milestone}，还没落地 —— 这里不画假表单。",
            tab.label()
        ))
        .into_any_element()
}

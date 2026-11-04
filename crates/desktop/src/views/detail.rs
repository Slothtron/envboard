//! 环境详情面板：`Sheet`（可拖宽、非遮罩）+ `TabBar` 三页签（概览 / 配置 / 规则）。
//!
//! 为什么是 `Sheet` 而不是自己拼一列：组件已经把"停靠侧栏 + 可拖宽 + 焦点陷阱 +
//! Esc 关闭 + 标题/页脚槽"做完了，自己写只会写出一套更差的（P0.3 之前这里打算
//! 手写 `clamp(520,48vw,860)` 宽度函数，现成件把那段整个省掉）。
//!
//! 面板是**独立实体**：它读同一份表数据（`Entity<TableState<EnvTable>>`），快照到了
//! 由环境屏 `touch` 一下刷新。写操作（保存配置）经表单自己的 `hop` 发出，回执后叫
//! 环境屏补一次快照 —— "哪一行在等回执"仍只有一处真相。
//!
//! 三条既有纪律在这里落地：凭据**永不回显**（密码框永远从空开始，留空 = 保持不变）；
//! 代理设置命令用服务端回显的 `proxy_command`，桌面不许重算；**热生效 / 停机生效**
//! 逐字段说清楚，但裁决权在服务端（运行中改停机字段会被拒，界面如实转述）。

use gpui_kit::component::ActiveTheme;
use gpui_kit::component::Disableable;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::description_list::DescriptionList;

use gpui_kit::component::form::Field;
use gpui_kit::component::input::{
    Input, InputContentType, InputEvent, InputState, Textarea, TextareaState,
};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::table::TableState;
use gpui_kit::component::{Icon, IconName, WindowExt};
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, Font, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Task, Window, div, px,
};

use envboard_protocol::{Desired, EnvView};

use crate::display::{
    command_lines, health_label, is_public_listen, listen_address, needs_attention,
};
use crate::errors::{Action, Notice};
use crate::host::{Host, hop};
use crate::views::environments::{EnvTable, Environments};
use crate::views::error_banner;
use gpui_kit::WeakEntity;

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
}

/// 规则页签的取数状态（正文是读盘来的：可能失败，也可能压根没绑）。
enum RulesState {
    /// 还没为当前环境发起过读盘（切页签 / 换环境后由加载逻辑接手）。
    Idle,
    /// 当前环境还没进快照，等下一帧再试。与 Idle 分开是为了不让渲染期
    /// 每帧重复发起读盘 —— `rules_task` 仍是 `None`。
    WaitingSnapshot,
    Loading,
    /// 未绑定规则集。
    Unbound,
    /// 绑了名字但账本里没有（该环境当前不覆盖任何域名）。
    Missing,
    Text(SharedString),
    Failed(Notice),
}

/// 详情面板实体。
pub struct Detail {
    name: SharedString,
    tab: DetailTab,
    table: Entity<TableState<EnvTable>>,
    environments: WeakEntity<Environments>,
    /// 配置表单：只在第一次进入「配置」页签时建；「放弃修改」= 丢掉重建，于是初值
    /// 永远回到服务端说的那一份，不需要自己维护第二套脏值。
    config: Option<Entity<ConfigForm>>,
    rules: RulesState,
    rules_task: Option<Task<()>>,
}

impl Detail {
    pub fn new(
        name: SharedString,
        table: Entity<TableState<EnvTable>>,
        environments: WeakEntity<Environments>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Self {
        Self {
            name,
            tab: DetailTab::Overview,
            table,
            environments,
            config: None,
            rules: RulesState::Idle,
            rules_task: None,
        }
    }

    pub fn name(&self) -> &SharedString {
        &self.name
    }

    /// 换到另一个环境：表单与规则正文都作废重建（旧环境的脏改动不该跟过来）。
    ///
    /// 面板实体是复用的（`open_detail` 只 `rename`），所以这里必须**同时**处理
    /// 在途的规则读盘：旧环境的异步回执会无条件写 `self.rules`，不取消它、也不
    /// 校验回执属于谁，A 的正文就会落到 B 的面板里。`Task` drop 即取消。
    pub fn rename(&mut self, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        if self.name == name {
            return;
        }
        self.name = name;
        self.config = None;
        self.rules = RulesState::Idle;
        self.rules_task = None;
        // 停留在「规则」页签时立即为新环境重读；没停留就等下次切进来。
        if self.tab == DetailTab::Rules {
            self.load_rules(window, cx);
        }
        cx.notify();
    }

    /// 当前要展示的环境：从同一份表数据里按名字取（快照换代后名字仍有效）。
    fn env(&self, cx: &App) -> Option<EnvView> {
        self.table.read(cx).delegate().find_by_name(&self.name)
    }

    /// 等宽字体只给机器值（端口、地址、命令、规则正文）。
    fn mono(&self, cx: &App) -> Font {
        Font {
            family: cx.theme().mono_font_family.clone(),
            ..Default::default()
        }
    }

    fn select_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = DetailTab::ALL.get(index).copied() else {
            return;
        };
        self.tab = tab;
        if tab == DetailTab::Rules {
            self.load_rules(window, cx);
        }
        cx.notify();
    }

    /// 读绑定规则的正文（读盘走 tokio，主线程不碰 IO）。
    fn load_rules(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(env) = self.env(cx) else {
            // 新环境可能还没进这一拍快照：标记等待，别在渲染期反复起任务。
            self.rules = RulesState::WaitingSnapshot;
            return;
        };
        let Some(bound) = env.rules.clone() else {
            self.rules = RulesState::Unbound;
            return;
        };
        if env.rules_missing {
            self.rules = RulesState::Missing;
            return;
        }
        self.rules = RulesState::Loading;
        let admin = cx.global::<Host>().admin().clone();
        let name = bound.clone();
        let subject = bound.clone();
        // 回执带环境名：面板可能在读盘途中被 `rename` 到别的环境，那份旧正文
        // 不能落进新面板（`rules_task` 被取消时，已送进主线程的回执仍会到达）。
        let owner = self.name.clone();
        self.rules_task = Some(hop(
            window,
            cx,
            async move { admin.rules_read(&name) },
            move |this, outcome, _, cx| {
                if this.name != owner {
                    return;
                }
                this.rules = match outcome {
                    Ok(text) => RulesState::Text(text.into()),
                    Err(error) => RulesState::Failed(Notice::from_error(
                        Action::ReadRules,
                        Some(&subject),
                        &error,
                    )),
                };
                cx.notify();
            },
        ));
    }

    /// 「放弃修改」：丢掉表单实体，下次渲染按当前环境重建。
    fn discard_config(&mut self, cx: &mut Context<Self>) {
        self.config = None;
        cx.notify();
    }

    /// 表单保存后要叫列表补一次快照（句柄由环境屏在建面板时给进来）。
    pub fn environments(&self) -> &WeakEntity<Environments> {
        &self.environments
    }
}

impl Render for Detail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(env) = self.env(cx) else {
            // 环境被删掉了：面板说清楚，不要留一份旧数据继续显示。
            return div()
                .id("detail-gone")
                .flex()
                .flex_col()
                .size_full()
                .gap_2()
                .p_4()
                .child(SharedString::from(format!("环境 {} 已不存在。", self.name)))
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
        if self.tab == DetailTab::Config && self.config.is_none() {
            let parent = cx.weak_entity();
            let name = SharedString::from(env.name.clone());
            let seed = env.clone();
            self.config = Some(cx.new(|cx| ConfigForm::new(seed, name, parent, window, cx)));
        }
        // 上一帧还在等快照，这一帧环境已经进表了 → 回到 Idle，让下面的
        // 守卫接手重读（不在 render 里直接起任务，保持"一处加载"）。
        if matches!(self.rules, RulesState::WaitingSnapshot) && self.env(cx).is_some() {
            self.rules = RulesState::Idle;
        }
        // 兜底：`rename` 时新环境可能还没进快照（找不到就标 WaitingSnapshot），
        // 等下一帧快照到了要再试一次。`rules_task` 守卫保证不会每帧起一次读盘：
        // 真正起过任务的状态（Loading / Text / Failed）都不该再进这里。
        if self.tab == DetailTab::Rules
            && matches!(self.rules, RulesState::Idle)
            && self.rules_task.is_none()
        {
            self.load_rules(window, cx);
        }
        let body = match self.tab {
            DetailTab::Overview => self.render_overview(&env, cx),
            DetailTab::Config => match &self.config {
                Some(form) => {
                    let form = form.clone();
                    form.update(cx, |form, cx| form.render_body(window, cx))
                        .into_any_element()
                }
                None => div().into_any_element(),
            },
            DetailTab::Rules => self.render_rules(cx),
        };

        div()
            .id("detail-panel")
            // L0 用它判断"面板到底开没开"：无头上下文没有查询 Sheet 的 API，
            // 只能看这一块锚点有没有被画出来。
            .debug_selector(|| "env-detail-panel".to_string())
            .flex()
            .flex_col()
            .size_full()
            .gap_3()
            .p_4()
            .overflow_y_scroll()
            .child(
                TabBar::new("detail-tabs")
                    .underline()
                    .children(tabs)
                    .selected_index(selected)
                    .on_click(cx.listener(move |this, index, window, cx| {
                        this.select_tab(*index, window, cx);
                    })),
            )
            .child(body)
            .into_any_element()
    }
}

impl Detail {
    fn render_overview(&mut self, env: &EnvView, cx: &mut Context<Self>) -> AnyElement {
        let mono = self.mono(cx);
        let warning = cx.theme().warning;
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
        let description = if env.description.is_empty() {
            "（无）".to_string()
        } else {
            env.description.clone()
        };
        let attention = needs_attention(env);
        let public = is_public_listen(env);
        let reason = env
            .health_reason
            .clone()
            .filter(|reason| !reason.trim().is_empty());

        let mut list = DescriptionList::horizontal()
            .columns(1)
            .bordered(true)
            .label_width(px(120.))
            .item("状态", status, 1);

        // 「需处理」只在真的需要处理时出现：一行恒为"否"的布尔值只是噪音，
        // 它还会让面板看起来"每条都有问题"。原因跟在这行后面 —— 这是"需处理"
        // 唯一有用的下半句（服务端一直在给，此前界面从不显示）。
        if attention {
            list = list.item(
                "需处理",
                div().text_color(warning).child("是").into_any_element(),
                1,
            );
            if let Some(reason) = reason {
                list = list.item(
                    "原因",
                    div()
                        .text_color(warning)
                        .child(SharedString::from(reason))
                        .into_any_element(),
                    1,
                );
            }
        }

        list = list
            .item(
                "监听",
                div().font(mono.clone()).child(listen).into_any_element(),
                1,
            )
            // 只在真的对外服务时占一行（回环是默认，默认不必说）。
            .item("规则", rules, 1)
            .item("上游代理", upstream, 1)
            .item(
                "抓包",
                SharedString::from(if env.capture { "开启" } else { "关闭" }),
                1,
            );

        if public {
            list = list.item(
                "对外服务",
                div()
                    .text_color(warning)
                    .child("是（绑在非回环地址）")
                    .into_any_element(),
                1,
            );
        }

        // 代理设置命令是这一屏里**唯一要用户带走**的内容：必须能一键复制。
        // 命令用服务端回显的 `proxy_command`，桌面不许自己重算。
        let command = env.proxy_command.clone();
        // 图标按钮而不是带字按钮：这一行的宽度要留给命令本身（等宽文本，
        // 一行 `export VAR=http://host:port` 在面板里刚好一行放得下，
        // 「复制」两个字会把最后几位端口挤出去）。
        let copy_command = Button::new("copy-proxy-command")
            .xsmall()
            .ghost()
            .icon(Icon::new(IconName::Copy))
            .accessibility_label("复制代理设置命令")
            .tooltip("复制代理设置命令（可直接粘到终端）")
            .on_click(cx.listener(move |_, _, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(command.clone()));
                window.push_notification(
                    Notification::success("已复制代理设置命令").id1::<Self>("detail-copy"),
                    cx,
                );
            }));

        list = list
            .item(
                "代理设置命令",
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .font(mono)
                            // 比正文小一档：一行 `export VAR=http://host:port` 要挤进
                            // 「标签列 + 值列 + 复制按钮」三段之后的宽度里，text_sm 会
                            // 差最后几位端口（实测 42 字符 × 8.3px > 可用 332px）。
                            .text_xs()
                            // 一条赋值一行、行内不折行：等宽文本按字符折行会从 URL
                            // 中间断开（读起来像两条命令）。复制仍给整条原文。
                            .children(command_lines(&env.proxy_command).into_iter().map(|line| {
                                div().whitespace_nowrap().child(SharedString::from(line))
                            })),
                    )
                    .child(copy_command)
                    .into_any_element(),
                1,
            )
            .item("说明", SharedString::from(description), 1);

        list.into_any_element()
    }

    fn render_rules(&self, cx: &App) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        match &self.rules {
            RulesState::Text(text) => div()
                .id("detail-rules-text")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                // 正文要保留换行：GPUI 的 Div 没有 whitespace_pre，所以按行拆成子元素
                // （空行填一个空格，否则那一行会被压成 0 高）。
                .child(
                    div()
                        .font(self.mono(cx))
                        .text_sm()
                        .children(text.lines().map(|line| {
                            div().child(SharedString::from(if line.is_empty() {
                                " "
                            } else {
                                line
                            }))
                        })),
                )
                .into_any_element(),
            // 与表格上的错误条同一套呈现（中文主句 + 码 + 英文原文）：同一个应用里
            // 错误只有一种长相。
            RulesState::Failed(notice) => error_banner("detail-rules-failed", notice.clone()),
            other => {
                let (id, body) = match other {
                    RulesState::Loading => ("detail-rules-loading", "读取规则正文…"),
                    RulesState::WaitingSnapshot => (
                        "detail-rules-waiting",
                        "等待环境进入快照，随后读取规则正文…",
                    ),
                    RulesState::Unbound => (
                        "detail-rules-unbound",
                        "未绑定规则集：这个环境不改写任何域名。",
                    ),
                    RulesState::Missing => (
                        "detail-rules-missing",
                        "绑定了规则名，但账本里没有这份规则 —— 该环境当前不覆盖任何域名。",
                    ),
                    _ => ("detail-rules-idle", "切换到「规则」页签读取正文。"),
                };
                div()
                    .id(id)
                    .text_sm()
                    .text_color(muted)
                    .child(body)
                    .into_any_element()
            }
        }
    }
}

// --------------------------------------------------------------------------- #
// 配置页签
// --------------------------------------------------------------------------- #

/// 两个选择器：规则集与上游代理。写成枚举是为了让"当前值 / 清除项 / 落到哪个字段"
/// 三件事只有一份代码。
#[derive(Debug, Clone, Copy)]
enum PickerKind {
    Rules,
    Upstream,
}

impl PickerKind {
    fn id(self) -> &'static str {
        match self {
            Self::Rules => "picker-rules",
            Self::Upstream => "picker-upstream",
        }
    }

    /// 空值那一档的文案（也是"清除绑定"的菜单项）。
    fn none_label(self) -> &'static str {
        match self {
            Self::Rules => "未绑定",
            Self::Upstream => "直连",
        }
    }

    fn current(self, form: &ConfigForm) -> Option<SharedString> {
        match self {
            Self::Rules => form.rules.clone(),
            Self::Upstream => form.upstream.clone(),
        }
    }

    fn options(self, form: &ConfigForm) -> &[SharedString] {
        match self {
            Self::Rules => &form.rule_names,
            Self::Upstream => &form.proxy_names,
        }
    }

    fn apply(self, form: &mut ConfigForm, name: SharedString) {
        match self {
            Self::Rules => form.rules = Some(name),
            Self::Upstream => form.upstream = Some(name),
        }
    }
}

/// 配置表单实体：字段与 web 形态一一对应。
///
/// 脏状态是粗粒度的（任一输入动过就算脏）—— web 是逐字段比对，但两边的**提交内容**
/// 与裁决都在服务端，粗粒度不会放过任何非法值，只可能多提示一次"有改动"。
pub struct ConfigForm {
    env_name: SharedString,
    parent: WeakEntity<Detail>,
    description: Entity<InputState>,
    port: Entity<InputState>,
    insecure_hosts: Entity<TextareaState>,
    mitm_hosts: Entity<TextareaState>,
    proxy_user: Entity<InputState>,
    proxy_password: Entity<InputState>,
    public_listen: bool,
    capture: bool,
    rules: Option<SharedString>,
    upstream: Option<SharedString>,
    rule_names: Vec<SharedString>,
    proxy_names: Vec<SharedString>,
    advanced_open: bool,
    dirty: bool,
    saving: bool,
    message: Option<Notice>,
    options_task: Option<Task<()>>,
    save_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl ConfigForm {
    fn new(
        env: EnvView,
        name: SharedString,
        parent: WeakEntity<Detail>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let description = cx.new(|cx| InputState::new(window, cx).default_value(&env.description));
        let port =
            cx.new(|cx| InputState::new(window, cx).default_value(env.listen.port.to_string()));
        let insecure_hosts = cx
            .new(|cx| TextareaState::new(window, cx).default_value(env.insecure_hosts.join("\n")));
        let mitm_hosts =
            cx.new(|cx| TextareaState::new(window, cx).default_value(env.mitm_hosts.join("\n")));
        // 凭据只进不出：两个框永远从空开始，留空 = 保持不变（服务端语义）。
        let proxy_user = cx.new(|cx| InputState::new(window, cx).placeholder("用户名"));
        // 密码框打码输入：`InputState::masked` 在 gpui-base 里就有（组件层的
        // `Input` 只是把它渲染出来），打码顺带把值挡在剪贴板选择之外。
        let proxy_password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("留空则保持不变")
                .masked(true)
        });

        // 任一输入变化都算脏（保存钮与"有未保存的改动"提示都看这个）。
        let mut subscriptions = Vec::new();
        for state in [&description, &port, &proxy_user, &proxy_password] {
            subscriptions.push(cx.subscribe_in(state, window, |this, _, event, _, cx| {
                let InputEvent::Change = event else { return };
                if !this.dirty {
                    this.dirty = true;
                    cx.notify();
                }
            }));
        }
        subscriptions.push(
            cx.subscribe_in(&insecure_hosts, window, |this, _, event, _, cx| {
                let InputEvent::Change = event else { return };
                if !this.dirty {
                    this.dirty = true;
                    cx.notify();
                }
            }),
        );
        subscriptions.push(
            cx.subscribe_in(&mitm_hosts, window, |this, _, event, _, cx| {
                let InputEvent::Change = event else { return };
                if !this.dirty {
                    this.dirty = true;
                    cx.notify();
                }
            }),
        );

        Self {
            env_name: name,
            parent,
            description,
            port,
            insecure_hosts,
            mitm_hosts,
            proxy_user,
            proxy_password,
            public_listen: is_public_listen(&env),
            capture: env.capture,
            rules: env.rules.clone().map(SharedString::from),
            upstream: env.upstream.clone().map(SharedString::from),
            rule_names: Vec::new(),
            proxy_names: Vec::new(),
            advanced_open: false,
            dirty: false,
            saving: false,
            message: None,
            options_task: None,
            save_task: None,
            _subscriptions: subscriptions,
        }
    }

    /// 规则集与上游代理的候选：一次 tokio 往返取两份列表（列表短，不建缓存层）。
    fn load_options(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let admin = cx.global::<Host>().admin().clone();
        self.options_task = Some(hop(
            window,
            cx,
            async move {
                let rules = admin
                    .rules_list()
                    .map(|names| {
                        names
                            .into_iter()
                            .map(SharedString::from)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let proxies = admin
                    .proxy_list()
                    .map(|views| {
                        views
                            .into_iter()
                            .map(|proxy| SharedString::from(proxy.name))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                (rules, proxies)
            },
            move |this, (rules, proxies), _, cx| {
                this.rule_names = rules;
                this.proxy_names = proxies;
                cx.notify();
            },
        ));
    }

    /// 客户端形状校验（与服务端同判据，但**裁决仍在服务端**）。
    fn build_patch(
        &self,
        cx: &App,
    ) -> Result<serde_json::Map<String, serde_json::Value>, SharedString> {
        let mut patch = serde_json::Map::new();
        patch.insert(
            "description".into(),
            self.description.read(cx).value().to_string().into(),
        );

        let port_raw = self.port.read(cx).value().trim().to_string();
        if port_raw.is_empty() || !port_raw.chars().all(|c| c.is_ascii_digit()) {
            return Err("端口必须是数字。".into());
        }
        let Ok(port) = port_raw.parse::<u16>() else {
            return Err("端口超出范围（1–65535）。".into());
        };
        patch.insert("port".into(), serde_json::json!(port));

        let hosts = self
            .insecure_hosts
            .read(cx)
            .value()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        patch.insert("insecure_hosts".into(), hosts.into());

        let mitm = self
            .mitm_hosts
            .read(cx)
            .value()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        patch.insert("mitm_hosts".into(), mitm.into());

        // 代理鉴权：两栏必须同时填（同时空着 = 保持不变），都不能含 ":" 或空白。
        let user = self.proxy_user.read(cx).value().to_string();
        let password = self.proxy_password.read(cx).value().to_string();
        if user.is_empty() != password.is_empty() {
            return Err("代理鉴权的用户名与密码必须同时填写。".into());
        }
        if !user.is_empty() {
            for value in [&user, &password] {
                if value.chars().any(char::is_whitespace) || value.contains(':') {
                    return Err("代理鉴权的用户名与密码都不能含「:」或空白字符。".into());
                }
            }
            patch.insert(
                "proxy_auth".into(),
                serde_json::json!({ "user": user, "password": password }),
            );
        }

        patch.insert(
            "host".into(),
            if self.public_listen {
                "0.0.0.0"
            } else {
                "127.0.0.1"
            }
            .into(),
        );
        patch.insert("rules".into(), binding_json(&self.rules));
        patch.insert("upstream".into(), binding_json(&self.upstream));
        patch.insert("capture".into(), self.capture.into());
        Ok(patch)
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let patch = match self.build_patch(cx) {
            Ok(patch) => patch,
            Err(message) => {
                // 本地校验拦下来的（形状不对，还没到服务端）：没有错误码也没有原文，
                // 只有一句中文 —— 与门面错误同一个错误条、同一种长相。
                self.message = Some(Notice::plain(message));
                cx.notify();
                return;
            }
        };
        let admin = cx.global::<Host>().admin().clone();
        let name = self.env_name.clone();
        let body = serde_json::Value::Object(patch);
        self.saving = true;
        self.message = None;
        cx.notify();
        let parent = self.parent.clone();
        self.save_task = Some(hop(
            window,
            cx,
            async move {
                let outcome = admin.update(&name, &body);
                (name, outcome.map(|env| env.name))
            },
            move |this, (name, outcome), window, cx| {
                this.saving = false;
                match outcome {
                    Ok(_) => {
                        this.dirty = false;
                        this.message = None;
                    }
                    Err(error) => {
                        this.message = Some(Notice::from_error(
                            Action::SaveEnvironment,
                            Some(&name),
                            &error,
                        ));
                    }
                }
                cx.notify();
                // 成没成都要叫列表补一次快照：服务端可能部分生效（热应用被拒也要看得见）。
                let _ = parent.update(cx, |parent, cx| {
                    let environments = parent.environments().clone();
                    let _ = environments.update(cx, |view, cx| view.refresh_now(window, cx));
                });
            },
        ));
    }

    /// 选择器：短列表用现成的 `dropdown_menu`（带勾选态与"清除"项）。
    ///
    /// 为什么不是 `Select`：`SelectState` 要求实现 `SearchableListDelegate`
    /// （搜索 + 委托 + 渲染一整套），对"规则集 / 上游代理"这种几条到几十条的短列表
    /// 是过度设计，而 `dropdown_menu` 在本地同构项目里已验证。真需要搜索的长列表
    /// 出现时再换，换的成本收在这一个函数里。
    fn picker(&self, kind: PickerKind, owner: WeakEntity<ConfigForm>) -> impl IntoElement {
        let current = kind
            .current(self)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| SharedString::from(kind.none_label()));
        let options = kind.options(self).to_vec();
        let selected = kind.current(self).unwrap_or_default();

        Button::new(kind.id())
            .xsmall()
            .ghost()
            .label(current.clone())
            .accessibility_label(format!("{current}，点击更换"))
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                let owner_clear = owner.clone();
                menu = menu.item(
                    PopupMenuItem::new(kind.none_label())
                        .checked(selected.is_empty())
                        .on_click(move |_, _, cx| {
                            let _ = owner_clear.update(cx, |form, cx| {
                                kind.apply(form, SharedString::default());
                                form.dirty = true;
                                cx.notify();
                            });
                        }),
                );
                for option in options.iter().cloned() {
                    let owner_item = owner.clone();
                    let selected_item = selected.clone();
                    menu = menu.item(
                        PopupMenuItem::new(option.to_string())
                            .checked(option == selected_item)
                            .on_click(move |_, _, cx| {
                                let name = option.clone();
                                let _ = owner_item.update(cx, |form, cx| {
                                    kind.apply(form, name);
                                    form.dirty = true;
                                    cx.notify();
                                });
                            }),
                    );
                }
                menu
            })
    }

    /// 表单主体（由 `Detail` 在配置页签下调用）。
    pub fn render_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.options_task.is_none() {
            self.load_options(window, cx);
        }
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let saving = self.saving;
        let dirty = self.dirty;
        let owner = cx.weak_entity();
        let hint = |text: &'static str| div().text_xs().text_color(muted).child(text);

        div()
            .id("config-form")
            .flex()
            .flex_col()
            .gap_3()
            .w_full()
            .max_w(px(760.))
            .child(
                Field::new()
                    .label("说明")
                    .child(Input::new(&self.description).w_full())
                    .child(hint("纯展示字段，热生效。")),
            )
            .child(
                Field::new()
                    .label("监听端口")
                    .child(Input::new(&self.port).w(px(160.)))
                    .child(hint("范围 16000–16999。停机生效。")),
            )
            .child(
                Field::new()
                    .label("规则集")
                    .child(self.picker(PickerKind::Rules, owner.clone()))
                    .child(hint("热生效：换绑定后运行中的实例立即按新规则改写，不必重启。")),
            )
            .child(
                Field::new()
                    .label("上游代理")
                    .child(self.picker(PickerKind::Upstream, owner))
                    .child(hint("热生效。绑定后出方向先经所选代理再到目标。")),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        Button::new("toggle-advanced")
                            .xsmall()
                            .ghost()
                            // 折叠区用组件库的 `Collapsible`：它自己管"开/合"的内容
                            // 挂载与可逆的展开动画；触发器仍是一个普通按钮
                            // （按钮负责"按下就展开"的语义）。
                            .label(if self.advanced_open {
                                "收起高级配置"
                            } else {
                                "高级配置（放宽校验 / 解密白名单 / 对外服务 / 抓包 / 访问鉴权）"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.advanced_open = !this.advanced_open;
                                cx.notify();
                            })),
                    )
                    .child(
                        Collapsible::new()
                            .open(self.advanced_open)
                            .content(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_3()
                                    .child(
                                        Field::new()
                                            .label("按域名放宽上游校验")
                                            .child(Textarea::new(&self.insecure_hosts).w_full()),
                                    )
                                    .child(hint(
                                        "每行一个完整域名，精确匹配：列出的域名才跳过上游证书校验，子域不继承、后缀不匹配，通配符会被服务端拒绝。热生效。",
                                    ))
                                    .child(
                                        Field::new()
                                            .label("MITM 白名单（只解密这些域名）")
                                            .child(Textarea::new(&self.mitm_hosts).w_full()),
                                    )
                                    .child(hint(
                                        "留空 = 全部域名解密。填了就只对这些域名做 TLS 解密与改写，名单外的 CONNECT 字节原样直通（不抓包、不改写，鉴权在 CONNECT 上判）。热生效。",
                                    ))
                                    .child(
                                        Checkbox::new("public-listen")
                                            .label("对外服务（绑到 0.0.0.0）")
                                            .checked(self.public_listen)
                                            .on_change(cx.listener(|this, value, _, cx| {
                                                this.public_listen = *value;
                                                this.dirty = true;
                                                cx.notify();
                                            })),
                                    )
                                    .child(hint(
                                        "默认只监听 127.0.0.1；勾选后局域网内可直接连此代理。停机生效。",
                                    ))
                                    .child(
                                        Checkbox::new("capture")
                                            .label("抓包（记录该环境的 HTTPS 请求/响应）")
                                            .checked(self.capture)
                                            .on_change(cx.listener(|this, value, _, cx| {
                                                this.capture = *value;
                                                this.dirty = true;
                                                cx.notify();
                                            })),
                                    )
                                    .child(hint("热生效。清空走显式动作。"))
                                    .child(
                                        Field::new().label("代理访问鉴权").child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .child(Input::new(&self.proxy_user).w_1_2())
                                                // 掩码 + 可切换可见：值不回显，但用户能确认自己打对了。
                                                .child(
                                                    Input::new(&self.proxy_password)
                                                        .w_1_2()
                                                        .mask_toggle()
                                                        .content_type(InputContentType::Password),
                                                ),
                                        ),
                                    )
                                    .child(hint(
                                        "两栏必须同时填写，都不能含「:」或空白字符。凭据只进不出 —— 服务端永不回显。停机生效。",
                                    )),
                            ),
                    ),
            )
            .children(
                self.message
                    .clone()
                    .map(|message| error_banner("config-message", message)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("config-dirty")
                            .text_xs()
                            .text_color(muted)
                            .child(if dirty {
                                "有未保存的改动。端口与代理鉴权是停机生效字段，运行中保存会被拒绝。"
                            } else {
                                "没有未保存的改动。"
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("config-reset")
                            .xsmall()
                            .ghost()
                            .label("放弃修改")
                            .disabled(!dirty || saving)
                            .on_click({
                                let parent = self.parent.clone();
                                move |_, _, cx| {
                                    let _ = parent.update(cx, |parent, cx| {
                                        parent.discard_config(cx)
                                    });
                                }
                            }),
                    )
                    .child(
                        Button::new("config-save")
                            .xsmall()
                            .primary()
                            .label("保存")
                            .loading(saving)
                            .disabled(!dirty || saving)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}

/// 绑定字段的 JSON：空 = 清除绑定（服务端接受 null）。
fn binding_json(value: &Option<SharedString>) -> serde_json::Value {
    match value {
        Some(name) if !name.is_empty() => serde_json::Value::String(name.to_string()),
        _ => serde_json::Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::{PickerKind, binding_json};
    use gpui_kit::SharedString;

    #[test]
    fn pickers_clear_to_the_same_words_the_rest_of_the_screen_uses() {
        // 选择器的"空"必须与概览/表格里的说法一致，否则同一件事有两种文案。
        assert_eq!(PickerKind::Rules.none_label(), "未绑定");
        assert_eq!(PickerKind::Upstream.none_label(), "直连");
    }

    #[test]
    fn clearing_a_binding_sends_null_not_an_empty_string() {
        let none: Option<SharedString> = None;
        let empty: Option<SharedString> = Some("".into());
        let bound: Option<SharedString> = Some("corp".into());
        assert!(binding_json(&none).is_null(), "没绑要发 null");
        assert!(
            binding_json(&empty).is_null(),
            "空名字等于清除绑定，也发 null"
        );
        assert_eq!(binding_json(&bound).as_str(), Some("corp"));
    }
}

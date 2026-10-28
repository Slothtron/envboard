//! 目录屏：规则集与上游代理。两屏形状一样（工具条 + 列表 + 表单弹窗 + 删除确认），
//! 所以放在一起；表格与弹窗全部用现成组件，本文件不写任何表格度量。
//!
//! 与环境屏的差别只在数据源：目录是**拉取式**（进屏与写后各取一次），不是 1s 快照流 ——
//! 规则集与代理的变更都来自本屏自己，没有别的写者。
//!
//! 三条既有纪律在这里是硬要求：
//!
//! * 引用关系决定能不能删 —— 被环境引用的上游代理，删除钮直接禁用并说明原因；
//!   规则集允许删（web 形态如此），但确认弹窗要把后果写出来。
//! * 凭据永不回显：`ProxyView` 只给 `has_auth` 布尔，所以编辑时用户名与密码都从空
//!   开始。这里的"空"是**不配置鉴权**（服务端按整条记录覆盖），不是"保持不变" ——
//!   弹窗文案必须把这件事说清楚，不许让用户以为改个地址就能顺带保住凭据。
//! * 请求体的形状由服务端契约决定，桌面不许自己发明：`port` 是数字不是字符串，
//!   `user`/`password` 要么两者都给、要么两者都不给（空串会被拒）。
//!
//! 还有一条是组件给不了的：`DataTable` 没有"列拉伸填满"的开关（`Column` 只有
//! `width`/`min_width`，宽度都是实数），所以列宽预算要按默认窗口宽度排布，让剩余
//! 空白落在最后一列之后而不是列与列之间 —— 与环境屏同一处理方式。

use gpui_kit::component::ActiveTheme;
use gpui_kit::component::Disableable;
use gpui_kit::component::Sizable;
use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants};
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::component::form::Field;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::table::{Column, DataTable, TableDelegate, TableState};
use gpui_kit::component::{Icon, IconName};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Font, FontWeight, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Task, WeakEntity, Window, div, px,
};

use envboard_protocol::ProxyView;

use crate::host::{Host, hop};
use crate::views::environments::EnvTable;

// --------------------------------------------------------------------------- #
// 规则集目录
// --------------------------------------------------------------------------- #

const RULES_NAME: usize = 0;
const RULES_COUNT: usize = 1;
const RULES_REFS: usize = 2;
const RULES_ACTIONS: usize = 3;

/// 规则集表：名字来自 `rules_list`，条数与引用从环境快照派生（服务端不单独给）。
pub struct RulesTable {
    columns: Vec<Column>,
    /// 服务端给的全量名字（顺序即账本顺序）。
    rows: Vec<SharedString>,
    /// 按搜索词过滤后的可见行 —— 表只认这个，所以行下标与搜索词同源。
    visible: Vec<SharedString>,
    query: String,
    envs: Entity<TableState<EnvTable>>,
    loading: bool,
    error: Option<SharedString>,
    view: WeakEntity<RulesCatalog>,
}

impl RulesTable {
    fn new(envs: Entity<TableState<EnvTable>>, view: WeakEntity<RulesCatalog>) -> Self {
        let columns = [
            ("name", "规则集", 220.),
            ("entries", "生效条数", 110.),
            // 引用列吃掉剩余宽度：环境名可能很长，也可能一个都没有。
            // 宽度按默认窗口（1180）排到接近填满 —— DataTable 没有"列拉伸填满"的开关。
            ("refs", "被哪些环境引用", 440.),
            // 两个动作（查看原文 + 删除）；不给排序箭头 —— 排序对它没有意义。
            ("actions", "操作", 130.),
        ]
        .into_iter()
        .map(|(key, name, width)| Column::new(key, name).resizable(true).width(px(width)))
        .collect();
        Self {
            columns,
            rows: Vec::new(),
            visible: Vec::new(),
            query: String::new(),
            envs,
            loading: true,
            error: None,
            view,
        }
    }

    fn set_rows(&mut self, outcome: Result<Vec<String>, String>) -> bool {
        self.loading = false;
        match outcome {
            Ok(names) => {
                let next: Vec<SharedString> = names.into_iter().map(SharedString::from).collect();
                let changed = next != self.rows;
                self.rows = next;
                self.error = None;
                self.rebuild_visible();
                changed
            }
            Err(message) => {
                let changed = self.error.as_deref() != Some(message.as_str());
                self.error = Some(message.into());
                changed
            }
        }
    }

    /// 把一条错误写进表（错误条渲染在视图层，但状态归表持有，避免两处真相）。
    fn set_error(&mut self, message: String) -> bool {
        let changed = self.error.as_deref() != Some(message.as_str());
        self.error = Some(message.into());
        self.loading = false;
        changed
    }

    pub fn set_query(&mut self, query: &str) -> bool {
        if self.query == query {
            return false;
        }
        self.query = query.to_string();
        self.rebuild_visible();
        true
    }

    fn rebuild_visible(&mut self) {
        let query = self.query.trim().to_lowercase();
        self.visible = if query.is_empty() {
            self.rows.clone()
        } else {
            self.rows
                .iter()
                .filter(|name| name.to_lowercase().contains(&query))
                .cloned()
                .collect()
        };
    }

    /// 有搜索词却一行不剩 —— 用来把"空表"和"没匹配"区分开说清楚。
    pub fn filtered_out(&self) -> bool {
        self.visible.is_empty() && !self.query.trim().is_empty()
    }

    pub fn error(&self) -> Option<SharedString> {
        self.error.clone()
    }
}

impl TableDelegate for RulesTable {
    fn columns_count(&self, _: &App) -> usize {
        4
    }

    fn rows_count(&self, _: &App) -> usize {
        self.visible.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.columns[col_ix].clone()
    }

    fn loading(&self, _: &App) -> bool {
        self.loading
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(name) = self.visible.get(row_ix).cloned() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        // 引用关系与条数都从环境快照派生 —— 同一份数据，不另算一套。
        let refs = self.envs.read(cx).delegate().referenced_by_rules(&name);
        let entries = self.envs.read(cx).delegate().rules_count_of(&name);
        let refs_text = if refs.is_empty() {
            "无环境引用".to_string()
        } else {
            refs.iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>()
                .join("、")
        };

        match col_ix {
            RULES_NAME => cell(
                div()
                    .truncate()
                    .font_weight(FontWeight::BOLD)
                    .child(name.clone()),
            ),
            RULES_COUNT => cell(
                div()
                    .child(match entries {
                        Some(count) => SharedString::from(count.to_string()),
                        None => "—".into(),
                    })
                    .text_color(if entries.is_none() {
                        muted
                    } else {
                        cx.theme().foreground
                    }),
            ),
            RULES_REFS => cell(
                div()
                    .truncate()
                    .text_color(muted)
                    .child(SharedString::from(refs_text)),
            ),
            RULES_ACTIONS => cell(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        // 原文：web 形态是一个指向 /api/rules/<name> 的链接，桌面没有
                        // 浏览器可跳 —— 读同一份正文，开只读弹窗。
                        Button::new(("rules-view", row_ix))
                            .xsmall()
                            .ghost()
                            .icon(Icon::new(IconName::Eye))
                            .accessibility_label(format!("查看规则集 {name} 原文"))
                            .tooltip("查看原文")
                            .on_click({
                                let view = self.view.clone();
                                let name = name.clone();
                                move |_, window, cx| {
                                    let name = name.clone();
                                    let _ = view
                                        .update(cx, |view, cx| view.show_text(name, window, cx));
                                }
                            }),
                    )
                    .child(
                        Button::new(("rules-delete", row_ix))
                            .xsmall()
                            .ghost()
                            .icon(Icon::new(IconName::Delete))
                            .text_color(danger)
                            .accessibility_label(format!("删除规则集 {name}"))
                            .tooltip("删除")
                            .on_click({
                                let view = self.view.clone();
                                let name = name.clone();
                                move |_, window, cx| {
                                    let name = name.clone();
                                    let _ = view.update(cx, |view, cx| {
                                        view.confirm_delete(name, window, cx)
                                    });
                                }
                            }),
                    ),
            ),
            _ => div().into_any_element(),
        }
    }
}

/// 规则集目录屏。
pub struct RulesCatalog {
    table: Entity<TableState<RulesTable>>,
    search: Entity<InputState>,
    load: Option<Task<()>>,
    inflight: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl RulesCatalog {
    pub fn new(
        envs: Entity<TableState<EnvTable>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let view = cx.weak_entity();
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("搜索规则集")
                .clean_on_escape()
        });
        let table = cx.new(|cx| TableState::new(RulesTable::new(envs, view), window, cx));

        // 搜索框：每次输入重算可见行（过滤是纯函数，不查后端）。
        let search_sub = cx.subscribe_in(&search, window, {
            let table = table.clone();
            let search = search.clone();
            move |_, _, event, _, cx| {
                let InputEvent::Change = event else { return };
                let query = search.read(cx).value().to_string();
                let changed = table.update(cx, |state, cx| {
                    let changed = state.delegate_mut().set_query(&query);
                    if changed {
                        state.refresh(cx);
                    }
                    changed
                });
                if changed {
                    cx.notify();
                }
            }
        });

        let mut this = Self {
            table,
            search,
            load: None,
            inflight: Vec::new(),
            _subscriptions: vec![search_sub],
        };
        this.reload(window, cx);
        this
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let admin = cx.global::<Host>().admin().clone();
        let table = self.table.clone();
        let task = hop(
            window,
            cx,
            async move { admin.rules_list().map_err(|error| error.message) },
            move |_, outcome, _, cx| {
                let changed = table.update(cx, |state, cx| {
                    let changed = state.delegate_mut().set_rows(outcome);
                    if changed {
                        state.refresh(cx);
                    }
                    changed
                });
                if changed {
                    cx.notify();
                }
            },
        );
        // Task 必须被持有：drop 即取消，所以挂回字段而不是就地丢弃。
        self.load = Some(task);
    }

    /// 读正文并开只读弹窗（读失败写进表的错误条，不开空弹窗）。
    fn show_text(&mut self, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let admin = cx.global::<Host>().admin().clone();
        let table = self.table.clone();
        let query = name.to_string();
        let task = hop(
            window,
            cx,
            async move { admin.rules_read(&query).map_err(|error| error.message) },
            move |this, outcome, window, cx| match outcome {
                Ok(text) => this.open_text_dialog(name, text, window, cx),
                Err(message) => {
                    let changed = table.update(cx, |state, cx| {
                        let changed = state.delegate_mut().set_error(message);
                        if changed {
                            state.refresh(cx);
                        }
                        changed
                    });
                    if changed {
                        cx.notify();
                    }
                }
            },
        );
        self.inflight.push(task);
    }

    fn open_text_dialog(
        &mut self,
        name: SharedString,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mono = Font {
            family: cx.theme().mono_font_family.clone(),
            ..Default::default()
        };
        window.open_dialog(cx, move |dialog, _, cx| {
            dialog
                .title(format!("规则集 {name} · 原文"))
                // 只读查看：只要一个「关闭」。普通 Dialog 不会自己画按钮，
                // 页脚必须自己给（`button_props` 只装回调，不画钮）。
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().trigger(|button| button.label("关闭"))),
                )
                .child(
                    div()
                        .id("rules-text")
                        .max_h(px(360.))
                        .overflow_y_scroll()
                        // 正文要保留换行：GPUI 的 Div 没有 whitespace_pre，所以按行拆成
                        // 子元素（空行填一个空格，否则那一行会被压成 0 高）。
                        .child(
                            div()
                                .font(mono.clone())
                                .text_sm()
                                .children(text.lines().map(|line| {
                                    div().child(SharedString::from(if line.is_empty() {
                                        " "
                                    } else {
                                        line
                                    }))
                                })),
                        )
                        .child(
                            div()
                                .pt_2()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("只读。要改正文，重新导入同名规则集即可（覆盖式）。"),
                        ),
                )
        });
    }

    fn confirm_delete(&mut self, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.weak_entity();
        let refs = self
            .table
            .read(cx)
            .delegate()
            .envs
            .read(cx)
            .delegate()
            .referenced_by_rules(&name);
        let consequence = if refs.is_empty() {
            "当前没有环境引用它。".to_string()
        } else {
            format!(
                "正在引用它的环境（{}）会立刻失去这份改写规则，标记为「规则缺失」。",
                refs.join("、")
            )
        };
        let title = format!("删除规则集 {name}？");
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let view = view.clone();
            let name = name.clone();
            let consequence = consequence.clone();
            dialog
                .confirm()
                .title(title.clone())
                .description(format!("{consequence}此操作不可撤销。"))
                .ok_text("删除")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let name = name.clone();
                    view.update(cx, |view, cx| view.remove(name, window, cx))
                        .is_ok()
                })
        });
    }

    fn remove(&mut self, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let admin = cx.global::<Host>().admin().clone();
        let table = self.table.clone();
        let query = name.to_string();
        let task = hop(
            window,
            cx,
            async move { admin.rules_delete(&query).map_err(|error| error.message) },
            move |this, outcome, window, cx| {
                if let Err(message) = outcome {
                    table.update(cx, |state, cx| {
                        state.delegate_mut().set_error(message);
                        state.refresh(cx);
                    });
                }
                this.reload(window, cx);
            },
        );
        self.inflight.push(task);
    }

    /// 导入规则集：名字 + hosts 正文（正文解析与拒绝都在服务端，见 spec/rules.md）。
    fn open_import_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.weak_entity();
        let name_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("小写字母开头，限 a-z 0-9 _ -"));
        let text_input =
            cx.new(|cx| TextareaState::new(window, cx).placeholder("127.0.0.1 api.example.com"));
        window.open_dialog(cx, move |dialog, _window, cx| {
            let view = view.clone();
            let name_input = name_input.clone();
            let text_input = text_input.clone();
            dialog
                .title("导入规则集")
                .child(
                    Field::new()
                        .label("名字")
                        .child(Input::new(&name_input).w_full()),
                )
                .child(Field::new().label("hosts 正文").child(
                    // 正文是多行的：默认单行高会把"每行一条"的编辑体验做没了。
                    Textarea::new(&text_input).w_full().h(px(140.)),
                ))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "每行一条 `ip 域名`；导入即覆盖同名规则集，解析与拒绝由服务端裁决。",
                        ),
                )
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().trigger(|button| button.label("取消")))
                        .child(
                            DialogAction::new()
                                .child(Button::new("rules-import-ok").primary().label("导入")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let name = name_input.read(cx).value().trim().to_string();
                    let text = text_input.read(cx).value().to_string();
                    // 形状先拦一道（省一次往返），裁决仍在服务端。
                    if name.is_empty() || !valid_rule_name(&name) || text.trim().is_empty() {
                        return false;
                    }
                    let _ = view.update(cx, |view, cx| view.import(name, text, window, cx));
                    true
                })
        });
    }

    fn import(&mut self, name: String, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let admin = cx.global::<Host>().admin().clone();
        let table = self.table.clone();
        let task = hop(
            window,
            cx,
            async move {
                admin
                    .import_rules(&name, &text)
                    .map(|_| ())
                    .map_err(|error| error.message)
            },
            move |this, outcome, window, cx| {
                if let Err(message) = outcome {
                    table.update(cx, |state, cx| {
                        state.delegate_mut().set_error(message);
                        state.refresh(cx);
                    });
                }
                this.reload(window, cx);
            },
        );
        self.inflight.push(task);
    }
}

impl Render for RulesCatalog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let delegate = self.table.read(cx).delegate();
        let error = delegate.error();
        let rows = delegate.rows_count(cx);
        let total = delegate.rows.len();
        let filtered_out = delegate.filtered_out();
        let view = cx.weak_entity();
        let status = match filtered_out {
            true => format!("{rows} / {total} 个规则集 · 无匹配项"),
            false if total != rows => format!("{rows} / {total} 个规则集"),
            false => format!("{total} 个规则集"),
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap_2()
            .p_4()
            .child(
                div()
                    .id("rules-toolbar")
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w_64().child(Input::new(&self.search)))
                    // 每屏只一个主操作，放在最右。
                    .child(div().flex_1())
                    .child(
                        Button::new("rules-import")
                            .xsmall()
                            .primary()
                            .icon(Icon::new(IconName::Plus))
                            .label("导入规则集")
                            .tooltip("导入规则集")
                            .on_click(move |_, window, cx| {
                                let _ =
                                    view.update(cx, |view, cx| view.open_import_dialog(window, cx));
                            }),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(status),
            )
            .children(error.map(|message| {
                div()
                    .id("rules-error")
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(message)
            }))
            .child(div().flex_1().min_h_0().child(DataTable::new(&self.table)))
    }
}

/// 规则集名的形状规则（与服务端同一判据；服务端仍是裁决方）。
fn valid_rule_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

// --------------------------------------------------------------------------- #
// 上游代理目录
// --------------------------------------------------------------------------- #

const PROXY_NAME: usize = 0;
const PROXY_ADDRESS: usize = 1;
const PROXY_AUTH: usize = 2;
const PROXY_REFS: usize = 3;
const PROXY_ACTIONS: usize = 4;

/// 上游代理表。
pub struct ProxiesTable {
    columns: Vec<Column>,
    rows: Vec<ProxyView>,
    loading: bool,
    error: Option<SharedString>,
    view: WeakEntity<ProxiesCatalog>,
}

impl ProxiesTable {
    fn new(view: WeakEntity<ProxiesCatalog>) -> Self {
        let columns = [
            ("name", "代理", 180.),
            ("address", "地址", 200.),
            ("auth", "鉴权", 90.),
            ("refs", "被哪些环境引用", 300.),
            ("actions", "操作", 130.),
        ]
        .into_iter()
        .map(|(key, name, width)| Column::new(key, name).resizable(true).width(px(width)))
        .collect();
        Self {
            columns,
            rows: Vec::new(),
            loading: true,
            error: None,
            view,
        }
    }

    fn set_rows(&mut self, outcome: Result<Vec<ProxyView>, String>) -> bool {
        self.loading = false;
        match outcome {
            Ok(rows) => {
                let changed = rows != self.rows;
                self.rows = rows;
                self.error = None;
                changed
            }
            Err(message) => {
                let changed = self.error.as_deref() != Some(message.as_str());
                self.error = Some(message.into());
                changed
            }
        }
    }

    /// 把一条错误写进表（与环境屏同一手法：状态归表持有）。
    fn set_error(&mut self, message: String) -> bool {
        let changed = self.error.as_deref() != Some(message.as_str());
        self.error = Some(message.into());
        self.loading = false;
        changed
    }

    pub fn error(&self) -> Option<SharedString> {
        self.error.clone()
    }
}

impl TableDelegate for ProxiesTable {
    fn columns_count(&self, _: &App) -> usize {
        5
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.columns[col_ix].clone()
    }

    fn loading(&self, _: &App) -> bool {
        self.loading
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(proxy) = self.rows.get(row_ix).cloned() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let referenced = !proxy.references.is_empty();
        let refs_text = if referenced {
            proxy.references.join("、")
        } else {
            "无环境引用".to_string()
        };

        match col_ix {
            PROXY_NAME => cell(
                div()
                    .truncate()
                    .font_weight(FontWeight::BOLD)
                    .child(proxy.name.clone()),
            ),
            PROXY_ADDRESS => cell(
                div()
                    .truncate()
                    .child(SharedString::from(format!("{}:{}", proxy.host, proxy.port))),
            ),
            PROXY_AUTH => cell(
                div()
                    .text_color(muted)
                    // 只说有没有：凭据永不回显。
                    .child(SharedString::from(if proxy.has_auth {
                        "已启用"
                    } else {
                        "无"
                    })),
            ),
            PROXY_REFS => cell(
                div()
                    .truncate()
                    .text_color(muted)
                    .child(SharedString::from(refs_text)),
            ),
            PROXY_ACTIONS => cell(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        Button::new(("proxy-edit", row_ix))
                            .xsmall()
                            .ghost()
                            .icon(Icon::new(IconName::Settings))
                            .accessibility_label(format!("编辑上游代理 {}", proxy.name))
                            .tooltip("编辑")
                            .on_click({
                                let view = self.view.clone();
                                let proxy = proxy.clone();
                                move |_, window, cx| {
                                    let proxy = proxy.clone();
                                    let _ = view.update(cx, |view, cx| {
                                        view.open_put_dialog(Some(proxy), window, cx)
                                    });
                                }
                            }),
                    )
                    .child(
                        Button::new(("proxy-delete", row_ix))
                            .xsmall()
                            .ghost()
                            .icon(Icon::new(IconName::Delete))
                            .text_color(danger)
                            .accessibility_label(format!("删除上游代理 {}", proxy.name))
                            .tooltip(if referenced {
                                "先解除环境引用才能删"
                            } else {
                                "删除"
                            })
                            .disabled(referenced)
                            .on_click({
                                let view = self.view.clone();
                                let name = SharedString::from(proxy.name.clone());
                                move |_, window, cx| {
                                    let name = name.clone();
                                    let _ = view.update(cx, |view, cx| {
                                        view.confirm_delete(name, window, cx)
                                    });
                                }
                            }),
                    ),
            ),
            _ => div().into_any_element(),
        }
    }
}

/// 上游代理目录屏。
pub struct ProxiesCatalog {
    table: Entity<TableState<ProxiesTable>>,
    inflight: Vec<Task<()>>,
}

impl ProxiesCatalog {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let view = cx.weak_entity();
        let table = cx.new(|cx| TableState::new(ProxiesTable::new(view), window, cx));
        let mut this = Self {
            table,
            inflight: Vec::new(),
        };
        this.reload(window, cx);
        this
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let admin = cx.global::<Host>().admin().clone();
        let table = self.table.clone();
        let task = hop(
            window,
            cx,
            async move { admin.proxy_list().map_err(|error| error.message) },
            move |_, outcome, _, cx| {
                let changed = table.update(cx, |state, cx| {
                    let changed = state.delegate_mut().set_rows(outcome);
                    if changed {
                        state.refresh(cx);
                    }
                    changed
                });
                if changed {
                    cx.notify();
                }
            },
        );
        self.inflight.push(task);
    }

    fn confirm_delete(&mut self, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.weak_entity();
        let title = format!("删除上游代理 {name}？");
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let view = view.clone();
            let name = name.clone();
            dialog
                .confirm()
                .title(title.clone())
                .description("引用它的环境会立刻改为直连（不重启即生效）。此操作不可撤销。")
                .ok_text("删除")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let name = name.clone();
                    view.update(cx, |view, cx| view.remove(name, window, cx))
                        .is_ok()
                })
        });
    }

    fn remove(&mut self, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let admin = cx.global::<Host>().admin().clone();
        let table = self.table.clone();
        let query = name.to_string();
        let task = hop(
            window,
            cx,
            async move { admin.proxy_delete(&query).map_err(|error| error.message) },
            move |this, outcome, window, cx| {
                if let Err(message) = outcome {
                    table.update(cx, |state, cx| {
                        state.delegate_mut().set_error(message);
                        state.refresh(cx);
                    });
                }
                this.reload(window, cx);
            },
        );
        self.inflight.push(task);
    }

    /// 新建 / 编辑上游代理。`existing` 只用来预填 host/port —— 凭据不回显，
    /// 所以两栏永远从空开始，而"空"在服务端就是**不配置鉴权**。
    fn open_put_dialog(
        &mut self,
        existing: Option<ProxyView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.weak_entity();
        let has_auth = existing.as_ref().is_some_and(|proxy| proxy.has_auth);
        let title = match &existing {
            Some(proxy) => format!("编辑上游代理 {}", proxy.name),
            None => "新建上游代理".to_string(),
        };
        let name = cx.new(|cx| {
            let state = InputState::new(window, cx).placeholder("up-lab");
            match &existing {
                Some(proxy) => state.default_value(proxy.name.as_str()),
                None => state,
            }
        });
        let host = cx.new(|cx| {
            let state = InputState::new(window, cx).placeholder("10.0.0.1");
            match &existing {
                Some(proxy) => state.default_value(proxy.host.as_str()),
                None => state,
            }
        });
        let port = cx.new(|cx| {
            let state = InputState::new(window, cx).placeholder("8888");
            match &existing {
                Some(proxy) => state.default_value(proxy.port.to_string()),
                None => state,
            }
        });
        let user = cx.new(|cx| InputState::new(window, cx).placeholder("用户名（可选）"));
        // 密码打码输入（`masked` 顺带把它挡在剪贴板选择之外）：桌面没有浏览器
        // 的 type=password，打码要自己声明。
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("密码（可选）")
                .masked(true)
        });
        let note = if has_auth {
            "鉴权两栏必须同时填写，都不能含「:」或空白。当前已配置鉴权：留空提交会清除它（凭据只进不出，不回显）。"
        } else {
            "鉴权两栏必须同时填写，都不能含「:」或空白。留空表示不配置鉴权。凭据只进不出。"
        };
        window.open_dialog(cx, move |dialog, _window, cx| {
            let view = view.clone();
            let (name, host, port, user, password) = (
                name.clone(),
                host.clone(),
                port.clone(),
                user.clone(),
                password.clone(),
            );
            dialog
                .title(title.clone())
                .child(Field::new().label("名字").child(Input::new(&name).w_full()))
                .child(
                    Field::new().label("地址").child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(Input::new(&host).w_1_2())
                            .child(Input::new(&port).w_24()),
                    ),
                )
                .child(
                    Field::new().label("访问鉴权").child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(Input::new(&user).w_1_2())
                            .child(Input::new(&password).w_1_2()),
                    ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(note),
                )
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().trigger(|button| button.label("取消")))
                        .child(DialogAction::new().child(
                            Button::new("proxy-put-ok").primary().label(if has_auth {
                                "保存（将清除鉴权）"
                            } else {
                                "保存"
                            }),
                        )),
                )
                .on_ok(move |_, window, cx| {
                    let name = name.read(cx).value().trim().to_string();
                    let host = host.read(cx).value().trim().to_string();
                    let port = port.read(cx).value().trim().to_string();
                    let user = user.read(cx).value().to_string();
                    let password = password.read(cx).value().to_string();
                    // 名字/地址/端口缺一不提交；鉴权的"半填"交给服务端报，
                    // 因为它的错误文案已经把 all-or-nothing 说清楚了。
                    if name.is_empty() || host.is_empty() || port.is_empty() {
                        return false;
                    }
                    let Ok(port) = port.parse::<u16>() else {
                        return false;
                    };
                    let body = build_proxy_body(&name, &host, port, &user, &password);
                    let _ = view.update(cx, |view, cx| view.put(body, window, cx));
                    true
                })
        });
    }

    fn put(&mut self, body: serde_json::Value, window: &mut Window, cx: &mut Context<Self>) {
        let admin = cx.global::<Host>().admin().clone();
        let table = self.table.clone();
        let task = hop(
            window,
            cx,
            async move {
                admin
                    .proxy_put(&body)
                    .map(|_| ())
                    .map_err(|error| error.message)
            },
            move |this, outcome, window, cx| {
                if let Err(message) = outcome {
                    table.update(cx, |state, cx| {
                        state.delegate_mut().set_error(message);
                        state.refresh(cx);
                    });
                }
                this.reload(window, cx);
            },
        );
        self.inflight.push(task);
    }
}

impl Render for ProxiesCatalog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let delegate = self.table.read(cx).delegate();
        let error = delegate.error();
        let rows = delegate.rows.len();
        let view = cx.weak_entity();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap_2()
            .p_4()
            .child(
                div()
                    .id("proxies-toolbar")
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1())
                    .child(
                        Button::new("proxy-put")
                            .xsmall()
                            .primary()
                            .icon(Icon::new(IconName::Plus))
                            .label("新建上游代理")
                            .tooltip("新建上游代理")
                            .on_click(move |_, window, cx| {
                                let _ = view
                                    .update(cx, |view, cx| view.open_put_dialog(None, window, cx));
                            }),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{rows} 个上游代理")),
            )
            .children(error.map(|message| {
                div()
                    .id("proxies-error")
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(message)
            }))
            .child(div().flex_1().min_h_0().child(DataTable::new(&self.table)))
    }
}

/// 组装 `proxy_put` 请求体：只给服务端认得的字段。
///
/// `port` 必须是数字（`upstream_proxy.port must be a positive integer`），
/// `user`/`password` 要么成对给、要么干脆不给 —— 空串会被判成非法凭据。
fn build_proxy_body(
    name: &str,
    host: &str,
    port: u16,
    user: &str,
    password: &str,
) -> serde_json::Value {
    let mut body = serde_json::json!({ "name": name, "host": host, "port": port });
    if !user.is_empty() && !password.is_empty() {
        body["user"] = serde_json::Value::String(user.to_string());
        body["password"] = serde_json::Value::String(password.to_string());
    }
    body
}

/// 单元格外壳（与环境屏同一写法：吃掉列宽 + 超长截断）。
fn cell(child: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .child(child)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{build_proxy_body, valid_rule_name};

    #[test]
    fn rule_names_follow_the_server_shape() {
        assert!(valid_rule_name("corp"));
        assert!(valid_rule_name("up_lab-2"));
        // 首字符必须是小写字母：数字、下划线、大写都不行。
        assert!(!valid_rule_name("2corp"));
        assert!(!valid_rule_name("_corp"));
        assert!(!valid_rule_name("Corp"));
        assert!(!valid_rule_name(""));
        // 正文里的空格与中文都不属于名字字符集。
        assert!(!valid_rule_name("cor p"));
    }

    #[test]
    fn proxy_body_carries_port_as_a_number() {
        let body = build_proxy_body("up-lab", "10.0.0.1", 8888, "", "");
        assert_eq!(body["port"], serde_json::json!(8888));
        // 不填凭据 = 字段根本不出现（空串会被服务端判成非法凭据）。
        assert!(body.get("user").is_none());
        assert!(body.get("password").is_none());
    }

    #[test]
    fn proxy_credentials_are_all_or_nothing() {
        let both = build_proxy_body("up-lab", "10.0.0.1", 8888, "u", "p");
        assert_eq!(both["user"], serde_json::json!("u"));
        assert_eq!(both["password"], serde_json::json!("p"));
        // 半填：交给服务端报 all-or-nothing，桌面不自己发明第三种语义。
        let half = build_proxy_body("up-lab", "10.0.0.1", 8888, "u", "");
        assert!(half.get("user").is_none());
        assert!(half.get("password").is_none());
    }
}

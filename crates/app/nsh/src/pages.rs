//! 三个页面：环境列表（含实例/规则两个子页签与详情抽屉）与证书页。
//! 只组合 `envboard_ui` 的原语；颜色一律经 `tokens.rs` 的具名角色，
//! 业务语义（状态、文案、计数、过滤）一律经 `model`，写操作一律经 `actions`。
//!
//! M3 起列表**不再抬高到编译期**：组件函数只跑一次，行由 `each` 按数据现建，
//! 所以 `filter` / 搜索词 / 快照变化都只是重算一次 items，不重建整页。

use nsh_core::prelude::*;

use envboard_ui::primitives as ui;
use envboard_ui::{font as f, metrics as m, tokens as t};

use crate::actions::{self, Ctx};
use crate::model::{self, Env, Health};
use crate::prefs::ThemeChoice;
use crate::service;

// ---- 列宽 ----------------------------------------------------------------
//
// 归页面而不是设计系统：列宽是这张表的排版决定，换个页面就该换一组数。

const COL_STATUS: f32 = 150.0;
const COL_LISTEN: f32 = 190.0;
const COL_RULES: f32 = 120.0;
const COL_LAST: f32 = 150.0;
const COL_ACTION: f32 = 124.0;

// ---- 环境页 --------------------------------------------------------------

pub fn env_page(
    ctx: Ctx,
    filter: Signal<usize>,
    section: Signal<usize>,
    search: Signal<String>,
) -> impl IntoElement {
    view()
        .flex_1()
        .w_full()
        .h_full()
        .flex_col()
        .child(toolbar(ctx, filter, search))
        .child(sub_tabs(section))
        // 行内错误条：写操作失败后停在这里，位置在内容流里，不是浮层。
        .child(when(
            move || ctx.error.get().is_some(),
            move || {
                ui::error_bar(
                    move || ctx.error_title.get(),
                    ctx.error.get().unwrap_or_default(),
                    move || ctx.error.set(None),
                )
            },
        ))
        .child(
            view()
                .flex_1()
                .w_full()
                .overflow_hidden()
                .bg(|| t::layer())
                .child(when(
                    move || section.get() == 0,
                    move || instance_table(ctx, filter, search),
                ))
                .child(when(move || section.get() == 1, move || rules_table())),
        )
}

fn toolbar(ctx: Ctx, filter: Signal<usize>, search: Signal<String>) -> impl IntoElement {
    // 四个数从同一份数据现算，筛选片与状态条不会互相漂。
    let c = move || model::counts(&ctx.envs.get());
    view()
        .h(px(m::BAR_H))
        .w_full()
        .px(px(16.0))
        .flex_row()
        .items_center()
        .border_b_1()
        .border_color(|| t::divider())
        .bg(|| t::page())
        .child(
            nsh_base::input()
                .placeholder("搜索环境、端口或规则")
                .value(search)
                .on_change(move |text: &str| search.set(text.to_owned()))
                .w(px(300.0))
                .h(px(m::CONTROL_H))
                .px(px(10.0))
                .border_1()
                .border_color(|| t::control_stroke())
                .rounded(px(m::RADIUS))
                .bg(|| t::control_fill())
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_TEXT)
                .text_color(|| t::ink()),
        )
        .child(ui::spacer(12.0))
        .child(ui::pill_live(
            "全部",
            move || c().total,
            move || filter.get() == 0,
            move || filter.set(0),
        ))
        .child(ui::spacer(4.0))
        .child(ui::pill_live(
            "运行中",
            move || c().running,
            move || filter.get() == 1,
            move || filter.set(1),
        ))
        .child(ui::spacer(4.0))
        .child(ui::pill_live(
            "已停止",
            move || c().stopped,
            move || filter.get() == 2,
            move || filter.set(2),
        ))
        .child(ui::spacer(4.0))
        .child(ui::pill_live(
            "需处理",
            move || c().attention,
            move || filter.get() == 3,
            move || filter.set(3),
        ))
        .child(view().flex_1())
        .child(ui::btn_subtle("刷新", move || actions::refresh_now(ctx)))
        .child(ui::spacer(6.0))
        .child(ui::btn_primary("新建环境", move || {
            actions::create(ctx)
        }))
}

fn sub_tabs(section: Signal<usize>) -> impl IntoElement {
    view()
        .h(px(m::TAB_H))
        .w_full()
        .px(px(16.0))
        .flex_row()
        .items_center()
        .border_b_1()
        .border_color(|| t::divider())
        .bg(|| t::layer())
        .child(ui::nav_tab(
            "实例",
            move || section.get() == 0,
            move || section.set(0),
        ))
        .child(ui::nav_tab(
            "规则",
            move || section.get() == 1,
            move || section.set(1),
        ))
        .child(view().flex_1())
        .child(
            text("按名称排序 · 端口自动分配 · 单实例")
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_TEXT)
                .text_color(|| t::ink_tertiary()),
        )
        .child(ui::spacer(16.0))
}

fn table_head() -> impl IntoElement {
    view()
        .h(px(m::HEAD_H))
        .w_full()
        .flex_row()
        .items_center()
        .bg(|| t::page())
        .border_b_1()
        .border_color(|| t::divider())
        .child(view().w(px(m::BAR_W)))
        .child(ui::head_cell_grow("环境"))
        .child(ui::head_cell("状态", COL_STATUS))
        .child(ui::head_cell("监听", COL_LISTEN))
        .child(ui::head_cell("规则", COL_RULES))
        .child(ui::head_cell("最近活动", COL_LAST))
        .child(ui::head_cell("操作", COL_ACTION))
}

fn instance_table(ctx: Ctx, filter: Signal<usize>, search: Signal<String>) -> impl IntoElement {
    // 10k 行不能一次全造（G3 量过：全量铺开 1126 MB、首帧 2.2 s）。NSH 的
    // `virtual_list` 只造视口附近的行、滚动时把一个格挪给另一行，是旧 P2
    // 探针 71 MB 的同一套机制。
    //
    // 过滤结果先算一次（`computed`）：否则每一格每次取值都要把 10k 条过滤、
    // 克隆一遍，比不虚拟化还慢。
    let rows: Memo<Vec<Env>> =
        computed(move || model::filtered(&ctx.envs.get(), filter.get(), &search.get()));
    view()
        .flex_col()
        .flex_1()
        .w_full()
        .h_full()
        .child(table_head())
        .child(
            nsh_core::element::virtual_list(
                move || rows.with(|r| r.len()),
                move |at| env_row_at(at, rows, ctx),
            )
            .scrollbar(false)
            .estimated_item_size(px(m::ROW_H))
            .empty(empty_state(ctx, filter, search))
            .flex_1()
            .w_full(),
        )
}

/// 列表为空时不留一片空白：分清“一个环境都没有”和“这条筛选/搜索没有命中”。
fn empty_state(ctx: Ctx, filter: Signal<usize>, search: Signal<String>) -> impl IntoElement {
    view()
        .w_full()
        .h(px(120.0))
        .flex_col()
        .items_center()
        .justify_center()
        .child(
            text(move || {
                if ctx.envs.get().is_empty() {
                    if ctx.error.get().is_some() {
                        // 宿主没起来时“空”是假的：别让一句“还没有环境”把人带偏。
                        "宿主尚未就绪，列表为空不是没有环境 —— 原因在上方错误条里。".to_owned()
                    } else {
                        "还没有环境：点右上角「新建环境」建第一个。".to_owned()
                    }
                } else if !search.get().trim().is_empty() {
                    format!("没有匹配「{}」的环境", search.get().trim())
                } else {
                    format!("「{}」筛选下没有环境", filter_name(filter.get()))
                }
            })
            .text_size(px(f::CAPTION))
            .font_family(f::FONT_TEXT)
            .text_color(|| t::ink_tertiary()),
        )
        .child(ui::vspacer(8.0))
        .child(when(
            move || !ctx.envs.get().is_empty(),
            move || {
                ui::btn_subtle("清除筛选", move || {
                    filter.set(0);
                    search.set(String::new());
                })
            },
        ))
}

fn filter_name(filter: usize) -> &'static str {
    match filter {
        1 => "运行中",
        2 => "已停止",
        3 => "需处理",
        _ => "全部",
    }
}

/// 虚拟列表的一行：格子的**结构**造一次，行换了只重跑读 `at` 的绑定。
fn env_row_at(at: Position, rows: Memo<Vec<Env>>, ctx: Ctx) -> impl IntoElement {
    let health = move || {
        rows.with(|r| {
            r.get(at.index())
                .map(|e| e.health)
                .unwrap_or(Health::Failed)
        })
    };
    let name = move || {
        rows.with(|r| {
            r.get(at.index())
                .map(|e| e.name.clone())
                .unwrap_or_default()
        })
    };
    let cells = vec![
        ui::cell_grow(name).into_element(),
        model::status_cell_live_for(health, COL_STATUS).into_element(),
        ui::mono_cell(
            move || {
                rows.with(|r| {
                    r.get(at.index())
                        .map(|e| e.listen.clone())
                        .unwrap_or_default()
                })
            },
            COL_LISTEN,
        )
        .into_element(),
        ui::quiet_cell(
            move || {
                rows.with(|r| {
                    r.get(at.index())
                        .map(|e| e.rules.clone())
                        .unwrap_or_default()
                })
            },
            COL_RULES,
        )
        .into_element(),
        // `EnvView` 没有“最近活动”字段：不编时间，代之以健康原因（有就写，没有写 —）。
        ui::quiet_cell(
            move || {
                rows.with(|r| {
                    r.get(at.index())
                        .map(|e| e.reason.clone().unwrap_or_else(|| "—".to_owned()))
                        .unwrap_or_else(|| "—".to_owned())
                })
            },
            COL_LAST,
        )
        .into_element(),
        action_cell(at, rows, ctx).into_element(),
    ];
    let selected = ctx.selected;
    ui::row_shell(
        move || {
            let n = rows.with(|r| {
                r.get(at.index())
                    .map(|e| e.name.clone())
                    .unwrap_or_default()
            });
            selected.get().as_deref() == Some(n.as_str())
        },
        move || {
            actions::open_drawer(
                ctx,
                rows.with(|r| {
                    r.get(at.index())
                        .map(|e| e.name.clone())
                        .unwrap_or_default()
                }),
            )
        },
    )(cells)
}

fn action_cell(at: Position, rows: Memo<Vec<Env>>, ctx: Ctx) -> impl IntoElement {
    let health = move || {
        rows.with(|r| {
            r.get(at.index())
                .map(|e| e.health)
                .unwrap_or(Health::Failed)
        })
    };
    view()
        .w(px(COL_ACTION))
        .flex_row()
        .items_center()
        .pl(px(6.0))
        .child(ui::btn_text_live(
            move || health().action().to_owned(),
            move || match health() {
                Health::Running => (t::ink_secondary)(),
                _ => (t::accent_ink)(),
            },
            move || {
                actions::act(
                    ctx,
                    rows.with(|r| {
                        r.get(at.index())
                            .map(|e| e.name.clone())
                            .unwrap_or_default()
                    }),
                    health(),
                )
            },
        ))
        .child(ui::spacer(2.0))
        .child(ui::btn_text("⋯", t::ink_tertiary, move || {
            actions::open_drawer(
                ctx,
                rows.with(|r| {
                    r.get(at.index())
                        .map(|e| e.name.clone())
                        .unwrap_or_default()
                }),
            )
        }))
        .child(ui::spacer(4.0))
}

// ---- 规则子页 ------------------------------------------------------------
//
// M3 不动规则子页：规则集读写属于 M4，这里仍是静态样张（6 条，覆盖生效/未生效）。

struct Rule {
    name: &'static str,
    matcher: &'static str,
    action: &'static str,
    live: bool,
}

static RULES: [Rule; 6] = [
    Rule {
        name: "api-rewrite",
        matcher: "host api.local",
        action: "rewrite → 127.0.0.1:8080",
        live: true,
    },
    Rule {
        name: "mock-json",
        matcher: "path /v1/user",
        action: "respond 200 · fixture",
        live: true,
    },
    Rule {
        name: "slow-3g",
        matcher: "all",
        action: "delay 300ms",
        live: true,
    },
    Rule {
        name: "block-tracker",
        matcher: "host *.tracker",
        action: "abort",
        live: true,
    },
    Rule {
        name: "staging-cors",
        matcher: "host staging.local",
        action: "header + allow-origin",
        live: false,
    },
    Rule {
        name: "legacy-tls",
        matcher: "port 8443",
        action: "rewrite → http",
        live: false,
    },
];

fn rules_table() -> impl IntoElement {
    view()
        .flex_col()
        .child(
            view()
                .h(px(m::HEAD_H))
                .w_full()
                .flex_row()
                .items_center()
                .bg(|| t::page())
                .border_b_1()
                .border_color(|| t::divider())
                .child(view().w(px(m::BAR_W)))
                .child(ui::head_cell_grow("规则"))
                .child(ui::head_cell("匹配", 300.0))
                .child(ui::head_cell("动作", 300.0))
                .child(ui::head_cell("状态", 120.0)),
        )
        .child(view().flex_col().children(RULES.iter().map(|r| {
            view()
                .h(px(m::ROW_H))
                .w_full()
                .flex_row()
                .items_center()
                .border_b_1()
                .border_color(|| t::divider())
                .bg(|| t::layer())
                .hover(|s| s.bg(|| t::hover_fill()))
                .child(
                    view()
                        .w(px(m::BAR_W))
                        .h(px(m::ROW_H))
                        .bg(move || if r.live { t::layer() } else { t::caution() }),
                )
                .child(ui::cell_grow(r.name))
                .child(ui::mono_cell(r.matcher, 300.0))
                .child(ui::mono_cell(r.action, 300.0))
                .child(
                    view()
                        .w(px(120.0))
                        .flex_row()
                        .items_center()
                        .pl(px(12.0))
                        .child(
                            text(if r.live { "已生效" } else { "未生效" })
                                .text_size(px(f::CAPTION))
                                .font_family(f::FONT_TEXT)
                                .text_color(move || {
                                    if r.live {
                                        t::ink_secondary()
                                    } else {
                                        t::caution_ink()
                                    }
                                }),
                        ),
                )
        })))
}

// ---- 详情抽屉 ------------------------------------------------------------

/// 抽屉由 `shell` 在 `drawer.get()` 为真时才挂上；这里假定选中的环境还在，
/// 环境被删（或快照里没了）就什么也不画。
pub fn drawer(ctx: Ctx) -> impl IntoElement {
    let name = ctx.selected.get().unwrap_or_default();
    match ctx.envs.get().into_iter().find(|e| e.name == name) {
        Some(e) => drawer_inner(ctx, e).into_element(),
        None => view().into_element(),
    }
}

fn drawer_inner(ctx: Ctx, e: Env) -> impl IntoElement {
    let name = signal(e.name.clone());
    let open = ctx.drawer;
    view()
        .absolute()
        .top(px(0.0))
        .right(px(0.0))
        .bottom(px(0.0))
        .z_index(1)
        .w(px(m::DRAWER_W))
        .flex_col()
        .bg(|| t::layer())
        .border_1()
        .border_color(|| t::card_stroke())
        .rounded(px(m::OVERLAY_RADIUS))
        .overflow_hidden()
        .child(drawer_head(e.name.clone(), e.health, open))
        .child(
            view()
                .flex_1()
                .flex_col()
                .p(px(16.0))
                .child(drawer_body(&e)),
        )
        .child(drawer_footer(ctx, name))
}

fn drawer_head(name: String, health: Health, open: Signal<bool>) -> impl IntoElement {
    view()
        .h(px(m::BAR_H))
        .w_full()
        .pl(px(16.0))
        .pr(px(8.0))
        .flex_row()
        .items_center()
        .border_b_1()
        .border_color(|| t::divider())
        .child(
            text(name)
                .text_size(px(f::BODY))
                .font_family(f::FONT_TEXT)
                .text_color(|| t::ink()),
        )
        .child(ui::spacer(8.0))
        .child(model::status_cell_for(health, COL_STATUS))
        .child(view().flex_1())
        .child(ui::btn_text("✕", t::ink_secondary, move || {
            open.set(false)
        }))
}

fn drawer_body(e: &Env) -> impl IntoElement {
    let mut body = view()
        .flex_col()
        .child(ui::section_label("环境"))
        .child(ui::kv("名称", e.name.clone(), false))
        .child(ui::kv("状态", e.health.label(), false))
        .child(ui::kv("监听", e.listen.clone(), true))
        .child(ui::kv(
            "上游",
            e.upstream.clone().unwrap_or_else(|| "直连".to_owned()),
            false,
        ));

    // 故障原因就是真数据里的 `health_reason`，没有就不编一条。
    if let Some(reason) = e.reason.clone() {
        body = body.child(ui::vspacer(10.0)).child(ui::alert(
            ui::Tone::Caution,
            e.health.label(),
            reason,
        ));
    }

    body.child(ui::vspacer(16.0))
        .child(ui::section_label("规则"))
        .child(ui::kv("规则集", e.rules.clone(), false))
        .child(ui::vspacer(10.0))
        .child(ui::section_label("代理地址"))
        .child(
            view()
                .w_full()
                .p(px(10.0))
                .rounded(px(m::RADIUS))
                .border_1()
                .border_color(|| t::control_stroke())
                .bg(|| t::card())
                .child(
                    text(e.proxy_command.clone())
                        .text_size(px(f::CAPTION))
                        .font_family(f::FONT_MONO)
                        .text_color(|| t::ink_secondary()),
                ),
        )
        .child(view().flex_1())
}

fn drawer_footer(ctx: Ctx, name: Signal<String>) -> impl IntoElement {
    let command = ctx
        .envs
        .get()
        .into_iter()
        .find(|e| Some(&e.name) == ctx.selected.get().as_ref())
        .map(|e| e.proxy_command)
        .unwrap_or_default();
    let command = signal(command);
    view()
        .h(px(m::BAR_H))
        .w_full()
        .px(px(16.0))
        .flex_row()
        .items_center()
        .border_t_1()
        .border_color(|| t::divider())
        .child(ui::btn_primary("复制地址", move || {
            actions::copy_proxy(ctx, command.get())
        }))
        .child(ui::spacer(8.0))
        .child(ui::btn_subtle("换个端口", move || {
            actions::reallocate_port(ctx, name.get())
        }))
        .child(view().flex_1())
        // 删除不直接做：先弹确认（全仓唯一不可逆的动作，也是唯一用胭脂填充的按钮）。
        .child(ui::btn_text("删除环境", t::critical, move || {
            actions::ask(ctx, actions::Confirm::delete_env(&name.get()))
        }))
}

// ---- 证书页 --------------------------------------------------------------
//
// M4：数据来自 `service::ca_snapshot()`（共享 CA 的信任探测 + 只读摘要）。
// 这一页只有两件事要说清：**系统信不信它**、**这张证书是什么**；没有“重新签发”
// 入口（轮换是命令行的事），也没有下载向导 —— 只把 PEM 在哪说清楚。
//
// 读证书是 Win32 探测，进页时放工作线程（`actions::refresh_ca`），不占渲染线程。

pub fn cert_page(ctx: Ctx) -> impl IntoElement {
    // 进页读一次。用 `get_untracked` 做守卫：这一句里写信号会触发重跑，
    // 不能同时又订阅（否则“读到 → 重跑 → 再读”会来回抖）。
    if ctx.ca.get_untracked().is_none() && !ctx.ca_missing.get_untracked() {
        actions::refresh_ca(ctx);
    }
    // 两个状态各自挂在一个 `when` 上：组件体里的 `.get()` 是**一次性读**，
    // 数据晚到（这里必然晚到 —— 证书存储探测在工作线程上）时必须靠闭包订阅，
    // 否则会停在“读到之前”的那一帧（踩过：状态条已显示指纹，页面还写着不可用）。
    let card = view()
        .w(px(880.0))
        .flex_col()
        .bg(|| t::card())
        .rounded(px(m::OVERLAY_RADIUS))
        .border_1()
        .border_color(|| t::card_stroke())
        .overflow_hidden()
        .child(
            view()
                .h(px(m::BAR_H))
                .w_full()
                .px(px(16.0))
                .flex_row()
                .items_center()
                .border_b_1()
                .border_color(|| t::divider())
                .child(
                    text("证书")
                        .text_size(px(f::SUBTITLE))
                        .font_family(f::FONT_DISPLAY)
                        .text_color(|| t::ink()),
                )
                .child(ui::spacer(10.0))
                .child(
                    text("共享 CA · 当前用户信任库 · 免管理员")
                        .text_size(px(f::CAPTION))
                        .font_family(f::FONT_TEXT)
                        .text_color(|| t::ink_tertiary()),
                )
                .child(view().flex_1())
                .child(ui::btn_subtle("重新读取", move || actions::refresh_ca(ctx))),
        )
        .child(when(
            move || ctx.ca_missing.get() || ctx.ca.get().is_none(),
            move || cert_unavailable(ctx),
        ))
        .child(when(
            move || !ctx.ca_missing.get() && ctx.ca.get().is_some(),
            move || cert_detail(ctx, ctx.ca.get().unwrap_or_else(model::fixture_ca)),
        ));

    view()
        .flex_1()
        .w_full()
        .h_full()
        .flex_col()
        .items_center()
        .pt(px(24.0))
        .bg(|| t::page())
        // 证书页也要有自己的错误条：安装/卸载失败是**这一页**的操作失败，
        // 不能只飘在环境页上方（那里用户根本看不见）。
        .child(when(move || ctx.error.get().is_some(), move || {
            ui::error_bar(
                move || ctx.error_title.get(),
                ctx.error.get().unwrap_or_default(),
                move || ctx.error.set(None),
            )
        }))
        .child(view().pt(px(10.0)).child(card))
}

/// 读不到 CA：一句实话 + 一个重试，不排一张空表。
fn cert_unavailable(ctx: Ctx) -> AnyElement {
    view()
        .flex_col()
        .p(px(20.0))
        .child(ui::alert(
            ui::Tone::Caution,
            "证书信息暂不可用",
            "读不到共享 CA：要么本宿主没有物化 CA（测试装配），要么系统证书存储探测失败。",
        ))
        .child(ui::vspacer(14.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .child(ui::btn_subtle("重新读取", move || {
                    ctx.ca_missing.set(false);
                    actions::refresh_ca(ctx);
                })),
        ).into_element()
}

fn cert_detail(ctx: Ctx, ca: model::Ca) -> AnyElement {
    let (title, body) = ca.headline();
    let can_install = ca.can_install();
    let can_uninstall = ca.can_uninstall();
    let machine = ca.machine;
    let warning = ca.detail.clone();
    let mut body_col = view()
        .flex_col()
        .p(px(20.0))
        .child(ui::alert(ca.tone(), title, body))
        .child(ui::vspacer(14.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .child(when(move || can_install, move || {
                    ui::btn_primary("安装证书", move || actions::install_ca(ctx))
                }))
                .child(when(move || !can_install, move || {
                    text("已信任：不需要重复安装")
                        .text_size(px(f::CAPTION))
                        .font_family(f::FONT_TEXT)
                        .text_color(|| t::ink_secondary())
                }))
                .child(ui::spacer(8.0))
                .child(when(move || can_uninstall, move || {
                    ui::btn_text("卸载证书", t::critical, move || {
                        actions::ask_uninstall_ca(ctx)
                    })
                }))
                .child(ui::spacer(10.0))
                .child(
                    text("安装与卸载都只作用于当前用户 ROOT 存储（免管理员、可逆）。")
                        .text_size(px(f::CAPTION))
                        .font_family(f::FONT_TEXT)
                        .text_color(|| t::ink_tertiary()),
                ),
        );

    // 探测给出的解释（例如“证书文件缺失”）就摆在按钮下面：有就写，没有不编。
    if let Some(detail) = warning {
        body_col = body_col.child(ui::vspacer(10.0)).child(
            text(detail)
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_TEXT)
                .text_color(|| t::ink_tertiary()),
        );
    }

    // 计算机存储里还有一份：说清“本版本只管理当前用户”，别让人以为没删干净。
    if machine == model::Trust::Installed {
        body_col = body_col.child(ui::vspacer(10.0)).child(ui::alert(
            ui::Tone::Caution,
            "计算机存储里也有一份",
            "本版本只管理当前用户存储，不代删计算机存储里的旧证书；需要时请在系统证书管理器里移除。",
        ));
    }

    body_col
        .child(ui::vspacer(18.0))
        .child(ui::divider())
        .child(ui::vspacer(14.0))
        .child(ui::section_label("证书信息"))
        .child(
            view()
                .flex_row()
                .child(view().flex_1().child(ui::kv("主体", ca.subject(), false)))
                .child(
                    view()
                        .flex_1()
                        .child(ui::kv("签名算法", ca.sig_alg.clone().unwrap_or_else(dash), false)),
                ),
        )
        .child(
            view()
                .flex_row()
                .child(
                    view()
                        .flex_1()
                        .child(ui::kv("序列号", ca.serial_hex.clone().unwrap_or_else(dash), true)),
                )
                .child(
                    view()
                        .flex_1()
                        .child(ui::kv("有效期至", ca.not_after.clone().unwrap_or_else(dash), false)),
                ),
        )
        .child(
            view()
                .flex_row()
                .child(
                    view()
                        .flex_1()
                        .child(ui::kv("有效期起", ca.not_before.clone().unwrap_or_else(dash), false)),
                )
                .child(
                    view()
                        .flex_1()
                        .child(ui::kv("公钥", ca.pubkey.clone().unwrap_or_else(dash), false)),
                ),
        )
        .child(ui::kv("SHA-256 指纹", ca.fingerprint.clone(), true))
        .child(ui::kv("文件路径", ca.cert_path.clone(), true))
        .child(ui::kv(
            "计算机存储",
            ca.machine.label().to_owned(),
            false,
        ))
        .child(ui::vspacer(12.0))
        .child(
            text("其他设备：把这份 PEM 拷到目标机按系统提示安装并信任；本版本不提供下载与向导。")
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_TEXT)
                .text_color(|| t::ink_tertiary()),
        ).into_element()
}

fn dash() -> String {
    "—".to_owned()
}

// ---- 设置框（模态）-------------------------------------------------------
//
// 模态与抽屉的分界只有一条：抽屉是浮层（无遮罩、点外不关），模态是平台级
// 对话框（焦点隔离、Esc 关、遮罩由设计系统画）。平台那层没有长相，所以这里
// 只负责**内容**，壳在 `ui::dialog_shell`。

/// 设置：**只放用户真会改的东西**——外观三档 + 关于。
/// 主题一改就落盘（`ui.json`），与桌面形态共用同一份偏好文件，换框架不换偏好。
pub fn settings_dialog(ctx: Ctx) -> impl IntoElement {
    let shell = ui::dialog_shell(m::DIALOG_W_LG, "设置", "改动立即生效");
    let close = move || ctx.settings.set(false);
    let core = service::core_version()
        .map(|(name, version)| format!("{name} {version}"))
        .unwrap_or_else(dash);

    let body: Vec<AnyElement> = vec![
        ui::section_label("外观").into_element(),
        view()
            .flex_row()
            .items_center()
            .child(ui::choice_live(
                "宣纸",
                move || ctx.theme.get() == ThemeChoice::Light,
                move || actions::set_theme(ctx, ThemeChoice::Light),
            ))
            .child(ui::spacer(6.0))
            .child(ui::choice_live(
                "夜黛",
                move || ctx.theme.get() == ThemeChoice::Dark,
                move || actions::set_theme(ctx, ThemeChoice::Dark),
            ))
            .child(ui::spacer(6.0))
            .child(ui::choice_live(
                "跟随系统",
                move || ctx.theme.get() == ThemeChoice::System,
                move || actions::set_theme(ctx, ThemeChoice::System),
            ))
            .into_element(),
        ui::vspacer(8.0).into_element(),
        text("跟随系统 = 用 Windows 的浅/深设置；三档换的都是同一套令牌，只有色值在换。")
            .text_size(px(f::CAPTION))
            .font_family(f::FONT_TEXT)
            .text_color(|| t::ink_tertiary())
            .into_element(),
        ui::vspacer(16.0).into_element(),
        ui::divider().into_element(),
        ui::vspacer(14.0).into_element(),
        ui::section_label("关于").into_element(),
        ui::kv("版本", format!("{} · NSH 0.6.1", env!("CARGO_PKG_VERSION")), false)
            .into_element(),
        ui::kv("内核", core, false).into_element(),
        ui::kv(
            "CA 目录",
            service::ca_dir()
                .map(|dir| dir.display().to_string())
                .unwrap_or_else(dash),
            true,
        )
        .into_element(),
        ui::kv("界面偏好", "状态目录\\ui.json（与桌面形态共用）".to_owned(), false).into_element(),
    ];

    let footer: Vec<AnyElement> =
        vec![ui::btn_primary("完成", close).into_element()];

    nsh_base::dialog()
        .label("设置")
        // 进场不做动画：`Slide` 的淡入会让抓图（golden）拍在半透明的中间帧上；
        // 桌面上的对话框也不该像手机 sheet 那样从底下滑上来。
        .animation(ModalAnimation::None)
        .on_cancel(close)
        .child(shell(body, footer))
}

// ---- 确认框（模态）-------------------------------------------------------

/// 确认框：**只有删除环境走这里**（不可逆）。
///
/// 可逆动作（卸载证书）也有确认，但确认键不是胭脂、也不印“不可撤销”——
/// 胭脂的面积全仓只留给那一次真正的删除（设计语言 §3）。
pub fn confirm_dialog(ctx: Ctx) -> AnyElement {
    let Some(confirm) = ctx.confirm.get() else {
        return view().into_element();
    };
    let shell = ui::dialog_shell(
        m::DIALOG_W,
        confirm.title.clone(),
        confirm.note.clone(),
    );
    let cancel = move || actions::cancel_confirm(ctx);
    let go = move || actions::run_confirm(ctx);

    let mut body: Vec<AnyElement> = vec![
        text(confirm.body.clone())
            .text_size(px(f::BODY))
            .font_family(f::FONT_TEXT)
            .text_color(|| t::ink_secondary())
            .into_element(),
    ];
    if confirm.irreversible {
        body.push(ui::vspacer(8.0).into_element());
        body.push(
            text("此操作不可撤销：删掉的环境不会自己回来。")
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_TEXT)
                .text_color(|| t::critical())
                .into_element(),
        );
    }

    let footer: Vec<AnyElement> = if confirm.irreversible {
        vec![
            ui::btn_subtle("取消", cancel).into_element(),
            ui::spacer(8.0).into_element(),
            ui::btn_destructive(confirm.confirm_label.clone(), go).into_element(),
        ]
    } else {
        vec![
            ui::btn_subtle("取消", cancel).into_element(),
            ui::spacer(8.0).into_element(),
            ui::btn_primary(confirm.confirm_label.clone(), go).into_element(),
        ]
    };

    nsh_base::dialog()
        .label(confirm.title.clone())
        .animation(ModalAnimation::None)
        .on_cancel(cancel)
        .child(shell(body, footer))
        .into_element()
}

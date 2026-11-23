//! 三个页面：环境列表（含实例/规则两个子页签与详情抽屉）与证书页。
//! 只组合 `envboard_ui` 的原语；颜色一律经 `tokens.rs` 的具名角色，
//! 业务语义（状态、文案）一律经 `model`。

use nsh_core::prelude::*;

use envboard_ui::primitives as ui;
use envboard_ui::{font as f, metrics as m, tokens as t};

use crate::model::{self, Env, Health, ENVS};

// ---- 列宽 ----------------------------------------------------------------
//
// 归页面而不是设计系统：列宽是这张表的排版决定，换个页面就该换一组数。
// 之前它们躺在 `tokens.rs` 里，是样板阶段的权宜。

const COL_STATUS: f32 = 150.0;
const COL_LISTEN: f32 = 190.0;
const COL_RULES: f32 = 120.0;
const COL_LAST: f32 = 150.0;
const COL_ACTION: f32 = 124.0;

// ---- 环境页 --------------------------------------------------------------

pub fn env_page(
    selected: Signal<usize>,
    drawer: Signal<bool>,
    filter: Signal<usize>,
    section: Signal<usize>,
) -> impl IntoElement {
    view()
        .flex_1()
        .w_full()
        .h_full()
        .flex_col()
        .child(toolbar(filter))
        .child(sub_tabs(section))
        .child(
            view()
                .flex_1()
                .w_full()
                .overflow_hidden()
                .bg(|| t::layer())
                // 一个分支一个 `If`：条件翻转时才建/撤那一组行。
                .child(when(
                    move || section.get() == 0 && filter.get() == 0,
                    move || instance_table(0, selected, drawer),
                ))
                .child(when(
                    move || section.get() == 0 && filter.get() == 1,
                    move || instance_table(1, selected, drawer),
                ))
                .child(when(
                    move || section.get() == 0 && filter.get() == 2,
                    move || instance_table(2, selected, drawer),
                ))
                .child(when(
                    move || section.get() == 0 && filter.get() == 3,
                    move || instance_table(3, selected, drawer),
                ))
                .child(when(move || section.get() == 1, move || rules_table())),
        )
}

fn toolbar(filter: Signal<usize>) -> impl IntoElement {
    // 四个数从同一份数据算，筛选片与状态条不会互相漂。
    let c = model::counts(&ENVS);
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
                .value("")
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
        .child(ui::pill("全部", c.total, move || filter.get() == 0, move || filter.set(0)))
        .child(ui::spacer(4.0))
        .child(ui::pill("运行中", c.running, move || filter.get() == 1, move || filter.set(1)))
        .child(ui::spacer(4.0))
        .child(ui::pill("已停止", c.stopped, move || filter.get() == 2, move || filter.set(2)))
        .child(ui::spacer(4.0))
        .child(ui::pill("需处理", c.attention, move || filter.get() == 3, move || filter.set(3)))
        .child(view().flex_1())
        .child(ui::btn_subtle("刷新", || {}))
        .child(ui::spacer(6.0))
        .child(ui::btn_primary("新建环境", || {}))
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
        .child(ui::nav_tab("实例", move || section.get() == 0, move || section.set(0)))
        .child(ui::nav_tab("规则", move || section.get() == 1, move || section.set(1)))
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

fn instance_table(
    filter: usize,
    selected: Signal<usize>,
    drawer: Signal<bool>,
) -> impl IntoElement {
    view()
        .flex_col()
        .child(table_head())
        .child(
            view()
                .flex_col()
                .children(ENVS.iter().filter(move |e| match filter {
                    1 => e.health == Health::Running,
                    2 => e.health == Health::Stopped,
                    // 判据只在 model 里定义一次，筛选片与状态条共用。
                    3 => model::needs_attention(e),
                    _ => true,
                }).map(move |e| env_row(e, selected, drawer))),
        )
}

fn env_row(e: &'static Env, selected: Signal<usize>, drawer: Signal<bool>) -> impl IntoElement {
    let id = e.id;
    let cells = vec![
        ui::cell_grow(e.name).into_element(),
        model::status_cell_for(e.health, COL_STATUS).into_element(),
        ui::mono_cell(e.listen, COL_LISTEN).into_element(),
        ui::quiet_cell(e.rules, COL_RULES).into_element(),
        ui::quiet_cell(e.last, COL_LAST).into_element(),
        action_cell(e).into_element(),
    ];
    ui::row_shell(
        move || selected.get() == id,
        move || {
            selected.set(id);
            drawer.set(true);
        },
    )(cells)
}

fn action_cell(e: &'static Env) -> impl IntoElement {
    let label = match e.health {
        Health::Running => "停止",
        Health::Stopped => "启动",
        _ => "查看",
    };
    let tone: fn() -> nsh_core::element::AdaptiveColor = match e.health {
        Health::Running => t::ink_secondary,
        _ => t::accent_ink,
    };
    view()
        .w(px(COL_ACTION))
        .flex_row()
        .items_center()
        .pl(px(6.0))
        .child(ui::btn_text(label, tone, || {}))
        .child(ui::spacer(2.0))
        .child(ui::btn_text("⋯", t::ink_tertiary, || {}))
        .child(ui::spacer(4.0))
}

// ---- 规则子页 ------------------------------------------------------------

struct Rule {
    name: &'static str,
    matcher: &'static str,
    action: &'static str,
    live: bool,
}

static RULES: [Rule; 6] = [
    Rule { name: "api-rewrite", matcher: "host api.local", action: "rewrite → 127.0.0.1:8080", live: true },
    Rule { name: "mock-json", matcher: "path /v1/user", action: "respond 200 · fixture", live: true },
    Rule { name: "slow-3g", matcher: "all", action: "delay 300ms", live: true },
    Rule { name: "block-tracker", matcher: "host *.tracker", action: "abort", live: true },
    Rule { name: "staging-cors", matcher: "host staging.local", action: "header + allow-origin", live: false },
    Rule { name: "legacy-tls", matcher: "port 8443", action: "rewrite → http", live: false },
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
                        .bg(move || {
                            if r.live {
                                t::layer()
                            } else {
                                t::caution()
                            }
                        }),
                )
                .child(ui::cell_grow(r.name))
                .child(ui::mono_cell(r.matcher, 300.0))
                .child(ui::mono_cell(r.action, 300.0))
                .child(
                    view().w(px(120.0)).flex_row().items_center().pl(px(12.0)).child(
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

pub fn drawer(selected: Signal<usize>, open: Signal<bool>) -> impl IntoElement {
    let e = model::env(selected.get());
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
        .child(drawer_head(e, open))
        .child(view().flex_1().flex_col().p(px(16.0)).child(drawer_body(e)))
        .child(drawer_footer())
}

fn drawer_head(e: &'static Env, open: Signal<bool>) -> impl IntoElement {
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
            text(e.name)
                .text_size(px(f::BODY))
                .font_family(f::FONT_TEXT)
                .text_color(|| t::ink()),
        )
        .child(ui::spacer(8.0))
        .child(model::status_cell_for(e.health, COL_STATUS))
        .child(view().flex_1())
        .child(ui::btn_text("✕", t::ink_secondary, move || open.set(false)))
}

fn drawer_body(e: &'static Env) -> impl IntoElement {
    let mut body = view()
        .flex_col()
        .child(ui::section_label("环境"))
        .child(ui::kv("名称", e.name, false))
        .child(ui::kv("状态", e.health.label(), false))
        .child(ui::kv("监听", e.listen, true))
        .child(ui::kv("最近活动", e.last, false));

    if !matches!(e.health, Health::Running | Health::Stopped) {
        body = body
            .child(ui::vspacer(10.0))
            .child(ui::alert(
                ui::Tone::Caution,
                "旧快照仍在服务",
                "新配置尚未装配成功；客户端仍在被上一份快照处理，重启环境可收敛。",
            ));
    }

    body.child(ui::vspacer(16.0))
        .child(ui::section_label("规则"))
        .child(
            view()
                .w_full()
                .p(px(10.0))
                .rounded(px(m::RADIUS))
                .border_1()
                .border_color(|| t::control_stroke())
                .bg(|| t::card())
                .child(
                    text(move || {
                        format!(
                            "match host {}\nrewrite /api -> /v1/api\nheader + x-envboard: {}",
                            e.listen, e.name
                        )
                    })
                        .text_size(px(f::CAPTION))
                        .font_family(f::FONT_MONO)
                        .text_color(|| t::ink_secondary()),
                ),
        )
        .child(view().flex_1())
}

fn drawer_footer() -> impl IntoElement {
    view()
        .h(px(m::BAR_H))
        .w_full()
        .px(px(16.0))
        .flex_row()
        .items_center()
        .border_t_1()
        .border_color(|| t::divider())
        .child(ui::btn_primary("编辑", || {}))
        .child(ui::spacer(8.0))
        .child(ui::btn_subtle("复制端口", || {}))
        .child(view().flex_1())
        .child(ui::btn_text("删除环境", t::critical, || {}))
}

// ---- 证书页 --------------------------------------------------------------

pub fn cert_page(dark: bool) -> impl IntoElement {
    view()
        .flex_1()
        .w_full()
        .h_full()
        .flex_col()
        .items_center()
        .pt(px(24.0))
        .bg(|| t::page())
        .child(
            view()
                .w(px(880.0))
                .flex_col()
                .bg(|| t::card())
                .rounded(px(m::OVERLAY_RADIUS))
                .border_1()
                .border_color(|| t::card_stroke())
                .overflow_hidden()
                .child(
                    view()
                        .h(px(48.0))
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
                        .child(ui::btn_subtle("重新读取", || {})),
                )
                .child(
                    view()
                        .flex_col()
                        .p(px(20.0))
                        .child(ui::alert(
                            ui::Tone::Caution,
                            "根证书尚未导入系统信任库",
                            "本机客户端会报证书不受信任：安装证书直接写入当前用户 ROOT 存储，可逆。",
                        ))
                        .child(ui::vspacer(14.0))
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .child(ui::btn_primary("安装证书", || {}))
                                .child(ui::spacer(8.0))
                                .child(ui::btn_subtle("卸载证书", || {}))
                                .child(ui::spacer(10.0))
                                .child(
                                    text("未安装，按钮为可用态；已安装时这里换成「已信任」提示条。")
                                        .text_size(px(f::CAPTION))
                                        .font_family(f::FONT_TEXT)
                                        .text_color(|| t::ink_tertiary()),
                                ),
                        )
                        .child(ui::vspacer(18.0))
                        .child(ui::divider())
                        .child(ui::vspacer(14.0))
                        .child(ui::section_label("证书信息"))
                        .child(
                            view()
                                .flex_row()
                                .child(view().flex_1().child(ui::kv("主体", "envboard local CA", false)))
                                .child(view().flex_1().child(ui::kv("签名算法", "ECDSA P-256", false))),
                        )
                        .child(
                            view()
                                .flex_row()
                                .child(view().flex_1().child(ui::kv("序列号", "7A:1F:44:0C:9E:22", true)))
                                .child(view().flex_1().child(ui::kv("有效期至", "2036-11-23", false))),
                        )
                        .child(ui::kv(
                            "SHA-256 指纹",
                            "3F:9C:71:2D:88:04:BE:57:C1:6A:0D:9F:33:10:5E:A2",
                            true,
                        ))
                        .child(ui::kv(
                            "文件路径",
                            r"C:\Users\WPS\AppData\Local\envboard\ca\envboard-ca-cert.pem",
                            true,
                        ))
                        .child(ui::vspacer(12.0))
                        .child(
                            text("其他设备：把这份 PEM 拷到目标机按系统提示安装并信任；本版本不提供下载与向导。")
                                .text_size(px(f::CAPTION))
                                .font_family(f::FONT_TEXT)
                                .text_color(|| t::ink_tertiary()),
                        )
                        .child(ui::vspacer(6.0))
                        .child(
                            text(move || {
                                if dark {
                                    "当前外观：夜黛（深色令牌全套生效）".to_owned()
                                } else {
                                    "当前外观：宣纸（浅色令牌全套生效）".to_owned()
                                }
                            })
                            .text_size(px(f::CAPTION))
                            .font_family(f::FONT_TEXT)
                            .text_color(|| t::ink_tertiary()),
                        ),
                ),
        )
}

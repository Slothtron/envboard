//! 抓包子页（EB-DESIGN-004 v1.4 方案 C：迁、按折衷形态）。
//!
//! 五条边界不由页面自行发明，全部来自 ADR-8：
//!
//! 1. **列表用瘦行**（`capture_rows`）：35k 行整条记录带 body，等于把引擎缓冲再拷一份；
//! 2. **筛选在界面侧**：两列的表头就是两个筛选入口，点开才出现分段面板（列即筛选），
//!    计数与过滤共用 `model::capture` 的同一套判据；
//! 3. **正文纯文本**：不做语法高亮，只重排缩进；**64 KiB 以上只给原文**；
//! 4. **复制跟随当前视图**：美化态复制美化文本、原文态复制原文；
//! 5. **列表是活的**：500ms 一拍增量，按 `request_id` 去重合并；会话换了（实例重启）、
//!    代次换了（手动清空）就把本地行清掉重来。
//!
//! 页面生命周期 = 一次进入：返回环境列表即卸载，再进来从空表重新读。

use std::time::Duration;

use envboard_protocol::{
    CaptureRow, Record, RecordBody, RecordRequest, RecordResponse,
};
use nsh_core::prelude::*;

use envboard_ui::primitives as ui;
use envboard_ui::{font as f, metrics as m, tokens as t};

use crate::actions::{self, Ctx};
use crate::model::body::{take_prefix, Body, Kind, CHUNK};
use crate::model::capture::{self as cap, Counts, ResFilter, StatusFilter};
use crate::model::detail::Detail;
use crate::service;

// 列宽归页面：换一张表就该换一组数。
const COL_ID: f32 = 56.0;
const COL_METHOD: f32 = 64.0;
const COL_STATUS: f32 = 64.0;
const COL_HOST: f32 = 190.0;
const COL_SIZE: f32 = 84.0;
const COL_TYPE: f32 = 96.0;

/// 一次读多少行。批小一点，缓冲锁的持有时间短、每拍的界面工作也分摊得开。
const PAGE: usize = 500;
/// 一拍里最多追几批（首屏 35k 行要追 70 批，靠一帧追完会卡住，所以分拍）。
const PULL_MAX_PAGES: usize = 8;
const POLL_MS: u64 = 500;

/// 抓包会话的元数据（工具栏那条 chip 的全部信息）。
#[derive(Clone, PartialEq, Debug, Default)]
struct Meta {
    session: u64,
    generation: u64,
    captured: u64,
    dropped: u64,
    /// 首拍是否已读到（决定空态说「正在读取」还是「还没有记录」）。
    loaded: bool,
    error: Option<String>,
}

/// 表头点开的那一列。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Column {
    Status,
    Type,
}

/// 一屏详情：完整记录 + 两侧当前要显示的正文文本（美化或原文）。
///
/// 文本在**打开详情时算一次**、切视图时重算一次：它要喂给多行只读框，
/// 每帧现算 64 KiB 的解析 + 重排是白花的。
///
/// 还有两个状态位：
///
/// * `loading` —— 骨架态。点行后先用行里的头部信息把面板撑起来（方法/状态/时刻/地址），
///   页签与正文等详情回来再挂；大记录（要 clone headers 与正文）不再堵住点击这一帧；
/// * `request_rendered` / `response_rendered` —— 已经放进控件的字节数。正文按
///   [`CHUNK`] 一段一段放，超过的部分等「继续加载」；40–64 KiB 不整块压进一次布局。
#[derive(Clone, PartialEq, Debug)]
struct DetailView {
    detail: Detail,
    loading: bool,
    request_shown: String,
    response_shown: String,
    request_rendered: usize,
    response_rendered: usize,
    request_prettyable: bool,
    response_prettyable: bool,
    request_pretty: bool,
    response_pretty: bool,
}

impl DetailView {
    fn build(detail: Detail) -> Self {
        let request_prettyable = detail
            .request_body
            .can_pretty(detail.request_content_type.as_deref());
        let response_prettyable = detail
            .response_body
            .can_pretty(detail.response_content_type.as_deref());
        let mut view = DetailView {
            detail,
            loading: false,
            request_shown: String::new(),
            response_shown: String::new(),
            request_rendered: 0,
            response_rendered: 0,
            request_prettyable,
            response_prettyable,
            request_pretty: request_prettyable,
            response_pretty: response_prettyable,
        };
        view.refresh_texts();
        view
    }

    /// 骨架：只用列表那一行能拿到的字段。地址先不带查询串（行里没有 query），
    /// 详情回来会补上；页签计数与正文都还没有，所以 `loading`。
    fn skeleton(row: &envboard_protocol::CaptureRow) -> Self {
        DetailView {
            detail: Detail {
                request_id: row.request_id,
                time_ms: row.time,
                method: row.method.clone(),
                authority: row.authority.clone(),
                path: row.path.clone(),
                status: row.status,
                ..Detail::default()
            },
            loading: true,
            request_shown: String::new(),
            response_shown: String::new(),
            request_rendered: 0,
            response_rendered: 0,
            request_prettyable: false,
            response_prettyable: false,
            request_pretty: false,
            response_pretty: false,
        }
    }

    fn refresh_texts(&mut self) {
        self.request_shown = shown_text(
            &self.detail.request_body,
            self.detail.request_content_type.as_deref(),
            self.request_pretty,
        );
        self.response_shown = shown_text(
            &self.detail.response_body,
            self.detail.response_content_type.as_deref(),
            self.response_pretty,
        );
        // 换视图就是换了一段文本：分块从头数，别拿旧长度去切新串。
        self.request_rendered = self.request_shown.len().min(CHUNK);
        self.response_rendered = self.response_shown.len().min(CHUNK);
    }

    fn set_pretty(&mut self, request: bool, pretty: bool) {
        if request {
            self.request_pretty = pretty && self.request_prettyable;
        } else {
            self.response_pretty = pretty && self.response_prettyable;
        }
        self.refresh_texts();
    }

    /// 现在该放进控件的正文（前 `rendered` 字节，落在字符边界）。
    fn rendered(&self, request: bool) -> String {
        let (text, limit) = if request {
            (&self.request_shown, self.request_rendered)
        } else {
            (&self.response_shown, self.response_rendered)
        };
        take_prefix(text, limit).to_owned()
    }

    /// 还没放出去多少字节（0 = 这一侧已经全在控件里了）。
    fn remaining(&self, request: bool) -> usize {
        let (text, limit) = if request {
            (&self.request_shown, self.request_rendered)
        } else {
            (&self.response_shown, self.response_rendered)
        };
        text.len().saturating_sub(limit)
    }

    fn load_more(&mut self, request: bool) {
        let full = if request {
            self.request_shown.len()
        } else {
            self.response_shown.len()
        };
        let limit = if request {
            &mut self.request_rendered
        } else {
            &mut self.response_rendered
        };
        *limit = (*limit + CHUNK).min(full);
    }
}

/// 页面手上的全部信号（`Ctx` 是 `Copy`，`Memo` 也是，所以这整包也是 `Copy`）。
#[derive(Clone, Copy)]
struct Cap {
    ctx: Ctx,
    name: Signal<String>,
    rows: Signal<Vec<CaptureRow>>,
    meta: Signal<Meta>,
    counts: Memo<Counts>,
    indexes: Memo<Vec<usize>>,
    status: Signal<StatusFilter>,
    res: Signal<ResFilter>,
    panel: Signal<Option<Column>>,
    selected: Signal<Option<u64>>,
    detail: Signal<Option<DetailView>>,
    tab: Signal<usize>,
}

// ---- 入口 ----------------------------------------------------------------

pub fn page(ctx: Ctx) -> impl IntoElement {
    let name = ctx.capture_env.get_untracked().unwrap_or_default();
    let synthetic = crate::harness_flag("NG_GOLDEN") || stress_rows() > 0;

    let rows = signal(Vec::<CaptureRow>::new());
    let meta = signal(Meta::default());
    let status = signal(StatusFilter::default());
    let res = signal(ResFilter::default());
    let panel = signal(None::<Column>);
    let selected = signal(None::<u64>);
    let detail = signal(None::<DetailView>);
    let tab = signal(0usize);

    let cap_info = Cap {
        ctx,
        name: signal(name.clone()),
        rows,
        meta,
        counts: computed(move || rows.with(|rows| cap::counts(rows))),
        indexes: computed(move || {
            rows.with(|rows| cap::filtered_indexes(rows, status.get(), res.get()))
        }),
        status,
        res,
        panel,
        selected,
        detail,
        tab,
    };

    if synthetic {
        // golden / 压力口径：不启宿主、不碰真状态目录，行来自 `fixture_rows`。
        let count = if stress_rows() > 0 { stress_rows() } else { 12 };
        let mut fixture = cap::fixture_rows(count);
        if crate::harness_flag("NG_CAPTURE_DETAIL") && !fixture.is_empty() {
            // 详情基线：首行换成样本记录，抽屉里的方法与状态、地址、体积
            // 都和列表那一行对得上（不启宿主也能拍）。
            fixture[0] = fixture_detail_row();
            selected.set(Some(1));
            // `NG_CAPTURE_SKELETON`：停在骨架态，用来目验/拍「正在读取」那一帧。
            if crate::harness_flag("NG_CAPTURE_SKELETON") {
                detail.set(Some(DetailView::skeleton(&fixture_detail_row())));
            } else {
                detail.set(Some(DetailView::build(fixture_detail())));
            }
        }
        rows.set(fixture);
        // 拍摄口径：`NG_CAPTURE_TAB=response` 起手切到响应页签（大正文样本在响应侧）。
        if crate::harness_var("NG_CAPTURE_TAB").as_deref() == Some("response") {
            tab.set(1);
        }
        // `NG_CAPTURE_CHUNK=n`：先把「继续加载」按 n 次（与按钮同一条路径），
        // 好拍出分块之后的帧；按钮的鼠标命中由真机会话那一轮点验。
        let chunks: usize = crate::harness_var("NG_CAPTURE_CHUNK")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        if chunks > 0 {
            detail.update(|slot| {
                if let Some(view) = slot.as_mut() {
                    for _ in 0..chunks {
                        view.load_more(false);
                    }
                }
            });
        }
        // 拍摄口径：`NG_CAPTURE_PANEL=status|type` 起手就把列筛选面板摊开，
        // 免得拍图时还要点一下表头。
        match crate::harness_var("NG_CAPTURE_PANEL").as_deref() {
            Some("status") => panel.set(Some(Column::Status)),
            Some("type") => panel.set(Some(Column::Type)),
            _ => {}
        }
        meta.set(Meta {
            session: 1,
            generation: 1,
            captured: count as u64,
            dropped: 0,
            loaded: true,
            error: None,
        });
    } else {
        // 首拍同步读（窗口第一帧就有内容），之后每 500ms 一拍。
        // `spawn` 归当前 owner：返回环境列表时页面卸载，循环随之取消。
        spawn(async move {
            pull(cap_info);
            loop {
                timer(Duration::from_millis(POLL_MS)).await;
                pull(cap_info);
            }
        });
    }

    view()
        .flex_col()
        .flex_1()
        .w_full()
        .h_full()
        .bg(|| t::page())
        .child(toolbar(cap_info))
        .child(when(
            move || cap_info.meta.get().error.is_some(),
            move || {
                ui::error_bar(
                    "抓包数据读取失败",
                    cap_info.meta.get().error.unwrap_or_default(),
                    move || cap_info.meta.update(|meta| meta.error = None),
                )
            },
        ))
        .child(
            view()
                .flex_1()
                .w_full()
                .relative()
                .overflow_hidden()
                .bg(|| t::layer())
                .child(list(cap_info))
                .child(when(move || cap_info.detail.get().is_some(), move || {
                    detail_panel(cap_info)
                })),
        )
}

// ---- 数据：增量合并 -----------------------------------------------------

/// 拉一拍：游标之后的行批量并入；会话/代次变了就清表重来，缓冲淘汰了就砍头。
fn pull(cap: Cap) {
    let name = cap.name.get_untracked();
    if name.is_empty() {
        return;
    }
    let mut pages = 0usize;
    loop {
        if pages >= PULL_MAX_PAGES {
            break;
        }
        let cursor = cap
            .rows
            .with_untracked(|rows| rows.last().map(|row| row.request_id).unwrap_or(0));
        match service::capture_rows(&name, cursor, PAGE) {
            Ok(Some(batch)) => {
                let session_changed = cap.meta.with_untracked(|meta| {
                    meta.session != batch.session.id || meta.generation != batch.session.generation
                });
                if session_changed {
                    cap.rows.update(|rows| rows.clear());
                }
                cap.meta.update(|meta| {
                    meta.session = batch.session.id;
                    meta.generation = batch.session.generation;
                    meta.captured = batch.captured;
                    meta.dropped = batch.dropped;
                    meta.loaded = true;
                    meta.error = None;
                });
                // 缓冲按字节淘汰：比 `oldest` 老的行已经不在引擎里了，本地也不该留着。
                if let Some(oldest) = batch.oldest {
                    cap.rows.update(|rows| {
                        let drop_to = rows
                            .iter()
                            .position(|row| row.request_id >= oldest)
                            .unwrap_or(rows.len());
                        rows.drain(..drop_to);
                    });
                }
                if batch.rows.is_empty() {
                    break;
                }
                cap.rows.update(|rows| rows.extend(batch.rows));
                pages += 1;
                if session_changed {
                    // 新会话的行从 1 开始；游标已清，下一轮从 0 拉。
                    continue;
                }
            }
            Ok(None) => {
                cap.meta.update(|meta| {
                    meta.loaded = true;
                    meta.error = None;
                });
                break;
            }
            Err(error) => {
                let line = format!("{} · {}", error.code.as_str(), error.message);
                cap.meta.update(|meta| meta.error = Some(line));
                break;
            }
        }
    }
}

// ---- 工具条 --------------------------------------------------------------

fn env_of(cap: Cap) -> Option<crate::model::Env> {
    let name = cap.name.get();
    cap.ctx.envs.get().into_iter().find(|env| env.name == name)
}

fn toolbar(cap: Cap) -> impl IntoElement {
    let listen = move || env_of(cap).map(|env| env.listen).unwrap_or_default();
    let running = move || env_of(cap).map(|env| env.capture).unwrap_or(false);
    view()
        .h(px(m::BAR_H))
        .w_full()
        .px(px(16.0))
        .flex_row()
        .items_center()
        .border_b_1()
        .border_color(|| t::divider())
        .bg(|| t::page())
        .child(ui::btn_text("← 环境", t::ink_secondary, move || {
            cap.ctx.capture_env.set(None)
        }))
        .child(ui::spacer(10.0))
        .child(
            text(move || cap.name.get())
                .text_size(px(f::BODY))
                .font_family(f::FONT_TEXT)
                .text_color(|| t::ink()),
        )
        .child(ui::spacer(8.0))
        .child(
            text(listen)
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_MONO)
                .text_color(|| t::ink_tertiary()),
        )
        .child(ui::spacer(12.0))
        // 会话 chip：会话号 · 已捕获 · 丢弃。数字全部来自数据面，不在这里自己数。
        .child(
            view()
                .h(px(24.0))
                .px(px(8.0))
                .flex_row()
                .items_center()
                .rounded(px(12.0))
                .border_1()
                .border_color(|| t::control_stroke())
                .child(
                    text(move || {
                        let meta = cap.meta.get();
                        format!(
                            "#{} · 已捕获 {} · 丢弃 {}",
                            meta.session,
                            thousands(meta.captured),
                            thousands(meta.dropped)
                        )
                    })
                    .text_size(px(f::CAPTION))
                    .font_family(f::FONT_MONO)
                    .text_color(|| t::ink_secondary()),
                ),
        )
        .child(view().flex_1())
        .child(when(move || !running(), move || {
            ui::btn_primary("开启抓包", move || toggle_capture(cap, false))
        }))
        .child(when(move || running(), move || {
            ui::btn_subtle("停止抓包", move || toggle_capture(cap, true))
        }))
        .child(ui::spacer(6.0))
        .child(ui::btn_text("清空", t::ink_secondary, move || {
            ask_clear(cap)
        }))
        .child(ui::spacer(6.0))
        // 导出对齐桌面工具条：只出 HAR（JSONL 是后端能力，页面不摆两个按钮）。
        .child(ui::btn_text("导出 HAR", t::accent_ink, move || {
            actions::export_har(cap.ctx, cap.name.get_untracked())
        }))
}

fn toggle_capture(cap: Cap, running: bool) {
    let name = cap.name.get_untracked();
    if running {
        match service::stop_debug() {
            Ok(_) => {
                actions::refresh(cap.ctx);
                actions::succeed(cap.ctx, "已停止抓包，已捕获的记录保留");
            }
            Err(error) => {
                actions::fail(cap.ctx, error);
            }
        }
        return;
    }
    match service::start_debug(&name) {
        Ok(_) => {
            actions::refresh(cap.ctx);
            actions::succeed(cap.ctx, format!("已在 {name} 开启抓包"));
            pull(cap);
        }
        Err(error) => {
            actions::fail(cap.ctx, error);
        }
    }
}

/// 清空不可逆（记录只在这块缓冲里），走应用内确认框；
/// 确认后由 `actions` 调数据面，页面靠轮询发现的代次变化清掉本地行。
fn ask_clear(cap: Cap) {
    let name = cap.name.get_untracked();
    let count = cap.rows.with(|rows| rows.len());
    cap.ctx
        .confirm
        .set(Some(actions::Confirm::clear_capture(&name, count)));
}

// ---- 表头（列即筛选） ----------------------------------------------------

fn table_head(cap: Cap) -> impl IntoElement {
    view()
        .h(px(m::HEAD_H))
        .w_full()
        .flex_row()
        .items_center()
        .bg(|| t::page())
        .border_b_1()
        .border_color(|| t::divider())
        .child(view().w(px(m::BAR_W)))
        .child(head_cell("#", COL_ID))
        .child(head_cell("方法", COL_METHOD))
        .child(head_button(cap, Column::Status, COL_STATUS))
        .child(head_cell("域名", COL_HOST))
        .child(head_cell_grow("路径"))
        .child(head_cell("大小", COL_SIZE))
        .child(head_button(cap, Column::Type, COL_TYPE))
}

fn head_cell(label: &'static str, width: f32) -> impl IntoElement {
    view()
        .w(px(width))
        .h_full()
        .flex_row()
        .items_center()
        .pl(px(12.0))
        .child(
            text(label)
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_TEXT)
                .text_color(|| t::ink_tertiary()),
        )
}

fn head_cell_grow(label: &'static str) -> impl IntoElement {
    view()
        .flex_1()
        .h_full()
        .flex_row()
        .items_center()
        .pl(px(12.0))
        .child(
            text(label)
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_TEXT)
                .text_color(|| t::ink_tertiary()),
        )
}

/// 可点的表头：一行两件事——「这一列叫什么」与「现在按它筛什么」。
fn head_button(cap: Cap, column: Column, width: f32) -> impl IntoElement {
    let label = move || match column {
        Column::Status => {
            let filter = cap.status.get();
            if filter == StatusFilter::All {
                "状态 ▾".to_owned()
            } else {
                format!("状态 · {} ▾", filter.label())
            }
        }
        Column::Type => {
            let filter = cap.res.get();
            if filter == ResFilter::All {
                "类型 ▾".to_owned()
            } else {
                format!("{} ▾", filter.label())
            }
        }
    };
    let active = move || match column {
        Column::Status => cap.status.get() != StatusFilter::All,
        Column::Type => cap.res.get() != ResFilter::All,
    };
    let open = move || cap.panel.get() == Some(column);
    view()
        .w(px(width))
        .h_full()
        .flex_row()
        .items_center()
        .pl(px(12.0))
        .on_press(move || {
            if cap.panel.get() == Some(column) {
                cap.panel.set(None);
            } else {
                cap.panel.set(Some(column));
            }
        })
        .child(
            text(label)
                .flex_1()
                .truncate()
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_TEXT)
                .text_color(move || {
                    if open() || active() {
                        t::accent_ink()
                    } else {
                        t::ink_tertiary()
                    }
                }),
        )
}

/// 点开表头后出现在表头与列表之间的分段面板（列即筛选的「面板」那一半）。
fn panel_bar(cap: Cap) -> impl IntoElement {
    view()
        .h(px(40.0))
        .w_full()
        .px(px(16.0))
        .flex_row()
        .items_center()
        .border_b_1()
        .border_color(|| t::divider())
        .bg(|| t::page())
        .overflow_hidden()
        .child(when(
            move || cap.panel.get() == Some(Column::Status),
            move || status_segments(cap),
        ))
        .child(when(
            move || cap.panel.get() == Some(Column::Type),
            move || type_segments(cap),
        ))
}

fn status_segments(cap: Cap) -> impl IntoElement {
    let mut row = view().flex_row().items_center();
    for filter in StatusFilter::ALL {
        row = row.child(ui::pill_live(
            filter.label(),
            move || cap.counts.with(|counts| counts.status(filter)),
            move || cap.status.get() == filter,
            move || {
                cap.status.set(filter);
                cap.panel.set(None);
            },
        ));
        row = row.child(ui::spacer(4.0));
    }
    row
}

fn type_segments(cap: Cap) -> impl IntoElement {
    let mut row = view().flex_row().items_center();
    for filter in ResFilter::ALL {
        row = row.child(ui::pill_live(
            filter.label(),
            move || cap.counts.with(|counts| counts.res(filter)),
            move || cap.res.get() == filter,
            move || {
                cap.res.set(filter);
                cap.panel.set(None);
            },
        ));
        row = row.child(ui::spacer(4.0));
    }
    // 九片在窄窗里放不下：横向滚动，不折行撑高（表头高度在任何窗宽下都一样）。
    scroll()
        .horizontal()
        .scrollbar(false)
        .flex_1()
        .h_full()
        .child(row)
}

// ---- 列表 ----------------------------------------------------------------

fn list(cap: Cap) -> impl IntoElement {
    view()
        .flex_col()
        .flex_1()
        .w_full()
        .h_full()
        .child(table_head(cap))
        .child(when(
            move || cap.panel.get().is_some(),
            move || panel_bar(cap),
        ))
        .child(
            nsh_core::element::virtual_list(
                move || cap.indexes.with(|indexes| indexes.len()),
                move |at| row_at(at, cap),
            )
            .scrollbar(false)
            .estimated_item_size(px(m::ROW_H))
            .empty(empty_state(cap))
            .flex_1()
            .w_full(),
        )
}

fn empty_state(cap: Cap) -> impl IntoElement {
    view()
        .w_full()
        .h(px(140.0))
        .flex_col()
        .items_center()
        .justify_center()
        .child(
            text(move || {
                let meta = cap.meta.get();
                if cap.rows.with(|rows| !rows.is_empty()) {
                    // 有行但筛完是空：说清楚是筛选的事。
                    let status = cap.status.get();
                    let res = cap.res.get();
                    if status != StatusFilter::All && res != ResFilter::All {
                        format!("「{}」×「{}」下没有记录", status.label(), res.label())
                    } else if status != StatusFilter::All {
                        format!("「{}」下没有记录", status.label())
                    } else {
                        format!("「{}」下没有记录", res.label())
                    }
                } else if meta.error.is_some() {
                    "读不到抓包记录 —— 原因在上方错误条里。".to_owned()
                } else if !meta.loaded {
                    "正在读取抓包记录…".to_owned()
                } else {
                    "还没有记录：让请求走这个环境的代理地址，就会出现在这里。".to_owned()
                }
            })
            .text_size(px(f::CAPTION))
            .font_family(f::FONT_TEXT)
            .text_color(|| t::ink_tertiary()),
        )
        .child(ui::vspacer(8.0))
        .child(when(
            move || {
                cap.rows.with(|rows| !rows.is_empty())
                    && (cap.status.get() != StatusFilter::All || cap.res.get() != ResFilter::All)
            },
            move || {
                ui::btn_subtle("清除筛选", move || {
                    cap.status.set(StatusFilter::All);
                    cap.res.set(ResFilter::All);
                })
            },
        ))
}

fn row_at(at: Position, cap: Cap) -> impl IntoElement {
    let row = move || {
        cap.indexes.with(|indexes| {
            indexes
                .get(at.index())
                .and_then(|row_index| cap.rows.with(|rows| rows.get(*row_index).cloned()))
        })
    };
    let id = move || row().map(|row| row.request_id).unwrap_or(0);
    let cells = vec![
        quiet_mono(move || id().to_string(), COL_ID).into_element(),
        mono_cell(
            move || {
                row()
                    .map(|row| cap::method_text(&row).to_owned())
                    .unwrap_or_default()
            },
            COL_METHOD,
        )
        .into_element(),
        status_cell(at, cap, COL_STATUS).into_element(),
        quiet_mono(
            move || row().map(|row| row.authority).unwrap_or_default(),
            COL_HOST,
        )
        .into_element(),
        path_cell(at, cap).into_element(),
        quiet_mono(
            move || row().map(|row| cap::size_text(&row)).unwrap_or_default(),
            COL_SIZE,
        )
        .into_element(),
        quiet_mono(
            move || {
                row()
                    .map(|row| cap::resource_of(&row).label().to_owned())
                    .unwrap_or_default()
            },
            COL_TYPE,
        )
        .into_element(),
    ];
    let is_selected = move || cap.selected.get() == Some(id());
    let at_index = at.index();
    ui::row_shell(is_selected, move || {
        let id = cap.indexes.with(|indexes| {
            indexes
                .get(at_index)
                .and_then(|row_index| cap.rows.with(|rows| rows.get(*row_index).map(|row| row.request_id)))
                .unwrap_or(0)
        });
        open_detail(cap, id);
    })(cells)
}

fn mono_cell(label: impl IntoReactive<SharedString> + 'static, width: f32) -> impl IntoElement {
    cell_with(label, width, f::FONT_MONO, t::ink_secondary)
}

fn quiet_mono(label: impl IntoReactive<SharedString> + 'static, width: f32) -> impl IntoElement {
    cell_with(label, width, f::FONT_MONO, t::ink_tertiary)
}

fn cell_with(
    label: impl IntoReactive<SharedString> + 'static,
    width: f32,
    font: &'static str,
    ink: fn() -> AdaptiveColor,
) -> impl IntoElement {
    view()
        .w(px(width))
        .h(px(m::ROW_H))
        .flex_row()
        .items_center()
        .pl(px(12.0))
        .overflow_hidden()
        .child(
            text(label)
                .flex_1()
                .truncate()
                .text_size(px(f::CAPTION))
                .font_family(font)
                .text_color(move || ink()),
        )
}

/// 路径列吃掉剩下的宽度，永远单行省略（一行里的长 query 不许把行撑高）。
fn path_cell(at: Position, cap: Cap) -> impl IntoElement {
    let path = move || {
        cap.indexes.with(|indexes| {
            indexes
                .get(at.index())
                .and_then(|row_index| cap.rows.with(|rows| rows.get(*row_index).map(|row| row.path.clone())))
                .unwrap_or_default()
        })
    };
    view()
        .flex_1()
        .h(px(m::ROW_H))
        .flex_row()
        .items_center()
        .pl(px(12.0))
        .overflow_hidden()
        .child(
            text(path)
                .flex_1()
                .truncate()
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_MONO)
                .text_color(|| t::ink()),
        )
}

/// 状态列：文字本身就是颜色（红/赭/墨三档），点只做辅助形状。
fn status_cell(at: Position, cap: Cap, width: f32) -> impl IntoElement {
    let text_of = move || {
        cap.indexes.with(|indexes| {
            indexes
                .get(at.index())
                .and_then(|row_index| cap.rows.with(|rows| rows.get(*row_index).map(cap::status_text)))
                .unwrap_or_else(|| "—".to_owned())
        })
    };
    let tone_of = move || {
        cap.indexes.with(|indexes| {
            indexes
                .get(at.index())
                .and_then(|row_index| cap.rows.with(|rows| rows.get(*row_index).map(cap::status_tone)))
                .unwrap_or(ui::Tone::Neutral)
        })
    };
    ui::status_cell_live(tone_of, || ui::Shape::Dot, text_of, width)
}

fn open_detail(cap: Cap, request_id: u64) {
    if request_id == 0 {
        return;
    }
    cap.selected.set(Some(request_id));
    cap.tab.set(0);
    let name = cap.name.get_untracked();
    if crate::harness_flag("NG_GOLDEN") || stress_rows() > 0 {
        // 合成样本走同一套骨架/回填两步，好把中间态单独拍下来。
        if crate::harness_flag("NG_CAPTURE_SKELETON") {
            cap.detail
                .set(Some(DetailView::skeleton(&fixture_detail_row())));
            return;
        }
        cap.detail.set(Some(DetailView::build(fixture_detail())));
        return;
    }
    // 先上骨架：头部四格（方法 / 状态 / 时刻 / 地址）列表那一行就有，
    // 不必等完整记录（headers + 两侧正文）拷过来才让面板出现。
    let row = cap
        .rows
        .with(|rows| rows.iter().find(|row| row.request_id == request_id).cloned());
    if let Some(row) = row {
        cap.detail.set(Some(DetailView::skeleton(&row)));
    }
    if crate::harness_flag("NG_CAPTURE_SKELETON") {
        return;
    }
    spawn(async move {
        match service::capture_detail_async(name, request_id).await {
            Ok(Ok(Some(record))) => {
                // 用户可能在取详情期间点了别的行或关掉抽屉：只回填还停在这一条上的那次。
                if cap.selected.get() == Some(request_id) {
                    cap.detail
                        .set(Some(DetailView::build(Detail::from_record(&record))));
                }
            }
            Ok(Ok(None)) => {
                cap.selected.set(None);
                cap.detail.set(None);
                actions::fail(
                    cap.ctx,
                    envboard_engine::Error::internal_error("这条记录已不在缓冲里（可能已被淘汰）"),
                );
            }
            Ok(Err(error)) => {
                cap.selected.set(None);
                cap.detail.set(None);
                actions::fail(cap.ctx, error);
            }
            Err(_) => {
                cap.selected.set(None);
                cap.detail.set(None);
                actions::fail(
                    cap.ctx,
                    envboard_engine::Error::internal_error("读取详情失败：回传通道已断开"),
                );
            }
        }
    });
}

// ---- 详情面板 ------------------------------------------------------------
//
// 面板在**打开那一帧**就建出来，此时手里只有列表行（骨架态）；页签与正文
// 由 `when(loading)` 在详情回来时补挂。头部四格不走快照，全部经信号现读——
// 骨架与完整记录共用同一个头，地址里的查询串回来时才补得上，切换美化态时
// 标题上的「复制美化 / 复制原文」也要跟着变。

fn detail_panel(cap: Cap) -> AnyElement {
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
        .child(detail_head(cap))
        .child(when(move || detail_view(cap, |v| v.loading).unwrap_or(false), {
            move || skeleton_body()
        }))
        .child(when(
            move || detail_view(cap, |v| !v.loading).unwrap_or(false),
            move || detail_tabs(cap),
        ))
        .child(when(
            move || detail_view(cap, |v| !v.loading).unwrap_or(false),
            move || detail_body(cap),
        ))
        .into_element()
}

/// 从详情信号里取一个字段（没有详情时给默认值）。同步求值，不需要 `'static`。
fn detail_view<T>(cap: Cap, pick: impl Fn(&DetailView) -> T) -> Option<T> {
    cap.detail.with(|slot| slot.as_ref().map(pick))
}

fn detail_head(cap: Cap) -> impl IntoElement {
    let status_ink = move || {
        let tone = detail_view(cap, |v| v.detail.status_tone()).unwrap_or(ui::Tone::Neutral);
        match tone {
            ui::Tone::Critical => t::critical(),
            ui::Tone::Caution => t::caution_ink(),
            _ => t::ink_secondary(),
        }
    };
    let method = move || detail_view(cap, |v| v.detail.method.clone()).unwrap_or_default();
    let status = move || detail_view(cap, |v| v.detail.status_text()).unwrap_or_default();
    let time = move || detail_view(cap, |v| v.detail.time_text()).unwrap_or_default();
    let url = move || detail_view(cap, |v| v.detail.url()).unwrap_or_default();
    view()
        .w_full()
        .flex_col()
        .border_b_1()
        .border_color(|| t::divider())
        .child(
            view()
                .h(px(m::BAR_H))
                .w_full()
                .pl(px(16.0))
                .pr(px(8.0))
                .flex_row()
                .items_center()
                .child(
                    text(method)
                        .text_size(px(f::CAPTION))
                        .font_family(f::FONT_MONO)
                        .text_color(|| t::ink()),
                )
                .child(ui::spacer(10.0))
                .child(
                    text(status)
                        .text_size(px(f::CAPTION))
                        .font_family(f::FONT_MONO)
                        .text_color(status_ink),
                )
                .child(ui::spacer(10.0))
                .child(
                    text(time)
                        .text_size(px(f::CAPTION))
                        .font_family(f::FONT_MONO)
                        .text_color(|| t::ink_tertiary()),
                )
                .child(view().flex_1())
                .child(ui::btn_text("✕", t::ink_secondary, move || {
                    cap.selected.set(None);
                    cap.detail.set(None);
                })),
        )
        .child(
            view().w_full().px(px(16.0)).pb(px(10.0)).child(
                text(url)
                    .w_full()
                    .truncate()
                    .text_size(px(f::CAPTION))
                    .font_family(f::FONT_MONO)
                    .text_color(|| t::ink_secondary()),
            ),
        )
}

/// 骨架：三根占位条 + 一句“正在读取”。高度按真实内容估，回来时不跳。
fn skeleton_body() -> impl IntoElement {
    let bar = |width: f32, height: f32| {
        view()
            .w(px(width))
            .h(px(height))
            .rounded(px(m::RADIUS))
            .bg(|| t::divider())
    };
    view()
        .flex_1()
        .w_full()
        .p(px(16.0))
        .flex_col()
        .child(
            text("正在读取这条记录…")
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_TEXT)
                .text_color(|| t::ink_tertiary()),
        )
        .child(ui::vspacer(14.0))
        .child(bar(120.0, 14.0))
        .child(ui::vspacer(8.0))
        .child(bar(300.0, 14.0))
        .child(ui::vspacer(8.0))
        .child(bar(240.0, 14.0))
        .child(ui::vspacer(18.0))
        .child(
            view()
                .w_full()
                .h(px(200.0))
                .rounded(px(m::RADIUS))
                .bg(|| t::control_fill()),
        )
}

fn detail_tabs(cap: Cap) -> impl IntoElement {
    let request_count = detail_view(cap, |v| v.detail.request_headers.len()).unwrap_or(0);
    let response_count = detail_view(cap, |v| v.detail.response_headers.len()).unwrap_or(0);
    let tab_button = move |index: usize, label: &'static str, count: usize| {
        view()
            .h(px(m::TAB_H))
            .px(px(12.0))
            .flex_row()
            .items_center()
            .border_b_1()
            .border_color(move || {
                if cap.tab.get() == index {
                    t::accent()
                } else {
                    t::divider()
                }
            })
            .on_press(move || cap.tab.set(index))
            .child(
                text(label)
                    .text_size(px(f::CAPTION))
                    .font_family(f::FONT_TEXT)
                    .text_color(move || {
                        if cap.tab.get() == index {
                            t::accent_ink()
                        } else {
                            t::ink_secondary()
                        }
                    }),
            )
            .child(ui::spacer(4.0))
            .child(
                text(count.to_string())
                    .text_size(px(f::CAPTION))
                    .font_family(f::FONT_MONO)
                    .text_color(|| t::ink_tertiary()),
            )
    };
    view()
        .h(px(m::TAB_H))
        .w_full()
        .px(px(8.0))
        .flex_row()
        .items_center()
        .border_b_1()
        .border_color(|| t::divider())
        .bg(|| t::layer())
        .child(tab_button(0, "请求", request_count))
        .child(tab_button(1, "响应", response_count))
        .child(view().flex_1())
}

fn detail_body(cap: Cap) -> impl IntoElement {
    // 两侧内容（一串头部 + 240 高的正文框 + 「继续加载」）会比抽屉高：
    // 竖滚放在这一层，抽屉头与页签固定。
    view()
        .flex_1()
        .w_full()
        .overflow_hidden()
        .child(when(move || cap.tab.get() == 0, move || {
            scroll().scrollbar(true).flex_1().w_full().child(side_panel(cap, true))
        }))
        .child(when(move || cap.tab.get() == 1, move || {
            scroll().scrollbar(true).flex_1().w_full().child(side_panel(cap, false))
        }))
}

fn side_panel(cap: Cap, request: bool) -> AnyElement {
    let Some(detail) = detail_view(cap, |v| v.detail.clone()) else {
        return view().into_element();
    };
    let headers = if request {
        detail.request_headers.clone()
    } else {
        detail.response_headers.clone()
    };
    let body = if request {
        detail.request_body.clone()
    } else {
        detail.response_body.clone()
    };
    let prettyable = detail_view(cap, |v| {
        if request {
            v.request_prettyable
        } else {
            v.response_prettyable
        }
    })
    .unwrap_or(false);

    let mut column = view().flex_col().p(px(16.0));
    if request {
        column = column
            .child(ui::section_label("请求"))
            .child(ui::kv("方法", detail.method.clone(), true))
            .child(ui::kv("地址", detail.url(), true))
            .child(ui::kv("时间", detail.time_text(), true))
            .child(ui::kv("体积", detail.request_size_text(), true));
    } else {
        column = column
            .child(ui::section_label("响应"))
            .child(ui::kv("状态", detail.status_text(), true))
            .child(ui::kv("体积", detail.response_size_text(), true));
    }
    if let Some(error) = detail.error.clone() {
        column = column.child(ui::vspacer(8.0)).child(ui::alert(
            ui::Tone::Critical,
            "这条记录以错误结束",
            error,
        ));
    }
    column
        .child(ui::vspacer(14.0))
        .child(ui::section_label(if request { "请求头" } else { "响应头" }))
        .child(headers_section(headers))
        .child(ui::vspacer(14.0))
        .child(body_section(cap, request, body, prettyable))
        .into_element()
}

fn headers_section(headers: Vec<(String, String)>) -> impl IntoElement {
    if headers.is_empty() {
        return view()
            .child(
                text("（没有记录到头部）")
                    .text_size(px(f::CAPTION))
                    .font_family(f::FONT_TEXT)
                    .text_color(|| t::ink_tertiary()),
            )
            .into_element();
    }
    let mut column = view().flex_col();
    for (name, value) in headers {
        column = column.child(
            view()
                .w_full()
                .py(px(3.0))
                .flex_row()
                .items_center()
                .child(
                    text(name)
                        .w(px(150.0))
                        .truncate()
                        .text_size(px(f::CAPTION))
                        .font_family(f::FONT_MONO)
                        .text_color(|| t::ink_tertiary()),
                )
                .child(
                    text(value)
                        .flex_1()
                        .truncate()
                        .text_size(px(f::CAPTION))
                        .font_family(f::FONT_MONO)
                        .text_color(|| t::ink_secondary()),
                ),
        );
    }
    column.into_element()
}

/// 正文块：标题 + 体积 + 美化/原文两态 + 复制；正文本身是**只读多行框**
/// （原生编辑控件扛得住几千行，逐行造 `text` 会又慢又没法选文字）。
///
/// 超过 [`CHUNK`] 的正文不整块进控件：先放前 32 KiB，底下给「继续加载剩余 …」，
/// 每次再放 32 KiB。框里的文字、复制按钮的文案、剩余多少，全部经信号现读——
/// 这个子树只在详情回来时建一次，快照会把切换美化态后的内容留在旧值上。
fn body_section(cap: Cap, request: bool, body: Body, prettyable: bool) -> impl IntoElement {
    let title = if request { "请求体" } else { "响应体" };
    let size = crate::model::capture::format_bytes(body.size);
    let copyable = body.kind != Kind::Omitted;
    let copy_label = move || {
        let pretty = detail_view(cap, |v| {
            if request {
                v.request_pretty
            } else {
                v.response_pretty
            }
        })
        .unwrap_or(false);
        SharedString::from(if pretty { "复制美化" } else { "复制原文" })
    };

    let mut head = view()
        .w_full()
        .flex_row()
        .items_center()
        .child(
            text(title)
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_TEXT)
                .text_color(|| t::ink_secondary()),
        )
        .child(ui::spacer(8.0))
        .child(
            text(size)
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_MONO)
                .text_color(|| t::ink_tertiary()),
        )
        .child(view().flex_1());

    if prettyable {
        head = head
            .child(ui::choice_live(
                "美化",
                move || {
                    cap.detail.with(|view| {
                        view.as_ref()
                            .map(|view| if request { view.request_pretty } else { view.response_pretty })
                            .unwrap_or(false)
                    })
                },
                move || {
                    cap.detail.update(|slot| {
                        if let Some(view) = slot.as_mut() {
                            view.set_pretty(request, true);
                        }
                    });
                },
            ))
            .child(ui::spacer(4.0))
            .child(ui::choice_live(
                "原文",
                move || {
                    cap.detail.with(|view| {
                        view.as_ref()
                            .map(|view| {
                                if request {
                                    !view.request_pretty
                                } else {
                                    !view.response_pretty
                                }
                            })
                            .unwrap_or(true)
                    })
                },
                move || {
                    cap.detail.update(|slot| {
                        if let Some(view) = slot.as_mut() {
                            view.set_pretty(request, false);
                        }
                    });
                },
            ));
    }
    if copyable {
        head = head.child(ui::spacer(6.0)).child(ui::btn_text(
            copy_label,
            t::accent_ink,
            // 点下去那一刻**从详情里现取**当前视图的文本：闭包得是 `Copy`
            // （按钮按帧求值），而 64 KiB 的字符串不是。
            move || {
                let (text, pretty_now) = cap.detail.with(|slot| {
                    slot.as_ref()
                        .map(|view| {
                            if request {
                                (view.request_shown.clone(), view.request_pretty)
                            } else {
                                (view.response_shown.clone(), view.response_pretty)
                            }
                        })
                        .unwrap_or_default()
                });
                copy_body(cap, text, pretty_now);
            },
        ));
    }

    view()
        .w_full()
        .flex_col()
        .child(head)
        .child(ui::vspacer(8.0))
        .child(body_note(&body, prettyable))
        .child(
            nsh_base::input()
                .multiline(true)
                .readonly(true)
                .scrollbar(true)
                .value(move || detail_view(cap, |v| v.rendered(request)).unwrap_or_default())
                .h(px(240.0))
                .w_full()
                .px(px(10.0))
                .py(px(8.0))
                .border_1()
                .border_color(|| t::control_stroke())
                .rounded(px(m::RADIUS))
                .bg(|| t::control_fill())
                .text_size(px(f::CAPTION))
                .font_family(f::FONT_MONO)
                .text_color(|| t::ink()),
        )
        .child(when(
            move || detail_view(cap, |v| v.remaining(request) > 0).unwrap_or(false),
            move || {
                view().w_full().pt(px(8.0)).flex_row().items_center().child(
                    ui::btn_text(
                        move || {
                            let rest = detail_view(cap, |v| v.remaining(request)).unwrap_or(0);
                            SharedString::from(format!(
                                "继续加载剩余 {}",
                                crate::model::capture::format_bytes(rest as u64)
                            ))
                        },
                        t::accent_ink,
                        move || {
                            cap.detail.update(|slot| {
                                if let Some(view) = slot.as_mut() {
                                    view.load_more(request);
                                }
                            });
                        },
                    ),
                )
            },
        ))
}

/// 一行说明：什么形状、为什么没有美化/没有原文。
fn body_note(body: &Body, prettyable: bool) -> impl IntoElement {
    let line = match body.kind {
        Kind::Omitted => body.note().unwrap_or_default(),
        Kind::Binary => body
            .note()
            .unwrap_or_else(|| "二进制正文：只能给原文".to_owned()),
        Kind::Absent => "（无正文）".to_owned(),
        Kind::Text => {
            if prettyable {
                String::new()
            } else if body.raw.len() as u64 > crate::model::body::PRETTY_LIMIT
                || body.size > crate::model::body::PRETTY_LIMIT
            {
                format!(
                    "正文 {} 超过 {} 美化阈值，只给原文",
                    crate::model::capture::format_bytes(body.size),
                    crate::model::capture::format_bytes(crate::model::body::PRETTY_LIMIT)
                )
            } else {
                "这一类正文不做重排（只对 JSON / HTML / XML 美化）".to_owned()
            }
        }
    };
    let note = line.clone();
    view().child(when(move || !line.is_empty(), move || {
        text(note.clone())
            .text_size(px(f::CAPTION))
            .font_family(f::FONT_TEXT)
            .text_color(|| t::ink_tertiary())
    }))
}

fn copy_body(cap: Cap, text: String, pretty: bool) {
    let mut item = ClipboardItem::default();
    item.text = text;
    set_clipboard_item(item);
    actions::succeed(
        cap.ctx,
        if pretty {
            "美化文本已复制"
        } else {
            "原文已复制"
        },
    );
}

/// 样本记录在列表里的那一行：与 `fixture_detail` 是同一笔请求，
/// 详情基线里抽屉和行内格子必须指向同一条。
fn fixture_detail_row() -> CaptureRow {
    CaptureRow {
        request_id: 1,
        time: 1_700_000_000_000,
        method: "POST".to_owned(),
        authority: "api.example.com".to_owned(),
        path: "/v1/orders".to_owned(),
        status: Some(200),
        error: false,
        request_size: 13,
        response_size: if crate::harness_flag("NG_CAPTURE_BIG") {
            big_body().len() as u64
        } else {
            21
        },
        content_type: Some("application/json".to_owned()),
    }
}

fn fixture_detail() -> Detail {
    let response_body = if crate::harness_flag("NG_CAPTURE_BIG") {
        let content = big_body();
        let size = content.len() as u64;
        RecordBody::Present {
            encoding: "utf8".to_owned(),
            size,
            content,
        }
    } else {
        RecordBody::Present {
            encoding: "utf8".to_owned(),
            size: 21,
            content: "{\"ok\":true,\"items\":[1]}".to_owned(),
        }
    };
    let record = Record {
        version: 2,
        session: 1,
        request_id: 1,
        time: 1_700_000_000_000,
        duration_ms: 7,
        request: RecordRequest {
            method: "POST".to_owned(),
            path: "/v1/orders".to_owned(),
            query: "page=2".to_owned(),
            authority: "api.example.com".to_owned(),
            headers: vec![
                ("content-type".to_owned(), "application/json".to_owned()),
                ("x-trace".to_owned(), "m5".to_owned()),
            ],
            body: Some(RecordBody::Present {
                encoding: "utf8".to_owned(),
                size: 13,
                content: "{\"id\":1,\"n\":2}".to_owned(),
            }),
        },
        response: Some(RecordResponse {
            status: Some(200),
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: Some(response_body),
        }),
        error: None,
    };
    Detail::from_record(&record)
}

/// 分块渲染的看板样本：40 KiB 的 JSON 数组（原文就超 32 KiB，美化后更长）。
/// 只由 `NG_CAPTURE_BIG` 触发，用来目验「继续加载」与滚动。
fn big_body() -> String {
    let items: Vec<String> = (1..=1400)
        .map(|index| format!("{{\"id\":{index},\"name\":\"row-{index}\"}}"))
        .collect();
    format!("{{\"items\":[{}]}}", items.join(","))
}


/// 压力口径的抓包行数（`NG_STRESS=35000`）：与列表压测同一个开关，
/// 量的是抓包表的首帧与每格成本。
fn stress_rows() -> usize {
    crate::harness_var("NG_STRESS")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

/// 千分位：35,000 比 35000 好读，也不占多少宽度。
fn thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// 正文当前该显示什么：能美化且处于美化态给美化文本，否则原文；
/// `omitted` 给一句话（不是原文，也不给复制），二进制给 base64 原文。
fn shown_text(body: &Body, content_type: Option<&str>, pretty: bool) -> String {
    match body.kind {
        Kind::Absent => "（无正文）".to_owned(),
        Kind::Omitted => body.note().unwrap_or_default(),
        Kind::Binary => body.raw.clone(),
        Kind::Text => {
            if pretty {
                body.pretty(content_type).unwrap_or_else(|| body.raw.clone())
            } else {
                body.raw.clone()
            }
        }
    }
}

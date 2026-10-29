//! 调试屏：工作区级抓包会话的**看**与**控**。
//!
//! 主任务只有一个：*让某个环境上的请求现形，并能读一条的请求与响应*。
//! 界面因此是「工具条 + 记录表 + 停靠详情面板」：
//!
//! * 记录表：时间（UTC）/ 方法 / 主机 / 路径 / 状态 / 大小；
//! * 详情：选中一行打开 `Sheet`，请求 / 响应两页签各给起始行、头、正文。
//!
//! 语义完全来自门面（见 spec/admin-api.md 与 spec/events.md「会话双轨」）：
//!
//! * **会话是工作区级单例**：切换目标 = 停旧 + 清旧 + 开新，界面不许假装能同时看两个；
//! * **只能调试在跑的实例**：候选列表来自环境快照，没在跑的环境列出但不可选；
//!   真开起来时服务端还判一次（`conflict`），界面照实显示它的原因；
//! * **增量取数按 `request_id` 续**：每 500ms 问一次"游标之后有什么"，游标是记录
//!   身份不是行号 —— 行被裁剪、列表重排都不影响续取；清空记录时游标一起归零。
//!
//! 时间列统一 **UTC** 并在表头写明：本机时区换算要额外依赖，宁可如实标 UTC，
//! 也不显示一个悄悄偏几小时的"本地时间"。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui_kit::component::ActiveTheme;
use gpui_kit::component::Disableable;
use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::table::{Column, DataTable, TableDelegate, TableEvent, TableState};
use gpui_kit::component::{Icon, IconName, Sizable};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Font, FontWeight, Hsla, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Task, Window, div, px,
};
use serde_json::Value;

use envboard_protocol::{CaptureDelta, DebugView};

use crate::display::format_bytes;
use crate::host::{Host, hop};
use crate::views::environments::EnvTable;

/// 增量轮询的节拍（与 web 形态调试实时流同一档）。
const POLL: Duration = Duration::from_millis(500);
/// 每个节拍最多取多少条增量。
const DELTA_LIMIT: usize = 500;
/// 表里最多留多少条（会话可以跑很久，界面要有界）。
const MAX_ROWS: usize = 2000;
/// 正文最多渲染多少行（单条记录正文上限 256 KiB，全渲染会卡住布局）。
const MAX_BODY_LINES: usize = 2000;

const COL_TIME: usize = 0;
const COL_METHOD: usize = 1;
const COL_AUTHORITY: usize = 2;
const COL_PATH: usize = 3;
const COL_STATUS: usize = 4;
const COL_SIZE: usize = 5;

// --------------------------------------------------------------------------- #
// 记录：把抓包 JSON 解析成展示字段（解析一次，渲染多次）
// --------------------------------------------------------------------------- #

/// 一条抓包记录。`raw` 留着给详情面板（头与正文都在里面）。
#[derive(Debug, Clone, PartialEq)]
struct Record {
    request_id: u64,
    time_ms: u64,
    method: String,
    authority: String,
    path: String,
    status: u16,
    bytes: u64,
    raw: Value,
}

impl Record {
    fn parse(raw: &Value) -> Option<Self> {
        let request_id = raw.get("request_id")?.as_u64()?;
        let request = raw.get("request")?;
        let response = raw.get("response").unwrap_or(&Value::Null);
        let text = |value: Option<&Value>| {
            value
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        Some(Self {
            request_id,
            time_ms: raw.get("time").and_then(Value::as_u64).unwrap_or(0),
            method: text(request.get("method")),
            authority: text(request.get("authority")),
            path: text(request.get("path")),
            status: response
                .get("status")
                .and_then(Value::as_u64)
                .and_then(|status| u16::try_from(status).ok())
                .unwrap_or(0),
            bytes: body_size(request).saturating_add(body_size(response)),
            raw: raw.clone(),
        })
    }
}

/// 记录一侧（请求或响应）的正文字节数：`{"size": N, …}`，省略时按 0。
fn body_size(side: &Value) -> u64 {
    side.get("body")
        .and_then(|body| body.get("size"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

/// unix 毫秒 → `HH:MM:SS.mmm`（UTC）。
///
/// 自己算是刻意的：为一个时间格式引一个时区库，代价比"标清 UTC"大得多。
fn utc_hms(ms: u64) -> String {
    let seconds = ms / 1000;
    let day = seconds % 86_400;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        day / 3600,
        (day % 3600) / 60,
        day % 60,
        ms % 1000
    )
}

// --------------------------------------------------------------------------- #
// 记录表
// --------------------------------------------------------------------------- #

/// 记录表：只负责"有什么、怎么画"，会话状态在 [`DebugScreen`] 上。
struct RecordsTable {
    columns: Vec<Column>,
    rows: Vec<Record>,
    error: Option<SharedString>,
}

impl RecordsTable {
    fn new() -> Self {
        let columns = [
            ("time", "时间 (UTC)", 118.),
            ("method", "方法", 72.),
            ("authority", "主机", 186.),
            ("path", "路径", 520.),
            ("status", "状态", 68.),
            ("size", "大小", 96.),
        ]
        .into_iter()
        .map(|(key, name, width)| Column::new(key, name).resizable(true).width(px(width)))
        .collect();
        Self {
            columns,
            rows: Vec::new(),
            error: None,
        }
    }

    /// 追加一批增量：按 `request_id` 去重、保持到达顺序、超过上限从最旧裁。
    fn append(&mut self, records: impl IntoIterator<Item = Record>) -> bool {
        let mut changed = false;
        for record in records {
            if self
                .rows
                .iter()
                .any(|row| row.request_id == record.request_id)
            {
                continue;
            }
            self.rows.push(record);
            changed = true;
        }
        if changed && self.rows.len() > MAX_ROWS {
            let overflow = self.rows.len() - MAX_ROWS;
            self.rows.drain(..overflow);
        }
        changed
    }

    fn clear(&mut self) -> bool {
        let changed = !self.rows.is_empty() || self.error.is_some();
        self.rows.clear();
        self.error = None;
        changed
    }

    fn set_error(&mut self, message: String) -> bool {
        let changed = self.error.as_deref() != Some(message.as_str());
        self.error = Some(message.into());
        changed
    }

    pub fn error(&self) -> Option<SharedString> {
        self.error.clone()
    }

    fn record_at(&self, row_ix: usize) -> Option<&Record> {
        self.rows.get(row_ix)
    }
}

impl TableDelegate for RecordsTable {
    fn columns_count(&self, _: &App) -> usize {
        6
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.columns[col_ix].clone()
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(record) = self.rows.get(row_ix) else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let mono_family = cx.theme().mono_font_family.clone();
        let mono = |text: String| {
            div()
                .truncate()
                .font(Font {
                    family: mono_family.clone(),
                    ..Default::default()
                })
                .child(SharedString::from(text))
        };
        match col_ix {
            COL_TIME => cell(mono(utc_hms(record.time_ms)).text_color(muted)),
            COL_METHOD => cell(mono(record.method.clone()).font_weight(FontWeight::SEMIBOLD)),
            COL_AUTHORITY => cell(mono(record.authority.clone())),
            COL_PATH => cell(mono(record.path.clone())),
            COL_STATUS => {
                let label = if record.status == 0 {
                    "—".to_string()
                } else {
                    record.status.to_string()
                };
                cell(mono(label).text_color(status_color(record.status, cx)))
            }
            COL_SIZE => cell(
                div()
                    .truncate()
                    .text_color(muted)
                    .child(SharedString::from(format_bytes(record.bytes))),
            ),
            _ => div().into_any_element(),
        }
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        // 空态说明"下一步做什么"，而不是留一块白。
        let muted = cx.theme().muted_foreground;
        div()
            .id("debug-empty")
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .py_8()
            .child(div().text_sm().child("还没有记录"))
            .child(
                div().text_xs().text_color(muted).child(
                    "开始调试后，把客户端的代理指到该环境的端口走一遍请求，记录会出现在这里。",
                ),
            )
            .into_any_element()
    }
}

fn status_color(status: u16, cx: &App) -> Hsla {
    match status {
        200..=299 => cx.theme().success,
        300..=399 => cx.theme().muted_foreground,
        400..=499 => cx.theme().warning,
        500..=599 => cx.theme().danger,
        _ => cx.theme().muted_foreground,
    }
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

// --------------------------------------------------------------------------- #
// 详情面板（Sheet）
// --------------------------------------------------------------------------- #

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailTab {
    Request,
    Response,
}

impl DetailTab {
    const ALL: [Self; 2] = [Self::Request, Self::Response];

    fn label(self) -> &'static str {
        match self {
            Self::Request => "请求",
            Self::Response => "响应",
        }
    }
}

/// 单条记录的停靠详情：请求 / 响应两页签，各给起始行、头、正文。
struct CaptureDetail {
    record: Record,
    tab: DetailTab,
}

impl CaptureDetail {
    fn new(record: Record, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {
            record,
            tab: DetailTab::Request,
        }
    }

    fn rename(&mut self, record: Record, cx: &mut Context<Self>) {
        if self.record.request_id != record.request_id {
            self.record = record;
            cx.notify();
        }
    }

    fn select_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(tab) = DetailTab::ALL.get(index).copied() else {
            return;
        };
        if self.tab != tab {
            self.tab = tab;
            cx.notify();
        }
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let tabs = DetailTab::ALL
            .into_iter()
            .map(|tab| Tab::from(tab.label()))
            .collect::<Vec<_>>();
        let selected = DetailTab::ALL
            .iter()
            .position(|tab| *tab == self.tab)
            .unwrap_or(0);
        div()
            .child(
                TabBar::new("capture-detail-tabs")
                    .underline()
                    .children(tabs)
                    .selected_index(selected)
                    .on_click(cx.listener(|this, index, _, cx| {
                        this.select_tab(*index, cx);
                    })),
            )
            .into_any_element()
    }

    fn render_body(&self, cx: &App) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let mono = Font {
            family: cx.theme().mono_font_family.clone(),
            ..Default::default()
        };
        let side = match self.tab {
            DetailTab::Request => self.record.raw.get("request"),
            DetailTab::Response => self.record.raw.get("response"),
        };
        let Some(side) = side else {
            return div()
                .id("capture-side-missing")
                .text_sm()
                .text_color(muted)
                .child("这一侧没有内容（例如 101 切换协议：响应头之后是双向透传）。")
                .into_any_element();
        };

        let start_line = match self.tab {
            DetailTab::Request => format!(
                "{} {}",
                if self.record.method.is_empty() {
                    "?"
                } else {
                    &self.record.method
                },
                if self.record.path.is_empty() {
                    "/"
                } else {
                    &self.record.path
                }
            ),
            DetailTab::Response => format!(
                "HTTP/1.1 {}",
                if self.record.status == 0 {
                    "—".to_string()
                } else {
                    self.record.status.to_string()
                }
            ),
        };
        let mut head = vec![start_line, format!("Host: {}", self.record.authority)];
        if let Some(pairs) = side.get("headers").and_then(Value::as_array) {
            for pair in pairs {
                let Some(pair) = pair.as_array() else {
                    continue;
                };
                let Some(name) = pair.first().and_then(Value::as_str) else {
                    continue;
                };
                let value = pair.get(1).and_then(Value::as_str).unwrap_or("");
                head.push(format!("{name}: {value}"));
            }
        }

        let body = body_lines(side.get("body"));

        div()
            .id("capture-detail-body")
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .font(mono.clone())
                    .text_sm()
                    .children(head.into_iter().map(SharedString::from)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_xs().text_color(muted).child("正文"))
                    .child(
                        div()
                            .id("capture-detail-text")
                            .max_h(px(420.))
                            .overflow_y_scroll()
                            .font(mono)
                            .text_sm()
                            // 保留换行：Div 没有 whitespace_pre，按行拆子元素
                            // （空行填一个空格，否则那一行会被压成 0 高）。
                            .children(body.into_iter().map(|line| {
                                div().child(SharedString::from(if line.is_empty() {
                                    " ".to_string()
                                } else {
                                    line
                                }))
                            })),
                    ),
            )
            .into_any_element()
    }
}

/// 把 `{"encoding":"utf8"|"base64"|"omitted", "size", "content"}` 渲染成行。
fn body_lines(body: Option<&Value>) -> Vec<String> {
    let Some(body) = body else {
        return vec!["（无正文）".to_string()];
    };
    if body.get("omitted").and_then(Value::as_bool) == Some(true) {
        let size = body.get("size").and_then(Value::as_u64).unwrap_or(0);
        return vec![format!(
            "（正文 {} 超过抓包上限，未记录；需要看全文请缩小响应或分段验证）",
            format_bytes(size)
        )];
    }
    let content = body.get("content").and_then(Value::as_str).unwrap_or("");
    if content.is_empty() {
        return vec!["（空）".to_string()];
    }
    if body.get("encoding").and_then(Value::as_str) == Some("base64") {
        let size = body.get("size").and_then(Value::as_u64).unwrap_or(0);
        return vec![format!(
            "（二进制正文 {}，base64 编码——界面暂不还原二进制）",
            format_bytes(size)
        )];
    }
    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
    if lines.len() > MAX_BODY_LINES {
        let rest = lines.len() - MAX_BODY_LINES;
        lines.truncate(MAX_BODY_LINES);
        lines.push(format!("（已截断，还有 {rest} 行）"));
    }
    lines
}

impl Render for CaptureDetail {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self.render_tabs(cx);
        let meta = div()
            .id("capture-detail-meta")
            .flex()
            .items_center()
            .gap_2()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(SharedString::from(format!("#{}", self.record.request_id)))
            .child(SharedString::from(utc_hms(self.record.time_ms)))
            .child(SharedString::from(format_bytes(self.record.bytes)));
        let body = self.render_body(cx);
        div()
            .id("capture-detail")
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .size_full()
            .min_h_0()
            .child(meta)
            .child(tabs)
            .child(div().flex_1().min_h_0().child(body))
    }
}

// --------------------------------------------------------------------------- #
// 调试屏
// --------------------------------------------------------------------------- #

/// 调试屏：会话控制 + 记录表 + 详情面板。
pub struct DebugScreen {
    table: Entity<TableState<RecordsTable>>,
    /// 环境快照（与规则目录共用同一份；候选列表也从这里取，不另调门面）。
    envs: Entity<TableState<EnvTable>>,
    sheet: Option<Entity<CaptureDetail>>,
    target: Option<SharedString>,
    started_at: Option<u64>,
    session_id: Option<u64>,
    captured: u64,
    dropped: u64,
    /// 增量游标：记录身份（`request_id`），不是行号。
    cursor: Arc<AtomicU64>,
    poll: Option<Task<()>>,
    inflight: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl DebugScreen {
    pub fn new(
        envs: Entity<TableState<EnvTable>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let table =
            cx.new(|cx| TableState::new(RecordsTable::new(), window, cx).row_selectable(true));
        // 选中一行 → 打开（或换到）详情面板。
        let table_sub = cx.subscribe_in(&table, window, {
            let table = table.clone();
            move |this, _, event, window, cx| {
                let TableEvent::SelectRow(row_ix) = event else {
                    return;
                };
                let Some(record) = table.read(cx).delegate().record_at(*row_ix).cloned() else {
                    return;
                };
                this.open_detail(record, window, cx);
            }
        });
        Self {
            table,
            envs,
            sheet: None,
            target: None,
            started_at: None,
            session_id: None,
            captured: 0,
            dropped: 0,
            cursor: Arc::new(AtomicU64::new(0)),
            poll: None,
            inflight: Vec::new(),
            _subscriptions: vec![table_sub],
        }
    }

    /// 错误只有一条通道：表上的错误条（与目录屏同一手法）。
    fn report(&mut self, message: String, cx: &mut Context<Self>) {
        let table = self.table.clone();
        table.update(cx, |state, cx| {
            if state.delegate_mut().set_error(message) {
                state.refresh(cx);
            }
        });
        cx.notify();
    }

    fn open_detail(&mut self, record: Record, window: &mut Window, cx: &mut Context<Self>) {
        let title = format!(
            "请求 #{} · {}",
            record.request_id,
            if record.authority.is_empty() {
                record.path.clone()
            } else {
                record.authority.clone()
            }
        );
        let detail = match &self.sheet {
            Some(detail) => {
                detail.update(cx, |detail, cx| detail.rename(record, cx));
                detail.clone()
            }
            None => {
                let created = cx.new(|cx| CaptureDetail::new(record, window, cx));
                self.sheet = Some(created.clone());
                created
            }
        };
        window.open_sheet(cx, move |sheet, _, _| {
            sheet
                .title(title.clone())
                .size(px(560.))
                .overlay(false)
                .resizable(true)
                .child(detail.clone())
        });
    }

    /// 开始 / 切换调试会话。切换即换代（服务端保证：停旧 + 清旧 + 开新）。
    fn start(&mut self, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let admin = cx.global::<Host>().admin().clone();
        let body = serde_json::json!({ "env": name });
        let task = hop(
            window,
            cx,
            async move { admin.start_debug(&body).map_err(|error| error.message) },
            move |this, outcome, window, cx| match outcome {
                Ok(view) => this.adopt(view, window, cx),
                Err(message) => {
                    this.target = Some(name);
                    this.report(message, cx);
                }
            },
        );
        self.inflight.push(task);
    }

    /// 把服务端给的会话视图装进界面（开始/切换后的第一帧全量）。
    fn adopt(&mut self, view: DebugView, window: &mut Window, cx: &mut Context<Self>) {
        let records: Vec<Record> = view
            .capture
            .records
            .iter()
            .filter_map(Record::parse)
            .collect();
        self.target = Some(SharedString::from(view.env));
        self.started_at = Some(view.capture.session.started_at);
        self.session_id = Some(view.capture.session.id);
        self.captured = view.capture.captured;
        self.dropped = view.capture.dropped;
        let highest = records.last().map(|record| record.request_id).unwrap_or(0);
        self.cursor.store(highest, Ordering::Relaxed);
        let table = self.table.clone();
        table.update(cx, |state, cx| {
            state.delegate_mut().clear();
            state.delegate_mut().append(records);
            state.refresh(cx);
        });
        self.start_poll(window, cx);
        cx.notify();
    }

    fn stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let admin = cx.global::<Host>().admin().clone();
        let task = hop(
            window,
            cx,
            async move { admin.stop_debug().map_err(|error| error.message) },
            move |this, outcome, window, cx| {
                if let Err(message) = outcome {
                    this.report(message, cx);
                    return;
                }
                this.reset_session(window, cx);
            },
        );
        self.inflight.push(task);
    }

    fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone() else {
            return;
        };
        let admin = cx.global::<Host>().admin().clone();
        let table = self.table.clone();
        let task = hop(
            window,
            cx,
            async move { admin.clear_capture(&target).map_err(|error| error.message) },
            move |this, outcome, _, cx| match outcome {
                Ok(_) => {
                    table.update(cx, |state, cx| {
                        if state.delegate_mut().clear() {
                            state.refresh(cx);
                        }
                    });
                    this.captured = 0;
                    // 清空后 request_id 从头计；游标必须跟着归零，否则"下一批"永远空。
                    this.cursor.store(0, Ordering::Relaxed);
                    cx.notify();
                }
                Err(message) => this.report(message, cx),
            },
        );
        self.inflight.push(task);
    }

    fn reset_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.target = None;
        self.started_at = None;
        self.session_id = None;
        self.captured = 0;
        self.dropped = 0;
        self.cursor.store(0, Ordering::Relaxed);
        self.poll = None;
        self.sheet = None;
        let table = self.table.clone();
        table.update(cx, |state, cx| {
            if state.delegate_mut().clear() {
                state.refresh(cx);
            }
        });
        // 停掉会话就没有"在看的记录"了：面板留着只会显示过时数据。
        window.close_sheet(cx);
        cx.notify();
    }

    /// 起增量轮询。与快照轮询同一手法：tokio 上跑，`mpsc(1)` 丢帧，
    /// 会话消失（None）时循环自己退出。
    ///
    /// 游标放 `Arc<AtomicU64>`：清空记录要能把它归零，而 tokio 那边的循环不该为了
    /// 这件事再跨一次线程。
    fn start_poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let admin = cx.global::<Host>().admin().clone();
        let handle = cx.global::<Host>().handle().clone();
        let cursor = Arc::clone(&self.cursor);
        let (sender, mut receiver) =
            tokio::sync::mpsc::channel::<Option<(String, CaptureDelta)>>(1);
        handle.spawn(async move {
            let mut tick = tokio::time::interval(POLL);
            loop {
                tick.tick().await;
                let after = cursor.load(Ordering::Relaxed);
                let delta = admin.debug_delta(after, DELTA_LIMIT);
                let alive = delta.is_some();
                // 丢帧：增量有界，积压没有意义（下一拍会取到更新的游标之后）。
                if sender.try_send(delta).is_err() && !alive {
                    break;
                }
                if !alive {
                    break;
                }
            }
        });

        let table = self.table.clone();
        self.poll = Some(cx.spawn_in(window, async move |this, cx| {
            while let Some(frame) = receiver.recv().await {
                let Some((_, delta)) = frame else {
                    // 会话消失（目标没在跑 / 被停掉）：保留已捕获的记录，说明原因。
                    let _ = this.update_in(cx, |this, _, cx| {
                        this.target = None;
                        this.session_id = None;
                        this.report(
                            "调试会话已失效：目标环境不在运行，或会话已被停止".to_string(),
                            cx,
                        );
                    });
                    continue;
                };
                let highest = delta
                    .records
                    .iter()
                    .filter_map(|raw| raw.get("request_id").and_then(Value::as_u64))
                    .max();
                let records: Vec<Record> = delta.records.iter().filter_map(Record::parse).collect();
                let changed = table.update_in(cx, |state, _, cx| {
                    let changed = state.delegate_mut().append(records);
                    if changed {
                        state.refresh(cx);
                    }
                    changed
                });
                let _ = this.update_in(cx, |this, _, cx| {
                    this.dropped = delta.dropped;
                    this.captured = delta.captured;
                    this.session_id = Some(delta.session.id);
                    this.started_at = Some(delta.session.started_at);
                    if let Some(highest) = highest {
                        this.cursor.fetch_max(highest, Ordering::Relaxed);
                    }
                    if changed.unwrap_or(false) {
                        cx.notify();
                    }
                });
            }
        }));
    }

    /// 会话状态行：有会话说会话，没会话说"选一个环境"。
    fn status_line(&self) -> String {
        match (&self.target, self.session_id) {
            (Some(target), Some(id)) => {
                let started = self
                    .started_at
                    .map(|at| format!(" · 起始 {} (UTC)", utc_hms(at)))
                    .unwrap_or_default();
                let _ = target;
                format!(
                    "会话 #{id}{started} · 已捕获 {} · 丢弃 {}",
                    self.captured, self.dropped
                )
            }
            (Some(_), None) => "会话未建立：目标环境还没起来".to_string(),
            (None, _) => "未开始：选一个运行中的环境".to_string(),
        }
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let view = cx.weak_entity();
        // 候选来自环境快照（已在轮询），渲染期不调门面。
        let targets = self.envs.read(cx).delegate().debug_targets();
        let has_live = targets.iter().any(|(_, live)| *live);
        let current = self.target.clone();
        let has_session = self.session_id.is_some();

        let picker = {
            let this = view.clone();
            let current = current.clone();
            Button::new("debug-target")
                .xsmall()
                .outline()
                .icon(Icon::new(IconName::Network))
                .label(match &current {
                    Some(target) => format!("目标：{target}"),
                    None => "选择环境".to_string(),
                })
                .tooltip("调试目标环境（只有运行中的环境能调试）")
                .dropdown_menu(move |mut menu, _, _| {
                    if targets.is_empty() {
                        return menu.item(PopupMenuItem::new("还没有环境").disabled(true));
                    }
                    for (name, live) in targets.clone() {
                        let this = this.clone();
                        let selected = current.as_deref() == Some(name.as_str());
                        let label = if live {
                            name.to_string()
                        } else {
                            format!("{name}（未运行）")
                        };
                        let target = name.clone();
                        menu = menu.item(
                            PopupMenuItem::new(label)
                                .checked(selected)
                                .disabled(!live)
                                .on_click(move |_, window, cx| {
                                    let target = target.clone();
                                    let _ =
                                        this.update(cx, |view, cx| view.start(target, window, cx));
                                }),
                        );
                    }
                    menu
                })
        };

        div()
            .id("debug-toolbar")
            .flex()
            .items_center()
            .gap_2()
            .child(picker)
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(SharedString::from(self.status_line())),
            )
            .child(div().flex_1())
            .child(
                Button::new("debug-clear")
                    .xsmall()
                    .ghost()
                    .icon(Icon::new(IconName::Delete))
                    .label("清空")
                    .tooltip("清空当前会话已捕获的记录")
                    .disabled(!has_session)
                    .on_click({
                        let this = view.clone();
                        move |_, window, cx| {
                            let _ = this.update(cx, |view, cx| view.clear(window, cx));
                        }
                    }),
            )
            .child(if has_session {
                Button::new("debug-stop")
                    .xsmall()
                    .outline()
                    .icon(Icon::new(IconName::Pause))
                    .label("停止")
                    .tooltip("停止调试并清空记录")
                    .on_click({
                        let this = view.clone();
                        move |_, window, cx| {
                            let _ = this.update(cx, |view, cx| view.stop(window, cx));
                        }
                    })
                    .into_any_element()
            } else {
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(if has_live {
                        "从「选择环境」开始调试"
                    } else {
                        "先到「环境」把一个环境跑起来"
                    })
                    .into_any_element()
            })
            .into_any_element()
    }
}

impl Render for DebugScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let error = self.table.read(cx).delegate().error();
        let toolbar = self.render_toolbar(cx);

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap_2()
            .p_4()
            .child(toolbar)
            .children(error.map(|message| {
                div()
                    .id("debug-error")
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(message)
            }))
            .child(div().flex_1().min_h_0().child(DataTable::new(&self.table)))
    }
}

#[cfg(test)]
mod tests {
    use super::{Record, body_lines, utc_hms};
    use serde_json::json;

    #[test]
    fn times_are_formatted_as_utc_wall_clock() {
        assert_eq!(utc_hms(0), "00:00:00.000");
        // 1970-01-02 00:00:00 UTC —— 跨天也要回到表盘起点。
        assert_eq!(utc_hms(86_400_000), "00:00:00.000");
        assert_eq!(utc_hms(3_661_500), "01:01:01.500");
        assert_eq!(
            utc_hms(1_700_000_000_123),
            // 2023-11-14T22:13:20.123Z
            "22:13:20.123"
        );
    }

    #[test]
    fn records_parse_the_shape_the_engine_writes() {
        let raw = json!({
            "version": 1,
            "session": 7,
            "request_id": 128,
            "time": 1_700_000_000_123u64,
            "request": {
                "method": "GET",
                "path": "/api/items?q=1",
                "authority": "api.example.com",
                "headers": [["host", "api.example.com"]],
                "body": {"encoding": "utf8", "size": 0, "content": ""}
            },
            "response": {
                "status": 200,
                "headers": [["content-type", "application/json"]],
                "body": {"encoding": "utf8", "size": 12, "content": "{\"items\":[]}"}
            },
            "error": null
        });
        let record = Record::parse(&raw).expect("引擎写的形状必须能解析");
        assert_eq!(record.request_id, 128);
        assert_eq!(record.method, "GET");
        assert_eq!(record.authority, "api.example.com");
        assert_eq!(record.path, "/api/items?q=1");
        assert_eq!(record.status, 200);
        assert_eq!(record.bytes, 12);
    }

    #[test]
    fn a_record_without_response_shows_a_dash_not_a_zero() {
        let raw = json!({
            "request_id": 1,
            "time": 0,
            "request": {"method": "CONNECT", "path": "x", "authority": "h"},
            "response": null
        });
        let record = Record::parse(&raw).expect("没有响应也要能进表");
        assert_eq!(record.status, 0);
        assert_eq!(record.bytes, 0);
    }

    #[test]
    fn bodies_render_text_omitted_and_binary_distinctly() {
        let text = body_lines(Some(&json!({
            "encoding": "utf8", "size": 5, "content": "a\n\nb"
        })));
        assert_eq!(text, vec!["a".to_string(), "".to_string(), "b".to_string()]);

        let omitted = body_lines(Some(&json!({"omitted": true, "size": 300_000})));
        assert!(omitted[0].contains("上限"), "{omitted:?}");

        let binary = body_lines(Some(&json!({
            "encoding": "base64", "size": 4, "content": "AAECAw=="
        })));
        assert!(binary[0].contains("二进制"), "{binary:?}");

        assert_eq!(body_lines(None), vec!["（无正文）".to_string()]);
    }
}

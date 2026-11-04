//! 调试屏：工作区级抓包会话的**看**与**控**。
//!
//! 主任务只有一个：*让某个环境上的请求现形，并能读一条的请求与响应*。
//! 界面因此是「工具条 + 筛选条 + 记录表 + 停靠详情面板」：
//!
//! * 记录表：时间（UTC）/ 方法 / 主机 / 路径 / 状态 / 耗时 / 大小；
//! * 筛选条：方法 / 状态档 / 主机三个维度可叠加，只筛视图不动会话缓冲
//!   （与快照无关的纯本地计算，过滤选项也从当前记录派生）；
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
    IntoElement, KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, Subscription, Task, WeakEntity, Window, div, px,
};
use serde_json::Value;

use envboard_protocol::{CaptureDelta, DebugView};

use crate::display::format_bytes;
use crate::display::format_duration;
use crate::errors::{Action, Notice};
use crate::host::{Host, hop};
use crate::views::environments::EnvTable;
use crate::views::error_banner;
use crate::views::text_tooltip;

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
const COL_DURATION: usize = 5;
const COL_SIZE: usize = 6;

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
    /// 上游交换耗时（请求行到达 → 响应体收齐）；老记录与手工构造的 JSON
    /// 没有这个字段时按 0（显示「—」）。
    duration_ms: u64,
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
            duration_ms: raw.get("duration_ms").and_then(Value::as_u64).unwrap_or(0),
            bytes: body_size(request).saturating_add(body_size(response)),
            raw: raw.clone(),
        })
    }

    /// 有没有拿到响应：没响应就没有"耗时"可言（0 是个合法耗时，不能拿它当哨兵）。
    fn has_response(&self) -> bool {
        self.status != 0 || self.raw.get("response").is_some_and(|r| !r.is_null())
    }
}

/// unix 毫秒 → `YYYY-MM-DD HH:MM:SS.mmm`（UTC）。
///
/// 表里只给时分秒（列宽宝贵），完整日期放在悬停提示与详情面板的 meta 上 ——
/// 跨天的长会话不该出现"两条 09:15 分不清是哪天"。
fn utc_datetime(ms: u64) -> String {
    let (year, month, day) = utc_ymd(ms);
    format!("{year:04}-{month:02}-{day:02} {}", utc_hms(ms))
}

/// unix 毫秒 → `(年, 月, 日)`（UTC）。Howard Hinnant 的 `civil_from_days`：
/// 自己算是刻意的（见 [`utc_hms`]），换成时区库只为一个日期不值当。
fn utc_ymd(ms: u64) -> (i64, u64, u64) {
    let days = (ms / 86_400_000) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (year + i64::from(month <= 2), month, day)
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

/// 状态档（筛选用）。"无响应"单列一档：**还没回来**与**回了个 5xx** 是两件事，
/// 混进「其他」只会让筛选结果说不清。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusClass {
    Pending,
    Class2xx,
    Class3xx,
    Class4xx,
    Class5xx,
    Other,
}

impl StatusClass {
    const ALL: [Self; 6] = [
        Self::Pending,
        Self::Class2xx,
        Self::Class3xx,
        Self::Class4xx,
        Self::Class5xx,
        Self::Other,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Pending => "无响应",
            Self::Class2xx => "2xx",
            Self::Class3xx => "3xx",
            Self::Class4xx => "4xx",
            Self::Class5xx => "5xx",
            Self::Other => "其他",
        }
    }

    fn of(status: u16) -> Self {
        match status {
            0 => Self::Pending,
            200..=299 => Self::Class2xx,
            300..=399 => Self::Class3xx,
            400..=499 => Self::Class4xx,
            500..=599 => Self::Class5xx,
            _ => Self::Other,
        }
    }
}

/// 记录筛选：三个维度可叠加，默认全空 = 不过滤。
///
/// 只筛**视图**：会话缓冲、游标与已捕获计数都不动 —— 调筛选不该让任何人
/// 以为记录被清掉了。
#[derive(Debug, Clone, Default, PartialEq)]
struct RecordFilter {
    method: Option<String>,
    status: Option<StatusClass>,
    authority: Option<String>,
}

impl RecordFilter {
    fn is_empty(&self) -> bool {
        self.method.is_none() && self.status.is_none() && self.authority.is_none()
    }

    fn matches(&self, record: &Record) -> bool {
        self.method
            .as_deref()
            .is_none_or(|method| record.method == method)
            && self
                .status
                .is_none_or(|status| status == StatusClass::of(record.status))
            && self
                .authority
                .as_deref()
                .is_none_or(|authority| record.authority == authority)
    }
}

/// 记录表：只负责"有什么、怎么画"，会话状态在 [`DebugScreen`] 上。
struct RecordsTable {
    columns: Vec<Column>,
    rows: Vec<Record>,
    /// `rows` 的下标（筛选后的顺序）。表只渲染可见行，行下标也指这里 ——
    /// 选中行 → 记录必须经这张表换算，不能直接拿 `rows[row_ix]`。
    visible: Vec<usize>,
    filter: RecordFilter,
    error: Option<Notice>,
}

impl RecordsTable {
    fn new() -> Self {
        // 列宽合计 900 = 1180 窗口的内容区（1180 − 侧栏 248 − 内边距 32），
        // 右端不会剩一条空白列。
        //
        // 除了「路径」，其余六列在**最小窗口**（1000，内容区 720）下也都放得下：
        // 110 + 64 + 170 + 60 + 64 + 80 = 548。路径是唯一会被裁的那一列，它也是
        // 唯一能安全截断的列（详情面板里有全文）。
        let columns = vec![
            Column::new("time", "时间 (UTC)")
                .resizable(true)
                .width(px(110.)),
            Column::new("method", "方法").resizable(true).width(px(64.)),
            Column::new("authority", "主机")
                .resizable(true)
                .width(px(170.)),
            Column::new("path", "路径").resizable(true).width(px(352.)),
            Column::new("status", "状态").resizable(true).width(px(60.)),
            Column::new("duration", "耗时")
                .resizable(true)
                .width(px(64.)),
            Column::new("size", "大小").resizable(true).width(px(80.)),
        ];
        Self {
            columns,
            rows: Vec::new(),
            visible: Vec::new(),
            filter: RecordFilter::default(),
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
        if changed {
            self.rebuild_visible();
        }
        changed
    }

    fn clear(&mut self) -> bool {
        let changed = !self.rows.is_empty() || self.error.is_some();
        self.rows.clear();
        self.visible.clear();
        // 记录都没了，筛选选项也就没意义；留着只会是一组指向空结果的条件。
        self.filter = RecordFilter::default();
        self.error = None;
        changed
    }

    fn set_filter(&mut self, filter: RecordFilter) -> bool {
        if self.filter == filter {
            return false;
        }
        self.filter = filter;
        self.rebuild_visible();
        true
    }

    fn filter(&self) -> &RecordFilter {
        &self.filter
    }

    /// 筛选下拉的选项：从**当前记录**派生（方法去重排序、主机去重排序）。
    /// 不从协议或配置猜：会话里没出现过的方法不该出现在菜单里。
    fn filter_options(&self) -> (Vec<String>, Vec<String>) {
        let mut methods: Vec<String> = Vec::new();
        let mut authorities: Vec<String> = Vec::new();
        for record in &self.rows {
            if !methods.contains(&record.method) {
                methods.push(record.method.clone());
            }
            if !authorities.contains(&record.authority) {
                authorities.push(record.authority.clone());
            }
        }
        methods.sort();
        authorities.sort();
        (methods, authorities)
    }

    fn rebuild_visible(&mut self) {
        self.visible = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, record)| self.filter.matches(record))
            .map(|(index, _)| index)
            .collect();
    }

    fn set_error(&mut self, notice: Notice) -> bool {
        let changed = self.error.as_ref() != Some(&notice);
        self.error = Some(notice);
        changed
    }

    pub fn error(&self) -> Option<Notice> {
        self.error.clone()
    }

    /// 会话缓冲里的总条数（不受筛选影响）。
    fn total(&self) -> usize {
        self.rows.len()
    }

    fn visible_count(&self) -> usize {
        self.visible.len()
    }

    fn record_at(&self, row_ix: usize) -> Option<&Record> {
        self.visible.get(row_ix).map(|&index| &self.rows[index])
    }
}

impl TableDelegate for RecordsTable {
    fn columns_count(&self, _: &App) -> usize {
        7
    }

    fn rows_count(&self, _: &App) -> usize {
        self.visible.len()
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
        let Some(record) = self.record_at(row_ix) else {
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
            COL_TIME => cell(
                div()
                    // 列里只放时分秒；完整日期（跨天会话要靠它）在悬停提示上。
                    .id(SharedString::from(format!(
                        "debug-time-{}",
                        record.request_id
                    )))
                    .tooltip(text_tooltip(format!(
                        "{} UTC",
                        utc_datetime(record.time_ms)
                    )))
                    .child(mono(utc_hms(record.time_ms)).text_color(muted)),
            ),
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
            COL_DURATION => {
                // 没有响应就还没有耗时：显示「—」而不是「0 ms」，
                // 后者会被读成"秒回"。
                let label = if record.has_response() {
                    format_duration(record.duration_ms)
                } else {
                    "—".to_string()
                };
                cell(mono(label).text_color(muted))
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
        // 有记录但都被筛掉了：这不是"还没有记录"，说错会让人以为抓包没开始。
        let filtered = !self.rows.is_empty();
        let (title, hint) = if filtered {
            (
                "没有匹配的记录",
                "调整或清除上面的筛选条件；会话缓冲里的记录没有被删除。",
            )
        } else {
            (
                "还没有记录",
                "开始调试后，把客户端的代理指到该环境的端口走一遍请求，记录会出现在这里。",
            )
        };
        div()
            .id("debug-empty")
            .flex()
            .flex_col()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .p_6()
            .child(div().text_sm().child(title))
            .child(div().text_xs().text_color(muted).child(hint))
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

/// 一条筛选菜单项：点下去把整份筛选条件换成 `filter`（三个维度一次写完，
/// 不依赖"当前条件是谁给的"——菜单项构造时就把结果算定）。
fn filter_menu_item(
    label: impl Into<SharedString>,
    checked: bool,
    filter: RecordFilter,
    view: WeakEntity<DebugScreen>,
) -> PopupMenuItem {
    PopupMenuItem::new(label)
        .checked(checked)
        .on_click(move |_, _, cx| {
            let filter = filter.clone();
            let _ = view.update(cx, |view, cx| view.set_filter(filter, cx));
        })
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
            .child(SharedString::from(format!(
                "{} UTC",
                utc_datetime(self.record.time_ms)
            )))
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
    /// 筛选条是否展开。默认收起：不筛的时候它只是一行占地方的控件；
    /// 已经有筛选条件时无论展开与否都显示（否则条件没入口可改回）。
    filters_open: bool,
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
                // 键盘 ↑/↓ 也会 emit SelectRow：那只是**移动高亮**，不该每移一行
                // 就重开/换一次详情面板（面板会抢焦点，键盘浏览立刻断掉）。
                // 键盘打开当前行走 Enter（见 `render` 里的 on_key_down）。
                if window.last_input_was_keyboard() {
                    return;
                }
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
            filters_open: false,
            poll: None,
            inflight: Vec::new(),
            _subscriptions: vec![table_sub],
        }
    }

    /// 错误只有一条通道：表上的错误条（与目录屏同一手法）。
    fn report(&mut self, notice: Notice, cx: &mut Context<Self>) {
        let table = self.table.clone();
        table.update(cx, |state, cx| {
            if state.delegate_mut().set_error(notice) {
                state.refresh(cx);
            }
        });
        cx.notify();
    }

    /// 改筛选：只重算可见行，**不动会话缓冲与游标**（筛选不该看起来像清空）。
    fn set_filter(&mut self, filter: RecordFilter, cx: &mut Context<Self>) {
        let table = self.table.clone();
        table.update(cx, |state, cx| {
            if state.delegate_mut().set_filter(filter) {
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
    ///
    /// 这里只负责真正发起；"切走会丢掉已抓记录"的确认在 [`Self::request_start`]。
    fn start(&mut self, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let admin = cx.global::<Host>().admin().clone();
        let body = serde_json::json!({ "env": name });
        let task = hop(
            window,
            cx,
            async move { admin.start_debug(&body) },
            move |this, outcome, window, cx| match outcome {
                Ok(view) => {
                    crate::views::push_success_toast(
                        Action::StartDebug,
                        Some(&name),
                        window,
                        cx,
                        "debug-write",
                    );
                    this.adopt(view, window, cx);
                }
                Err(error) => {
                    // 失败不留"已选"的幽灵状态：按钮回到「选择环境」，原因进错误条。
                    this.target = None;
                    this.report(
                        Notice::from_error(Action::StartDebug, Some(&name), &error),
                        cx,
                    );
                }
            },
        );
        self.inflight.push(task);
    }

    /// 切换目标前的把关：已抓到的记录会被换代清掉，先问一句。
    ///
    /// 只有"已有会话 + 换了目标 + 真有记录"三个条件同时成立才弹确认；
    /// 首次开始、或重选同一个目标（那只是重开一个空会话）都不烦人。
    fn request_start(&mut self, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.table.read(cx).delegate().total();
        let switching = self.session_id.is_some() && self.target.as_deref() != Some(name.as_str());
        if !switching || rows == 0 {
            self.start(name, window, cx);
            return;
        }
        let view = cx.weak_entity();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let view = view.clone();
            let name = name.clone();
            dialog
                .confirm()
                .title(format!("切换到 {name} 调试？"))
                .description(format!(
                    "当前会话已捕获 {rows} 条记录；切换目标会停掉旧会话并清空这些记录。此操作不可撤销。"
                ))
                .ok_text("切换")
                .on_ok(move |_, window, cx| {
                    let name = name.clone();
                    view.update(cx, |view, cx| view.start(name, window, cx))
                        .is_ok()
                })
        });
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
            async move { admin.stop_debug() },
            move |this, outcome, window, cx| {
                if let Err(error) = outcome {
                    this.report(Notice::from_error(Action::StopDebug, None, &error), cx);
                    return;
                }
                crate::views::push_success_toast(
                    Action::StopDebug,
                    None,
                    window,
                    cx,
                    "debug-write",
                );
                this.reset_session(window, cx);
            },
        );
        self.inflight.push(task);
    }

    fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let table = self.table.clone();
        let Some(target) = self.target.clone() else {
            // 会话已失效（目标停了 / 会话被拿走）但记录还留着：服务端那份没得清，
            // 本地这一份仍要能清掉 —— 否则它们会一直挂在表里，而按钮又因为
            // "没有会话"被禁用，等于把用户困住。
            table.update(cx, |state, cx| {
                if state.delegate_mut().clear() {
                    state.refresh(cx);
                }
            });
            self.captured = 0;
            self.cursor.store(0, Ordering::Relaxed);
            crate::views::push_success_toast(Action::ClearCapture, None, window, cx, "debug-write");
            cx.notify();
            return;
        };
        let admin = cx.global::<Host>().admin().clone();
        let subject = target.clone();
        let task = hop(
            window,
            cx,
            async move { admin.clear_capture(&target) },
            move |this, outcome, window, cx| match outcome {
                Ok(_) => {
                    table.update(cx, |state, cx| {
                        if state.delegate_mut().clear() {
                            state.refresh(cx);
                        }
                    });
                    this.captured = 0;
                    // 清空后 request_id 从头计；游标必须跟着归零，否则"下一批"永远空。
                    this.cursor.store(0, Ordering::Relaxed);
                    crate::views::push_success_toast(
                        Action::ClearCapture,
                        Some(&subject),
                        window,
                        cx,
                        "debug-write",
                    );
                    cx.notify();
                }
                Err(error) => this.report(
                    Notice::from_error(Action::ClearCapture, Some(&subject), &error),
                    cx,
                ),
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
                            // 这条不是门面错误：会话消失是轮询自己观察到的，
                            // 没有错误码也没有英文原文，只给一句中文。
                            Notice::plain("调试会话已失效：目标环境不在运行，或会话已被停止"),
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
    ///
    /// `rows` 是表里还留着的记录数：会话失效后记录仍可查看，这时说"未开始"
    /// 会让人以为那些记录不存在了。
    fn status_line(&self, rows: usize) -> String {
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
            (None, _) if rows > 0 => {
                format!("会话已失效：{rows} 条记录仍在，可清空后重选环境")
            }
            (None, _) => "未开始：选一个运行中的环境".to_string(),
        }
    }

    /// 筛选条：方法 / 状态档 / 主机三个维度可叠加。
    ///
    /// 选项从**当前记录**派生（会话里没出现过的方法不列），选中即生效；
    /// 右端显示命中数/总数，并给「清除筛选」。筛的是视图，会话不受影响。
    fn render_filters(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let (filter, methods, authorities, total, shown) = {
            let delegate = self.table.read(cx).delegate();
            let (methods, authorities) = delegate.filter_options();
            (
                delegate.filter().clone(),
                methods,
                authorities,
                delegate.total(),
                delegate.visible_count(),
            )
        };
        let view = cx.weak_entity();

        // 方法
        let method = {
            let this = view.clone();
            let base = filter.clone();
            let methods = methods.clone();
            let active = base.method.is_some();
            let button = Button::new("debug-filter-method")
                .xsmall()
                .label(match &base.method {
                    Some(method) => format!("方法：{method}"),
                    None => "方法：全部".to_string(),
                })
                .tooltip("按请求方法筛选");
            let button = if active {
                button.outline()
            } else {
                button.ghost()
            };
            button
                .disabled(total == 0)
                .dropdown_menu(move |mut menu, _, _| {
                    let this = this.clone();
                    let base = base.clone();
                    // 「全部方法」= 清掉这一维，其余两维保持不变。
                    let mut cleared = base.clone();
                    cleared.method = None;
                    menu = menu.item(filter_menu_item(
                        "全部方法",
                        base.method.is_none(),
                        cleared,
                        this.clone(),
                    ));
                    for method in methods.clone() {
                        let mut next = base.clone();
                        next.method = Some(method.clone());
                        let checked = base.method.as_deref() == Some(method.as_str());
                        menu = menu.item(filter_menu_item(method, checked, next, this.clone()));
                    }
                    menu
                })
        };

        // 状态档（2xx / 3xx / 4xx / 5xx / 无响应 / 其他）
        let status = {
            let this = view.clone();
            let base = filter.clone();
            let active = base.status.is_some();
            let button = Button::new("debug-filter-status")
                .xsmall()
                .label(match base.status {
                    Some(status) => format!("状态：{}", status.label()),
                    None => "状态：全部".to_string(),
                })
                .tooltip("按响应状态档筛选");
            let button = if active {
                button.outline()
            } else {
                button.ghost()
            };
            button
                .disabled(total == 0)
                .dropdown_menu(move |mut menu, _, _| {
                    let this = this.clone();
                    let base = base.clone();
                    let mut cleared = base.clone();
                    cleared.status = None;
                    menu = menu.item(filter_menu_item(
                        "全部状态",
                        base.status.is_none(),
                        cleared,
                        this.clone(),
                    ));
                    for status in StatusClass::ALL {
                        let mut next = base.clone();
                        next.status = Some(status);
                        menu = menu.item(filter_menu_item(
                            status.label(),
                            base.status == Some(status),
                            next,
                            this.clone(),
                        ));
                    }
                    menu
                })
        };

        // 主机（去重排序；会话里出现几个就列几个）
        let authority = {
            let this = view.clone();
            let base = filter.clone();
            let authorities = authorities.clone();
            let active = base.authority.is_some();
            let button = Button::new("debug-filter-authority")
                .xsmall()
                .label(match &base.authority {
                    Some(authority) => format!("主机：{authority}"),
                    None => "主机：全部".to_string(),
                })
                .tooltip("按目标主机筛选");
            let button = if active {
                button.outline()
            } else {
                button.ghost()
            };
            button
                .disabled(total == 0)
                .dropdown_menu(move |mut menu, _, _| {
                    let this = this.clone();
                    let base = base.clone();
                    let mut cleared = base.clone();
                    cleared.authority = None;
                    menu = menu.item(filter_menu_item(
                        "全部主机",
                        base.authority.is_none(),
                        cleared,
                        this.clone(),
                    ));
                    for authority in authorities.clone() {
                        let mut next = base.clone();
                        next.authority = Some(authority.clone());
                        let checked = base.authority.as_deref() == Some(authority.as_str());
                        menu = menu.item(filter_menu_item(authority, checked, next, this.clone()));
                    }
                    menu
                })
        };

        let summary = if filter.is_empty() {
            format!("{total} 条记录")
        } else {
            format!("筛选命中 {shown} / {total} 条")
        };
        let clear = (!filter.is_empty()).then(|| {
            let this = view.clone();
            Button::new("debug-filter-clear")
                .xsmall()
                .ghost()
                .icon(Icon::new(IconName::Close))
                .label("清除筛选")
                .on_click(move |_, _, cx| {
                    let _ =
                        this.update(cx, |view, cx| view.set_filter(RecordFilter::default(), cx));
                })
                .into_any_element()
        });

        div()
            .id("debug-filters")
            .flex()
            .items_center()
            .gap_2()
            // 固定宽度：标签文字随命中值变化，宽度跟着跳会让整行在打字时抖动。
            .child(div().w(px(170.)).min_w_0().child(method))
            .child(div().w(px(110.)).min_w_0().child(status))
            .child(div().w(px(230.)).min_w_0().child(authority))
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(SharedString::from(summary)),
            )
            .children(clear)
            .into_any_element()
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let view = cx.weak_entity();
        // 候选来自环境快照（已在轮询），渲染期不调门面。
        let targets = self.envs.read(cx).delegate().debug_targets();
        let has_live = targets.iter().any(|(_, live)| *live);
        let current = self.target.clone();
        let has_session = self.session_id.is_some();
        // 清空按钮看的是"有没有记录"，不是"有没有会话"：会话失效后记录仍留在
        // 表里，禁用按钮会把它们困在那里。
        let record_count = self.table.read(cx).delegate().total();
        let has_records = record_count > 0;

        let picker = {
            let this = view.clone();
            let current = current.clone();
            let button = Button::new("debug-target")
                .xsmall()
                .icon(Icon::new(IconName::Network));
            // 没会话时「选择环境」就是这一屏唯一的主操作（按下即开始调试），
            // 用 primary 把它从工具条里抬出来；有会话时它只是切换目标的手段。
            let button = if has_session {
                button.outline()
            } else {
                button.primary()
            };
            button
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
                                    let _ = this.update(cx, |view, cx| {
                                        view.request_start(target, window, cx)
                                    });
                                }),
                        );
                    }
                    menu
                })
        };

        // 筛选条的开关：收起时不占那一行（条件本身有筛选时会强制显示）。
        let active_dims = {
            let filter = self.table.read(cx).delegate().filter();
            usize::from(filter.method.is_some())
                + usize::from(filter.status.is_some())
                + usize::from(filter.authority.is_some())
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
                    .child(SharedString::from(self.status_line(record_count))),
            )
            .child(div().flex_1())
            .child({
                let this = view.clone();
                let label = if active_dims == 0 {
                    "筛选".to_string()
                } else {
                    format!("筛选 ({active_dims})")
                };
                Button::new("debug-filter-toggle")
                    .xsmall()
                    .ghost()
                    .icon(Icon::new(gpui_kit::assets::IconName::Funnel))
                    .label(label)
                    .tooltip("按方法 / 状态档 / 主机筛选记录")
                    .on_click(move |_, _, cx| {
                        let _ = this.update(cx, |view, cx| {
                            view.filters_open = !view.filters_open;
                            cx.notify();
                        });
                    })
            })
            .child(
                // 「清空」不配垃圾桶图标：垃圾桶读起来是"删除这些记录"，
                // 而这里清的是**会话缓冲**，服务端那份账本不受影响。
                Button::new("debug-clear")
                    .xsmall()
                    .ghost()
                    .label("清空记录")
                    .tooltip("清空当前会话已捕获的记录")
                    .disabled(!has_records)
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
            } else if has_live {
                // 有可调试的环境时不再说话：左侧状态行已经写了"未开始：选一个
                // 运行中的环境"，主操作就是左边那个已高亮的按钮。同一句话
                // 在一行里说两遍，只会让两处都不可信。
                div().into_any_element()
            } else {
                // 一个能跑的环境都没有 —— 这是**另一件事**，要说出来，
                // 而且要说清去哪儿解决。
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("先到「环境」把一个环境跑起来")
                    .into_any_element()
            })
            .into_any_element()
    }
}

impl Render for DebugScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let error = self.table.read(cx).delegate().error();
        let has_records = self.table.read(cx).delegate().total() > 0;
        let filter_active = !self.table.read(cx).delegate().filter().is_empty();
        let toolbar = self.render_toolbar(cx);
        // 没有会话也没有记录时不摆三条筛不了任何东西的下拉：那条筛选条只会
        // 让"还没开始"看起来像"没有匹配"。收起态同理 —— 只有真有条件时
        // 才强制显示，否则条件没有入口可改回。
        let filters = (self.session_id.is_some() || has_records)
            .then_some(())
            .filter(|_| self.filters_open || filter_active)
            .map(|_| self.render_filters(cx));

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap_2()
            .p_4()
            .child(toolbar)
            .children(filters)
            .children(error.map(|message| error_banner("debug-error", message)))
            .child(
                div()
                    .id("debug-table")
                    .flex_1()
                    .min_h_0()
                    // Enter 打开当前选中行的详情（↑/↓ 只移动高亮）。
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        if event.keystroke.key != "enter" {
                            return;
                        }
                        let record = {
                            let table = this.table.read(cx);
                            table
                                .selected_row()
                                .and_then(|row| table.delegate().record_at(row).cloned())
                        };
                        if let Some(record) = record {
                            this.open_detail(record, window, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(DataTable::new(&self.table)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Record, RecordFilter, RecordsTable, StatusClass, body_lines, utc_datetime, utc_hms,
    };
    use serde_json::json;

    fn record(id: u64, method: &str, authority: &str, status: u16) -> Record {
        Record::parse(&json!({
            "request_id": id,
            "time": 0,
            "duration_ms": 12,
            "request": {"method": method, "path": "/x", "authority": authority},
            "response": {"status": status, "headers": [], "body": {"size": 0}}
        }))
        .expect("测试构造的记录必须能解析")
    }

    #[test]
    fn filters_combine_over_the_visible_projection() {
        let mut table = RecordsTable::new();
        table.append([
            record(1, "GET", "a.test", 200),
            record(2, "POST", "a.test", 500),
            record(3, "GET", "b.test", 404),
        ]);
        assert_eq!(table.visible_count(), 3);
        assert_eq!(table.total(), 3);

        let mut filter = RecordFilter {
            method: Some("GET".into()),
            ..RecordFilter::default()
        };
        assert!(table.set_filter(filter.clone()));
        assert_eq!(table.visible_count(), 2);
        // 行下标指可见行：第 0 行是 #1，不是被筛掉的 #2。
        assert_eq!(table.record_at(0).map(|r| r.request_id), Some(1));
        assert_eq!(table.record_at(1).map(|r| r.request_id), Some(3));

        // 叠加状态档：GET ∩ 4xx → 只剩 #3。
        filter.status = Some(StatusClass::Class4xx);
        assert!(table.set_filter(filter));
        assert_eq!(table.visible_count(), 1);
        assert_eq!(table.record_at(0).map(|r| r.request_id), Some(3));

        // 再叠加主机：条件互斥 → 空结果，但不删任何记录。
        let mut filter = table.filter().clone();
        filter.authority = Some("a.test".into());
        assert!(table.set_filter(filter));
        assert_eq!(table.visible_count(), 0);
        assert_eq!(table.total(), 3);
        assert!(table.record_at(0).is_none());

        // 清空缓冲连带清掉筛选条件（选项已经指向空结果了）。
        table.clear();
        assert!(table.filter().is_empty());
        assert_eq!(table.total(), 0);
    }

    #[test]
    fn status_classes_partition_every_status_including_pending() {
        assert_eq!(StatusClass::of(0), StatusClass::Pending);
        assert_eq!(StatusClass::of(201), StatusClass::Class2xx);
        assert_eq!(StatusClass::of(304), StatusClass::Class3xx);
        assert_eq!(StatusClass::of(404), StatusClass::Class4xx);
        assert_eq!(StatusClass::of(502), StatusClass::Class5xx);
        // 1xx 与 600+ 落在「其他」，不被悄悄并入任何一档。
        assert_eq!(StatusClass::of(101), StatusClass::Other);
        assert_eq!(StatusClass::of(700), StatusClass::Other);
    }

    /// F7 回归：会话失效（没有 target）时，本地记录仍要能被清掉。
    ///
    /// 这不是纯 UI 便利：会话消失后服务端那份没得清，界面这份是唯一副本；
    /// 清不掉 + 按钮被禁用 = 记录被困在表里。
    #[test]
    fn records_can_be_cleared_without_a_live_session() {
        let mut table = RecordsTable::new();
        table.append([
            record(1, "GET", "a.test", 200),
            record(2, "POST", "b.test", 500),
        ]);
        // 先叠一个筛选，确认清空会一并把筛选条件收掉（选项已指向空结果）。
        assert!(table.set_filter(RecordFilter {
            method: Some("GET".into()),
            ..RecordFilter::default()
        }));
        assert_eq!(table.visible_count(), 1);

        assert!(table.clear(), "有记录时清空应报告变化");
        assert_eq!(table.total(), 0);
        assert_eq!(table.visible_count(), 0);
        assert!(table.filter().is_empty(), "记录没了，筛选条件也该归零");
        assert!(!table.clear(), "已经空了，再清不该报告变化");
    }

    #[test]
    fn dates_come_from_the_civil_calendar_not_a_fixed_offset() {
        assert_eq!(utc_datetime(0), "1970-01-01 00:00:00.000");
        // 跨日：1970-01-02（上面 UTC 时间格式的用例只覆盖时分秒）。
        assert_eq!(utc_datetime(86_400_000), "1970-01-02 00:00:00.000");
        assert_eq!(utc_datetime(1_700_000_000_123), "2023-11-14 22:13:20.123");
        // 闰日：2024-02-29 是真实存在的一天，不能被算成 03-01。
        assert_eq!(utc_datetime(1_709_164_800_000), "2024-02-29 00:00:00.000");
        // 世纪闰：2000-02-29（1900 不是闰年，这条能抓住"百年不闰"的漏算）。
        assert_eq!(utc_datetime(951_782_400_000), "2000-02-29 00:00:00.000");
    }

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
            "duration_ms": 87,
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
        assert_eq!(record.duration_ms, 87);
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

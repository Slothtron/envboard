//! 环境列表屏：`DataTable` + 快照轮询。
//!
//! 表格**整体交给现成组件**：`TableState<EnvTable>` 自带虚拟滚动、可排序表头、
//! 可拖拽列宽、空态与骨架加载态（`loading()` / `render_empty()` 的默认实现就是
//! 框架内置的那套），所以本文件里没有一行手搓的表格度量。
//!
//! 数据通路只有一条：`main` 装的 [`Host`] Global → 1s 轮询器（跑在 tokio 上）→
//! `mpsc(1)` → 主线程写进 delegate → `TableState::refresh`。
//! 快照是**全量**的，`generation` 只作展示参考，界面不许拿它当"跳过渲染"的依据
//! （判据见 spec/admin-api.md 的游标语义）。

use std::time::Duration;

use gpui_kit::component::ActiveTheme;
use gpui_kit::component::badge::Badge;
use gpui_kit::component::table::{Column, ColumnSort, DataTable, TableDelegate, TableState};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Hsla, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Task,
    TestSupportExt as _, Window, div, px,
};

use envboard_engine::Error;
use envboard_protocol::EnvView;

use crate::display::{Tone, health_label, health_tone, listen_address, needs_attention};
use crate::host::Host;

/// 一帧快照：`(账本代次, 全量环境视图)` —— 与 `AdminService::snapshot()` 同形。
pub type Snapshot = Result<(u64, Vec<EnvView>), Error>;

/// 列定义（顺序即渲染顺序；`None` 宽度的列吃掉剩余空间）。
const COLUMNS: [&str; 6] = [
    "name",
    "health",
    "listen",
    "rules",
    "upstream",
    "description",
];

fn columns() -> Vec<Column> {
    // 固定列合计 + 说明列下限必须容得下最小窗口的内容区
    // （1192 逻辑宽 - 侧栏 ≈ 930），否则最后一列被裁出视口。
    [
        ("name", "环境", Some(180.)),
        ("health", "健康", Some(110.)),
        ("listen", "监听", Some(150.)),
        ("rules", "规则", Some(140.)),
        ("upstream", "上游", Some(130.)),
        ("description", "说明", None),
    ]
    .into_iter()
    .map(|(key, name, width)| {
        let column = Column::new(key, name).sortable().resizable(true);
        match width {
            Some(w) => column.width(px(w)),
            None => column.min_width(px(160.)),
        }
    })
    .collect()
}

/// 环境表的 delegate：持有当前快照 + 加载标记 + 取数错误。
pub struct EnvTable {
    columns: Vec<Column>,
    rows: Vec<EnvView>,
    generation: u64,
    loading: bool,
    error: Option<SharedString>,
}

impl EnvTable {
    /// 首帧：还没有任何数据，让框架画它的骨架。
    pub fn loading() -> Self {
        Self {
            columns: columns(),
            rows: Vec::new(),
            generation: 0,
            loading: true,
            error: None,
        }
    }

    /// 写入一帧快照；返回**是否真的变了**（没变就不必通知重绘）。
    pub fn apply(&mut self, snapshot: Snapshot) -> bool {
        match snapshot {
            Ok((generation, rows)) => {
                let changed = self.rows != rows;
                self.generation = generation;
                self.rows = rows;
                self.loading = false;
                self.error = None;
                changed
            }
            // 取数失败**不清空已有行**：界面继续显示上一次的事实，
            // 错误另说一句（错误码与消息是契约的一部分，见 spec/errors.md）。
            Err(error) => {
                let changed = self.error.as_deref() != Some(error.message.as_str());
                self.error = Some(error.message.into());
                self.loading = false;
                changed
            }
        }
    }

    /// 最近一次取数的失败原因（视图顶部说明用）。
    pub fn error(&self) -> Option<SharedString> {
        self.error.clone()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn rows(&self) -> &[EnvView] {
        &self.rows
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// `sort_key`：排序要拿行本身比，不能借 `&self`（`sort_by` 里 `self.rows` 已被可变借用）。
fn sort_key_of(env: &EnvView, col_ix: usize) -> String {
    match COLUMNS.get(col_ix).copied().unwrap_or("name") {
        "name" => env.name.clone(),
        "health" => health_label(&env.health),
        "listen" => listen_address(env),
        "rules" => env.rules.clone().unwrap_or_else(|| "zzz".into()),
        "upstream" => env.upstream.clone().unwrap_or_else(|| "zzz".into()),
        _ => env.description.clone(),
    }
}

impl TableDelegate for EnvTable {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
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

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        let ascending = match sort {
            ColumnSort::Default => return,
            ColumnSort::Ascending => true,
            ColumnSort::Descending => false,
        };
        // 稳定排序：同键的行保持服务端顺序（账本序），不靠运气。
        self.rows.sort_by(|left, right| {
            let left_key = sort_key_of(left, col_ix);
            let right_key = sort_key_of(right, col_ix);
            if ascending {
                left_key.cmp(&right_key)
            } else {
                right_key.cmp(&left_key)
            }
        });
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(env) = self.rows.get(row_ix).cloned() else {
            return div().into_any_element();
        };
        // 主题令牌经 App 解引用取（不许出现字面色值）。
        let muted = cx.theme().muted_foreground;
        let warning = cx.theme().warning;

        match COLUMNS.get(col_ix).copied().unwrap_or("name") {
            "name" => cell(
                div()
                    .truncate()
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .child(env.name.clone()),
            ),
            "health" => cell(
                Badge::new()
                    .color(tone_color(health_tone(&env.health), cx))
                    .child(health_label(&env.health)),
            ),
            "listen" => cell(div().truncate().child(listen_address(&env))),
            "rules" => cell(
                div()
                    .truncate()
                    .child(env.rules.clone().unwrap_or_else(|| "未绑定".into())),
            ),
            "upstream" => cell(
                div()
                    .truncate()
                    .child(env.upstream.clone().unwrap_or_else(|| "直连".into())),
            ),
            _ => cell(
                div()
                    .truncate()
                    .text_color(if needs_attention(&env) {
                        warning
                    } else {
                        muted
                    })
                    .child(env.description.clone()),
            ),
        }
    }
}

/// 单元格外壳：吃掉列宽、超长截断（`min_w_0` 缺了会把整表撑出视口）。
fn cell(child: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .child(child)
        .into_any_element()
}

/// 色调 → 主题令牌（**不许出现字面色值**，一切经 `cx.theme()`）。
fn tone_color(tone: Tone, cx: &App) -> Hsla {
    let theme = cx.theme();
    match tone {
        Tone::Success => theme.success,
        Tone::Warning => theme.warning,
        Tone::Danger => theme.danger,
        Tone::Muted => theme.muted_foreground,
    }
}

/// 环境屏的视图实体：持有表与轮询任务。
pub struct Environments {
    table: Entity<TableState<EnvTable>>,
    poll: Task<()>,
}

impl Environments {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let table = cx.new(|cx| TableState::new(EnvTable::loading(), window, cx));
        Self {
            table,
            poll: Task::ready(()),
        }
    }

    /// 起轮询器（首帧后由根视图调一次；构造期不启动后台任务）。
    pub fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let host = cx.global::<Host>();
        let admin = host.admin().clone();
        let handle = host.handle().clone();
        // 容量 1 + try_send 丢帧：快照是全量，积压没有意义（同一语义见
        // web 形态的 /api/events：每个客户端各跑一遍全量快照）。
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<Snapshot>(1);
        handle.spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tick.tick().await;
                let frame = admin.snapshot();
                if sender.try_send(frame).is_err() {
                    continue;
                }
            }
        });

        let table = self.table.clone();
        self.poll = cx.spawn_in(window, async move |this, cx| {
            while let Some(frame) = receiver.recv().await {
                let _ = table.update_in(cx, |state, _, cx| {
                    if state.delegate_mut().apply(frame) {
                        state.refresh(cx);
                    }
                });
                // 行数与错误条都挂在视图这一层，表刷新不等于视图刷新。
                let _ = this.update_in(cx, |_, _, cx| cx.notify());
            }
        });
    }
}

impl Render for Environments {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.table.read(cx).delegate().rows().len();
        let count = SharedString::from(format!("{rows} 个环境"));
        let error = self
            .table
            .read(cx)
            .delegate()
            .error()
            .map(|message| SharedString::from(format!("取数失败：{message}")));

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap_2()
            .child(
                div()
                    .id("env-count")
                    .test_support()
                    .aria_label(count.clone())
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(count),
            )
            .children(error.clone().map(|message| {
                div()
                    .id("env-error")
                    .test_support()
                    .aria_label(message.clone())
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(message)
            }))
            // DataTable 自己画满容器（内部 size_full），所以容器要给足高度。
            .child(div().flex_1().min_h_0().child(DataTable::new(&self.table)))
    }
}

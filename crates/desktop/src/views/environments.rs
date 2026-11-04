//! 环境列表屏：搜索 + 状态过滤 + 可排序表 + 行内启停/重启/删除 + 新建。
//!
//! 表格整体交给现成组件：`TableState<EnvTable>` + `DataTable` 自带虚拟滚动、
//! 可排序表头、可拖拽列宽、行选中、空态与骨架加载态，本文件没有一行手搓表格度量。
//!
//! 数据通路只有一条：`main` 装的 [`Host`] Global → 1s 轮询器（跑在 tokio 上）→
//! `mpsc(1)` → 主线程写进 delegate → `TableState::refresh`。快照是**全量**的，
//! `generation` 只作展示参考，界面不许拿它当"跳过渲染"的依据。
//!
//! 写操作（启停/重启/删除/新建）一律经 [`crate::host::hop`] 回到 tokio，回执后立刻
//! 补一次快照 —— 用户不该等下一拍才看到按钮灰下去。在途的环境名记在 delegate 里
//! （按钮转圈 + 禁用，防连点），所以"哪一行在等回执"也只有一处真相。

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::future::Future;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::ActiveTheme;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{
    Button, ButtonCustomVariant, ButtonGroup, ButtonVariant, ButtonVariants,
};
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::table::{
    Column, ColumnSort, DataTable, TableDelegate, TableEvent, TableState,
};
use gpui_kit::component::{Icon, IconName, Selectable, Sizable, WindowExt};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, FontWeight, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, TestSupportExt as _, WeakEntity,
    Window, div, px,
};

use envboard_admin::AdminService;
use envboard_engine::Error;
use envboard_protocol::{Desired, EnvView};

use crate::display::{Tone, health_label, health_tone, listen_address, needs_attention};
use crate::errors::{Action, Notice};
use crate::host::{Host, hop};
use crate::views::detail::Detail;
use crate::views::{empty_state, empty_state_with_action, error_banner, text_tooltip};

/// 一帧快照：`(账本代次, 全量环境视图)` —— 与 `AdminService::snapshot()` 同形。
///
/// 失败一侧是 [`Notice`] 而不是字符串：错误码、英文原文、中文主句要在跨线程之前
/// 就装好，界面这一侧只管显示（映射规则见 `crate::errors`）。
pub type Snapshot = Result<(u64, Vec<EnvView>), Notice>;

/// 状态过滤（单一入口：分段控件就是唯一筛选面，不与别处叠加）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Running,
    Stopped,
    Attention,
}

impl Filter {
    fn label(self) -> &'static str {
        match self {
            Self::All => "全部",
            Self::Running => "运行中",
            Self::Stopped => "已停止",
            Self::Attention => "需处理",
        }
    }

    /// 四档顺序：工具条按它画、分组回调按索引取档、计数也按它算。
    pub const ALL: [Self; 4] = [Self::All, Self::Running, Self::Stopped, Self::Attention];

    fn matches(self, env: &EnvView) -> bool {
        match self {
            Self::All => true,
            Self::Running => env.health == "running",
            Self::Stopped => env.health == "stopped",
            Self::Attention => needs_attention(env),
        }
    }
}

/// 列下标（顺序即渲染顺序；`None` 宽度的列吃掉剩余空间）。
const NAME: usize = 0;
const HEALTH: usize = 1;
const LISTEN: usize = 2;
const RULES: usize = 3;
const UPSTREAM: usize = 4;
// 操作列排在说明**之前**：表格按从左到右裁切，说明是唯一可牺牲的列。
// 若把操作列放在最后，窄窗口下先被裁掉的恰恰是启停/重启/删除这些唯一入口。
const ACTIONS: usize = 5;
const DESCRIPTION: usize = 6;
const COLUMN_COUNT: usize = 7;

/// 列宽预算，按两个宽度同时成立来排：
///
/// * **最小窗口**（1000）：内容区 = 1000 − 侧栏 248 − 内边距 32 = 720。
///   前六列合计 596 ≤ 720 —— 操作列（启停/重启/删除的唯一入口）必须在视口内；
/// * **默认窗口**（1180）：内容区 900，七列合计 896，右侧不留空列。
///
/// 说明列给固定 300 而不是弹性：这套表格没有"拉伸填满"的开关，弹性列只会塌到
/// 下限、把剩余宽度留成一条空白列。代价是窄窗口下说明列被裁掉一截 —— 它是唯一
/// 可以牺牲的列，所以排在最后。
///
/// 这条预算曾经按"默认宽度 1192 − 侧栏 ≈ 930"算过 —— 那个数在最小窗口下不成立，
/// 操作列会被整个裁出视口（P0.3 之后复发的同一类问题）。
fn columns() -> Vec<Column> {
    [
        ("name", "环境", Some(126.), true),
        ("health", "健康", Some(84.), true),
        ("listen", "监听", Some(124.), true),
        ("rules", "规则", Some(108.), true),
        ("upstream", "上游", Some(90.), true),
        // 操作列不参与排序：给它排序箭头是骗人的（P0.4 截图里发现了这个多余的东西）。
        ("actions", "操作", Some(104.), false),
        ("description", "说明", Some(300.), true),
    ]
    .into_iter()
    .map(|(key, name, width, sortable)| {
        let column = Column::new(key, name).resizable(true);
        let column = if sortable { column.sortable() } else { column };
        match width {
            Some(w) => column.width(px(w)),
            None => column.min_width(px(120.)),
        }
    })
    .collect()
}

/// 状态栏聚合要的数（全部从当前快照派生，不额外取数）。
///
/// 与过滤档的计数同源（[`Filter::matches`] / [`needs_attention`]），
/// 所以卡片上的数字和点进过滤档看到的行数永远一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Overview {
    pub total: usize,
    pub running: usize,
    pub stopped: usize,
    pub attention: usize,
    /// 开着抓包的环境数（`capture=true`，与调试会话共用同一字段）。
    pub capture: usize,
    /// 绑了规则集的环境数（含规则集已缺失的：绑定关系仍是事实）。
    pub rules_bound: usize,
    /// 经上游代理的环境数（直连不算）。
    pub upstream: usize,
}

/// 环境表的 delegate：全量快照 + 过滤后的可见行 + 在途写操作。
pub struct EnvTable {
    columns: Vec<Column>,
    rows: Vec<EnvView>,
    /// `rows` 的下标，按当前搜索词与过滤条件筛过（表只渲染可见行）。
    visible: Vec<usize>,
    /// 正在等后端回执的环境名（按钮转圈 + 禁用，防连点）。
    pending: BTreeSet<String>,
    query: String,
    filter: Filter,
    generation: u64,
    loading: bool,
    error: Option<Notice>,
    /// 表头排序的当前状态（列下标 + 方向）。
    ///
    /// 组件把箭头状态存在 `TableState` 里、通过 `perform_sort` 的参数送下来；
    /// 这里必须自己留一份：快照是**服务端账本序的全量帧**，每秒覆盖一次
    /// `rows`，只排当前一帧的话下一拍就退回账本序，而表头箭头还指着刚才那档。
    /// `ColumnSort` 是 `Copy + PartialEq + Default`（gpui-component 0.8.0）。
    sort: Option<(usize, ColumnSort)>,
    /// 这条错误来自**取数**还是**写操作**。
    ///
    /// 两种失败的收场方式不同：取数失败在下一次成功快照时自然消失（数据回来了，
    /// 那句话就不成立了）；写操作失败不能这样收场 —— 随后那次快照成功只说明
    /// "读得到"，并不说明刚才那次写成功了。曾经两种一律清掉，于是「删除失败」
    /// 在错误条上只闪一帧，用户根本读不到原因。
    error_from_load: bool,
    view: WeakEntity<Environments>,
}

impl EnvTable {
    /// 首帧：还没有任何数据，让框架画它的骨架。
    pub fn loading(view: WeakEntity<Environments>) -> Self {
        Self {
            columns: columns(),
            rows: Vec::new(),
            visible: Vec::new(),
            pending: BTreeSet::new(),
            query: String::new(),
            filter: Filter::All,
            generation: 0,
            loading: true,
            sort: None,
            error: None,
            error_from_load: false,
            view,
        }
    }

    /// 写入一帧快照；返回**视图是否需要重绘**（没变就不必通知）。
    ///
    /// 「需要重绘」不等于「行数据变了」：取数失败被撤掉、首帧脱掉 loading、
    /// 在途标记被清理，这些挂在视图层的东西都得靠这一句返回值才能刷新。
    pub fn apply(&mut self, snapshot: Snapshot) -> bool {
        match snapshot {
            Ok((generation, mut rows)) => {
                // 先把新帧按保存的排序键排好，再比较：
                // * `changed` 必须是**顺序无关**的，否则排过序的行与账本序永远不等，
                //   每秒都会误报"数据变了"；
                // * 排序状态由用户点表头时留下，不能让全量快照把它覆盖回去。
                let sort = self.sort;
                sort_rows(&mut rows, sort);
                let rows_changed = self.rows != rows;
                let error_cleared = self.error_from_load;
                let loading_changed = self.loading;
                self.generation = generation;
                self.rows = rows;
                self.loading = false;
                // 取数成功只撤掉"取数失败"那句话，不动写操作的失败（见字段注释）。
                if self.error_from_load {
                    self.error = None;
                    self.error_from_load = false;
                }
                // 已被删除的环境不该继续在途表里挂着（否则名字会被幽灵占住）。
                let pending_before = self.pending.len();
                self.pending
                    .retain(|name| self.rows.iter().any(|env| env.name == *name));
                let pending_changed = self.pending.len() != pending_before;
                self.rebuild_visible();
                rows_changed || error_cleared || loading_changed || pending_changed
            }
            // 取数失败**不清空已有行**：界面继续显示上一次的事实，另说一句错误。
            Err(notice) => {
                let loading_changed = self.loading;
                self.set_error(notice) || loading_changed
            }
        }
    }

    /// 取数失败：下一次成功快照会把这句撤掉。
    fn set_error(&mut self, notice: Notice) -> bool {
        let changed = self.error.as_ref() != Some(&notice);
        self.error = Some(notice);
        self.error_from_load = true;
        self.loading = false;
        changed
    }

    /// 写操作失败：**不随快照撤掉**（一次读成功证明不了刚才那次写成功）。
    /// 下一次写操作开始时由调用方决定是否清场 —— 现在不清，后一次失败会直接替换它。
    pub fn set_write_error(&mut self, notice: Notice) -> bool {
        let changed = self.error.as_ref() != Some(&notice);
        self.error = Some(notice);
        self.error_from_load = false;
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

    pub fn set_filter(&mut self, filter: Filter) -> bool {
        if self.filter == filter {
            return false;
        }
        self.filter = filter;
        self.rebuild_visible();
        true
    }

    /// 在途标记：写操作发出到回执落地之间，那一行的按钮转圈并禁用。
    pub fn set_pending(&mut self, name: &str, pending: bool) {
        if pending {
            self.pending.insert(name.to_string());
        } else {
            self.pending.remove(name);
        }
    }

    fn rebuild_visible(&mut self) {
        let query = self.query.to_lowercase();
        let filter = self.filter;
        self.visible = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, env)| {
                let hit_query = query.is_empty()
                    || env.name.to_lowercase().contains(&query)
                    || env.description.to_lowercase().contains(&query)
                    || env
                        .rules
                        .as_deref()
                        .is_some_and(|r| r.to_lowercase().contains(&query))
                    || env
                        .upstream
                        .as_deref()
                        .is_some_and(|u| u.to_lowercase().contains(&query));
                hit_query && filter.matches(env)
            })
            .map(|(index, _)| index)
            .collect();
    }

    pub fn name_at(&self, row_ix: usize) -> Option<SharedString> {
        self.visible
            .get(row_ix)
            .map(|&index| self.rows[index].name.clone().into())
    }

    pub fn env_at(&self, row_ix: usize) -> Option<&EnvView> {
        self.visible.get(row_ix).map(|&index| &self.rows[index])
    }

    /// 账本里的环境总数（不受过滤与搜索影响）。
    pub fn total(&self) -> usize {
        self.rows.len()
    }

    /// 过滤后没命中，但账本里确实有环境 —— 空态要说"没有匹配"而不是"没有环境"。
    pub fn filtered_out(&self) -> bool {
        !self.rows.is_empty() && self.visible.is_empty()
    }

    /// 每一档的计数，**与过滤用同一个判据**（[`Filter::matches`]）。
    ///
    /// 不能写成"总数 − 运行中"这类推导式：那样失败 / 启动中 / 端口冲突都会被
    /// 算进"已停止"，控件的数字与点出来的行数对不上（实测：chip 写 2、点开 1 行）。
    /// 数字是用户决定"点不点"的依据，它必须等于点下去的结果。
    pub fn tier_counts(&self) -> [usize; Filter::ALL.len()] {
        Filter::ALL.map(|filter| self.rows.iter().filter(|env| filter.matches(env)).count())
    }

    pub fn error(&self) -> Option<Notice> {
        self.error.clone()
    }

    /// 按名字取当前快照里的环境（详情面板读同一份数据，不另存一份）。
    pub fn find_by_name(&self, name: &str) -> Option<EnvView> {
        self.rows.iter().find(|env| env.name == name).cloned()
    }

    /// 哪些环境绑了这份规则集（规则目录的「被引用」列；与 web 形态同一算法：
    /// 引用关系从环境视图派生，服务端不单独给一份）。
    pub fn referenced_by_rules(&self, name: &str) -> Vec<SharedString> {
        self.rows
            .iter()
            .filter(|env| env.rules.as_deref() == Some(name))
            .map(|env| SharedString::from(env.name.clone()))
            .collect()
    }

    /// 某份规则集当前生效的条数（从引用它的环境取；没有环境引用就取不到，
    /// 界面显示「—」而不是猜一个 0）。
    pub fn rules_count_of(&self, name: &str) -> Option<usize> {
        self.rows
            .iter()
            .find(|env| env.rules.as_deref() == Some(name))
            .map(|env| env.rules_count)
    }

    /// 调试目标候选：**全量**环境 + 是否在跑。调试图只用这一份（不另调门面）——
    /// 调试会话只能开在活着的实例上，界面把没在跑的也列出来但标记为不可选，
    /// 让人看得见"为什么它不在候选里"。
    pub fn debug_targets(&self) -> Vec<(SharedString, bool)> {
        self.rows
            .iter()
            .map(|env| {
                (
                    SharedString::from(env.name.clone()),
                    env.health == "running",
                )
            })
            .collect()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 环境聚合（状态栏显示）：与过滤档计数用同一判据（数字必须等于点下去的行数）。
    pub fn overview(&self) -> Overview {
        overview_of(&self.rows)
    }
}

impl TableDelegate for EnvTable {
    fn columns_count(&self, _: &App) -> usize {
        COLUMN_COUNT
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

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        // 先记下这一档，再排当前的 `rows`。此后每一帧全量快照都会由
        // `apply` 按同一个键重排，排序不会因为轮询而丢失（也不会与表头
        // 箭头说的方向相反）。
        self.sort = match sort {
            ColumnSort::Default => None,
            _ => Some((col_ix, sort)),
        };
        let saved = self.sort;
        sort_rows(&mut self.rows, saved);
        self.rebuild_visible();
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(env) = self.env_at(row_ix).cloned() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let warning = cx.theme().warning;

        match col_ix {
            NAME => cell(
                div()
                    // 行高回归断言的锚点：L0 量相邻两行的原点差，必须一致。
                    .debug_selector(move || format!("env-row-{row_ix}"))
                    .truncate()
                    .font_weight(FontWeight::BOLD)
                    .child(env.name.clone()),
            ),
            HEALTH => cell(
                // 健康状态**不能只靠颜色**（色觉障碍与灰度打印都读不出来）：
                // 前面加一个同色的圆点做形状标记，文字仍写中文标签。
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .size_2()
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(tone_color(health_tone(&env.health), cx)),
                    )
                    .child(div().truncate().child(health_label(&env.health))),
            ),
            LISTEN => cell(div().truncate().child(listen_address(&env))),
            RULES => cell(
                div()
                    .truncate()
                    .text_color(if env.rules_missing { warning } else { muted })
                    .child(match (&env.rules, env.rules_missing) {
                        (Some(name), true) => format!("{name}（缺失）"),
                        (Some(name), false) => format!("{name} · {} 条", env.rules_count),
                        (None, _) => "未绑定".to_string(),
                    }),
            ),
            UPSTREAM => cell(div().truncate().child(match &env.upstream {
                Some(name) if env.proxy_auth_enabled => format!("{name}（带鉴权）"),
                Some(name) => name.clone(),
                None => "直连".to_string(),
            })),
            // 「说明」是用户写的备注，不是系统状态：它恒定用次要色。
            // 把"需处理"染在这里等于借一个语义无关的通道 —— 用户会以为这段备注
            // 有问题，而空说明的环境又完全得不到提示。状态归健康列。
            DESCRIPTION => cell(
                div()
                    // 说明列是唯一会被裁切的可变长文本：hover 给出全文，
                    // 否则被截掉的部分再也读不到。
                    .id(SharedString::from(format!("env-desc-{row_ix}")))
                    .tooltip(text_tooltip(env.description.clone()))
                    .truncate()
                    .text_color(muted)
                    .child(env.description.clone()),
            ),
            ACTIONS => self.render_actions(row_ix, &env, cx),
            _ => div().into_any_element(),
        }
    }

    /// 空态：区分"一个环境都没有"与"过滤/搜索后没匹配"。
    ///
    /// 两种空态的下一步不一样：前者该去「新建环境」，后者该改搜索词或换过滤档。
    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        if self.filtered_out() {
            empty_state(
                cx,
                "env-empty-filtered",
                "没有匹配的环境",
                "换个关键词，或把过滤切回「全部」。",
            )
        } else {
            // 空账本是**首次使用**的那一屏：话说到"下一步做什么"还不够，
            // 把那个按钮就地放一份 —— 文字指路（"点右上角")要让用户先跑一趟。
            let view = self.view.clone();
            empty_state_with_action(
                cx,
                "env-empty-ledger",
                "还没有环境",
                "一个环境 = 一个进程内引擎实例 = 一个监听端口。",
                Some(
                    Button::new("empty-create-env")
                        .xsmall()
                        .icon(Icon::new(IconName::Plus))
                        .label("新建环境")
                        .on_click(move |_, window, cx| {
                            let _ = view.update(cx, |view, cx| view.open_create_dialog(window, cx));
                        })
                        .into_any_element(),
                ),
            )
        }
    }
}

impl EnvTable {
    /// 行内操作：启停（按期望状态翻转）、重启、删除。
    ///
    /// 每个按钮自己拿环境名，不依赖"当前选中行" —— 操作列是行的兄弟节点，
    /// 点击不该被当成行选中（坑 #15）。
    fn render_actions(
        &mut self,
        row_ix: usize,
        env: &EnvView,
        cx: &mut Context<TableState<EnvTable>>,
    ) -> AnyElement {
        let pending = self.pending.contains(&env.name);
        let view = self.view.clone();
        let name: SharedString = env.name.clone().into();
        let running = env.desired == Desired::Running;
        let theme = cx.theme();
        let hover = theme.secondary;
        let active = theme.secondary_active;
        let (toggle_icon, toggle_label) = if running {
            (IconName::Pause, "停止")
        } else {
            (IconName::Play, "启动")
        };

        cell(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child({
                    let name = name.clone();
                    let view = view.clone();
                    row_icon_button(
                        ("toggle", row_ix),
                        toggle_icon,
                        toggle_label,
                        hover,
                        active,
                        cx,
                    )
                    .disabled(pending)
                    .loading(pending)
                    // tooltip 与无障碍名**都说清对象**：只看图标分不出是哪一行。
                    .tooltip(format!("{toggle_label} {name}"))
                    .accessibility_label(format!("{toggle_label} {name}"))
                    .on_click(move |_, window, cx| {
                        let name = name.clone();
                        let _ = view.update(cx, |view, cx| {
                            if running {
                                view.stop(name, window, cx);
                            } else {
                                view.start(name, window, cx);
                            }
                        });
                    })
                })
                .child({
                    let name = name.clone();
                    let view = view.clone();
                    row_icon_button(
                        ("restart", row_ix),
                        IconName::RotateCw,
                        "重启",
                        hover,
                        active,
                        cx,
                    )
                    .disabled(pending)
                    .tooltip(format!("重启 {name}"))
                    .accessibility_label(format!("重启 {name}"))
                    .on_click(move |_, window, cx| {
                        let name = name.clone();
                        let _ = view.update(cx, |view, cx| view.restart(name, window, cx));
                    })
                })
                .child({
                    let name = name.clone();
                    let view = view.clone();
                    // 「删除」不摆在行里：它是**不可逆**动作，和重启挤在一起时
                    // 一次误点就是删环境。收进「更多」菜单，多一步确认换一次误触。
                    row_icon_button(
                        ("more", row_ix),
                        IconName::Ellipsis,
                        "更多操作",
                        hover,
                        active,
                        cx,
                    )
                    .disabled(pending)
                    .tooltip("更多操作")
                    .accessibility_label(format!("{name} 的更多操作"))
                    .dropdown_menu(move |mut menu, _, _| {
                        let view = view.clone();
                        let name = name.clone();
                        menu = menu.item(
                            PopupMenuItem::new(SharedString::from(format!("删除 {name}")))
                                .icon(Icon::new(IconName::Delete))
                                .on_click(move |_, window, cx| {
                                    let name = name.clone();
                                    let _ = view.update(cx, |view, cx| {
                                        view.confirm_delete(name, window, cx)
                                    });
                                }),
                        );
                        menu
                    })
                }),
        )
    }
}

/// 行内图标按钮：不用 `.ghost()` —— 组件的 ghost 悬停色与表格行的悬停色在默认
/// 主题里同色，指针移上去看起来毫无反应（坑 #32）。改用 `secondary` 自定义变体。
/// tooltip 与无障碍名都从同一个 `label` 出，避免两处文案漂移。
fn row_icon_button(
    id: impl Into<ElementId>,
    icon: IconName,
    label: &str,
    hover: Hsla,
    active: Hsla,
    cx: &App,
) -> Button {
    Button::new(id)
        .custom(ButtonCustomVariant::new(cx).hover(hover).active(active))
        .xsmall()
        .icon(Icon::new(icon))
        .tooltip(label.to_string())
}

/// 排序键：全部走"给人看的那串文本"，与列显示一致（不另起一套序）。
///
/// 未绑定 / 直连排到最后（`"zzz"` 是个够用的哨兵：规则集名与环境名一样受
/// `^[a-z][a-z0-9_-]*$` 约束，不会有真名字落在它之后）。
fn sort_key_of(env: &EnvView, col_ix: usize) -> String {
    match col_ix {
        LISTEN => listen_address(env),
        HEALTH => health_label(&env.health),
        RULES => env.rules.clone().unwrap_or_else(|| "zzz".into()),
        UPSTREAM => env.upstream.clone().unwrap_or_else(|| "zzz".into()),
        DESCRIPTION => env.description.clone(),
        ACTIONS => "zzz".to_string(),
        _ => env.name.clone(),
    }
}

/// 按保存的排序键重排一帧行。
///
/// 抽成自由函数是为了让"排序能不能扛住全量快照覆盖"这件事能直接单测 ——
/// `EnvTable` 需要一个 `WeakEntity<Environments>`，纯逻辑用例里造不出来。
/// 稳定排序：同键的行保持服务端顺序（账本序），不靠运气。
fn sort_rows(rows: &mut [EnvView], sort: Option<(usize, ColumnSort)>) {
    let Some((col_ix, sort)) = sort else {
        return;
    };
    let ascending = match sort {
        ColumnSort::Default => return,
        ColumnSort::Ascending => true,
        ColumnSort::Descending => false,
    };
    rows.sort_by(|left, right| {
        let left_key = sort_key_of(left, col_ix);
        let right_key = sort_key_of(right, col_ix);
        if ascending {
            left_key.cmp(&right_key)
        } else {
            right_key.cmp(&left_key)
        }
    });
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

/// 环境屏的视图实体：持有表、搜索框、选中行与后台任务。
pub struct Environments {
    table: Entity<TableState<EnvTable>>,
    search: Entity<InputState>,
    filter: Filter,
    /// 选中的环境名（详情面板挂它）。
    selected: Option<SharedString>,
    /// 停靠的详情面板（`Sheet` 里渲染的实体；同一环境复用，换行只改名字）。
    detail: Option<Entity<Detail>>,
    poll: Task<()>,
    /// 在途写操作：Task 必须被持有，就地丢弃等于取消。
    inflight: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Environments {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let view = cx.weak_entity();
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("搜索环境、规则集或上游代理")
                .clean_on_escape()
        });
        let table = cx.new(|cx| {
            TableState::new(EnvTable::loading(view), window, cx)
                .sortable(true)
                .row_selectable(true)
        });

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

        // 行选中：表只给下标，视图记名字 —— 名字才扛得住快照换代与排序变化。
        let table_sub = cx.subscribe_in(&table, window, {
            let table = table.clone();
            move |this, _, event, window, cx| {
                let TableEvent::SelectRow(row_ix) = event else {
                    return;
                };
                // 「选中」与「打开详情」不是一回事：↑/↓ 也会 emit SelectRow。
                // 键盘移动只改高亮（面板不抢焦点），鼠标单击仍然点即开；
                // 键盘打开详情走 Enter（见 `render` 里的 on_key_down）。
                let keyboard = window.last_input_was_keyboard();
                let name = table.read(cx).delegate().name_at(*row_ix);
                if this.selected != name {
                    this.selected = name.clone();
                    cx.notify();
                }
                if let (false, Some(name)) = (keyboard, name) {
                    this.open_detail(name, window, cx);
                }
            }
        });

        Self {
            table,
            search,
            filter: Filter::All,
            selected: None,
            detail: None,
            poll: Task::ready(()),
            inflight: Vec::new(),
            _subscriptions: vec![search_sub, table_sub],
        }
    }

    /// 起轮询器（首帧后由根视图调一次；构造期不启动后台任务）。
    pub fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let host = cx.global::<Host>();
        let admin = host.admin().clone();
        let handle = host.handle().clone();
        // 容量 1 + try_send 丢帧：快照是全量，积压没有意义（同一语义见 web 形态
        // 的 /api/events：每个客户端各跑一遍全量快照）。
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<Snapshot>(1);
        handle.spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tick.tick().await;
                let frame = admin
                    .snapshot()
                    .map_err(|error| Notice::from_error(Action::LoadSnapshot, None, &error));
                if sender.try_send(frame).is_err() {
                    continue;
                }
            }
        });

        let table = self.table.clone();
        self.poll = cx.spawn_in(window, async move |this, cx| {
            while let Some(frame) = receiver.recv().await {
                let changed = table.update_in(cx, |state, _, cx| {
                    let changed = state.delegate_mut().apply(frame);
                    if changed {
                        state.refresh(cx);
                    }
                    changed
                });
                // 计数、chips 与错误条都挂在视图这一层，表刷新不等于视图刷新。
                // 但**只在数据真的变了时**才通知：否则每秒一帧会把 tier_counts()
                // 的四次全量遍历、rebuild_visible() 与详情面板的 notify 全拖进来，
                // 哪怕快照和上一帧一模一样。
                if changed.unwrap_or(false) {
                    let _ = this.update_in(cx, |this, _, cx| {
                        this.touch_detail(cx);
                        cx.notify();
                    });
                }
            }
        });
    }

    /// 当前环境表实体：规则目录要从**同一份**快照派生"被哪些环境引用"，不另拉一次。
    pub fn table(&self) -> &Entity<TableState<EnvTable>> {
        &self.table
    }

    /// 当前过滤档（只读）。给无头 UI 测试断言用 —— 视图层的状态本来就该可观察。
    pub fn filter(&self) -> Filter {
        self.filter
    }

    /// 当前选中的环境名（只读）。键盘 ↑/↓ 只改这个，不打开面板。
    pub fn selected(&self) -> Option<SharedString> {
        self.selected.clone()
    }

    /// 详情面板当前指向的环境名（只读）。给无头 UI 测试断言「换环境后面板
    /// 不再指向旧环境」用；没有面板时为 `None`。
    pub fn detail_name(&self, cx: &App) -> Option<String> {
        self.detail
            .as_ref()
            .map(|detail| detail.read(cx).name().to_string())
    }

    /// 把焦点交给表格。给无头测试用：Tab 顺序在无头上下文里不会自己发生。
    pub fn focus_table(&self, window: &mut Window, cx: &mut App) {
        use gpui_kit::Focusable as _;
        self.table.read(cx).focus_handle(cx).focus(window, cx);
    }

    /// 立刻补一次快照：写操作回执后不让用户等下一拍（详情面板保存后也走这里）。
    pub(crate) fn refresh_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let admin = cx.global::<Host>().admin().clone();
        let table = self.table.clone();
        let task = hop(
            window,
            cx,
            async move {
                admin
                    .snapshot()
                    .map_err(|error| Notice::from_error(Action::LoadSnapshot, None, &error))
            },
            move |this, frame, _, cx| {
                table.update(cx, |state, cx| {
                    if state.delegate_mut().apply(frame) {
                        state.refresh(cx);
                    }
                });
                this.touch_detail(cx);
                cx.notify();
            },
        );
        self.inflight.push(task);
    }

    pub fn start(&mut self, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.dispatch(
            name,
            Action::StartEnvironment,
            window,
            cx,
            |admin, name| async move { admin.start(name.as_str()).await },
        );
    }

    pub fn stop(&mut self, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.dispatch(
            name,
            Action::StopEnvironment,
            window,
            cx,
            |admin, name| async move { admin.stop(name.as_str()).await },
        );
    }

    pub fn restart(&mut self, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.dispatch(
            name,
            Action::RestartEnvironment,
            window,
            cx,
            |admin, name| async move { admin.restart(name.as_str()).await },
        );
    }

    /// 启停/重启共用一条路：标在途 → tokio 跑用例 → 回执后清标记并补一次快照。
    fn dispatch<Run, Fut>(
        &mut self,
        name: SharedString,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
        run: Run,
    ) where
        Run: FnOnce(Arc<AdminService>, SharedString) -> Fut + Send + 'static,
        Fut: Future<Output = Result<EnvView, Error>> + Send + 'static,
    {
        let admin = cx.global::<Host>().admin().clone();
        let table = self.table.clone();
        self.set_pending(&name, true, cx);
        let task = hop(
            window,
            cx,
            async move {
                let outcome = run(admin, name.clone()).await;
                (name, outcome.map(|env| env.name))
            },
            move |this, (name, outcome), window, cx| {
                table.update(cx, |state, cx| {
                    state.delegate_mut().set_pending(&name, false);
                    state.refresh(cx);
                });
                if let Err(error) = outcome {
                    this.report(Notice::from_error(action, Some(&name), &error), cx);
                }
                this.refresh_now(window, cx);
            },
        );
        self.inflight.push(task);
    }

    /// 删除：不可逆操作必须走确认弹窗（与 web 形态同一条纪律）。
    pub fn confirm_delete(
        &mut self,
        name: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.weak_entity();
        let title = format!("删除环境 {name}？");
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let view = view.clone();
            let name = name.clone();
            dialog
                .confirm()
                .title(title.clone())
                .description("实例会先停止，账本与运行时状态一并删除。此操作不可撤销。")
                .ok_text("删除")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let name = name.clone();
                    view.update(cx, |view, cx| view.remove(name, window, cx))
                        .is_ok()
                })
        });
    }

    pub fn remove(&mut self, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let admin = cx.global::<Host>().admin().clone();
        let table = self.table.clone();
        self.set_pending(&name, true, cx);
        let task = hop(
            window,
            cx,
            async move {
                let outcome = admin.remove(name.as_str());
                (name, outcome)
            },
            move |this, (name, outcome), window, cx| {
                table.update(cx, |state, cx| {
                    state.delegate_mut().set_pending(&name, false);
                    state.refresh(cx);
                });
                if let Err(error) = outcome {
                    this.report(
                        Notice::from_error(Action::DeleteEnvironment, Some(&name), &error),
                        cx,
                    );
                }
                // 删掉的正是被选中的那个 → 选中态收起，并把指向它的详情面板
                // 一并关掉。只清 `selected` 是不够的：面板实体还活着，Sheet
                // 标题仍是旧环境名，正文只会显示"环境 X 已不存在"的空壳，
                // 而且 `touch_detail` 还每帧 notify 它。
                if this.selected.as_deref() == Some(name.as_ref()) {
                    this.selected = None;
                }
                if this
                    .detail
                    .as_ref()
                    .is_some_and(|detail| detail.read(cx).name() == name.as_ref())
                {
                    this.detail = None;
                    window.close_sheet(cx);
                }
                this.refresh_now(window, cx);
            },
        );
        self.inflight.push(task);
    }

    /// 新建环境：形状规则与服务端一致，但**裁决仍在服务端**，这里只是不让人白填。
    pub fn open_create_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.weak_entity();
        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("beta")
                .clean_on_escape()
        });
        // 校验失败的原因写在**弹窗里**：弹窗是模态，表格上方的错误条被它挡住，
        // 写到那儿等于没人看见（这条是走查抓出来的真缺陷）。
        let problem: Rc<RefCell<Option<SharedString>>> = Rc::new(RefCell::new(None));
        window.open_dialog(cx, move |dialog, _, cx| {
            let view = view.clone();
            let name_input = name_input.clone();
            let problem = problem.clone();
            let hint_color = cx.theme().muted_foreground;
            // 先把可能存在的校验原因取出来（借用在表达式内结束，不与闭包抢 cx）。
            let invalid = problem.borrow().clone();
            dialog
                .title("新建环境")
                .child(
                    div()
                        .text_sm()
                        .text_color(hint_color)
                        .child("一个环境 = 一个进程内引擎实例 = 一个监听端口。"),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_sm().child("名称"))
                        .child(Input::new(&name_input).w_full())
                        .child(
                            div()
                                .id("create-hint")
                                .test_support()
                                .text_xs()
                                .text_color(hint_color)
                                .child("小写字母开头，可含数字、下划线与短横线；端口随机分配。"),
                        ),
                )
                // 校验失败时的原因行（只在有问题时出现）。
                .children(invalid.map(|message| {
                    div()
                        .id("create-error")
                        .test_support()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(message)
                        .into_any_element()
                }))
                // 普通 Dialog 不画按钮：页脚不给，弹窗就只有「关闭」，谁都提交不了。
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().trigger(|button| button.label("取消")))
                        .child(
                            DialogAction::new()
                                .child(Button::new("create-env-ok").primary().label("新建")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let raw = name_input.read(cx).value().to_string();
                    if !valid_env_name(&raw) {
                        // 校验不过就让弹窗开着，并把原因写进弹窗里的错误行（不静默关）。
                        *problem.borrow_mut() =
                            Some("环境名不合法：小写字母开头，只允许 a-z 0-9 _ -".into());
                        // 弹窗层要重画一帧才会带上这行错误。
                        window.refresh();
                        return false;
                    }
                    *problem.borrow_mut() = None;
                    let _ = view.update(cx, |view, cx| view.create(raw, window, cx));
                    true
                })
        });
    }

    fn create(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let admin = cx.global::<Host>().admin().clone();
        // 名字要同时进请求体与失败主句：两个闭包各持一份（async 块按值拿走 `name`，
        // 回调再引用它就是借用已移动的值）。
        let subject = name.clone();
        let task = hop(
            window,
            cx,
            async move { admin.create(&serde_json::json!({ "name": name })) },
            move |this, outcome, window, cx| {
                if let Err(error) = outcome {
                    this.report(
                        Notice::from_error(Action::CreateEnvironment, Some(&subject), &error),
                        cx,
                    );
                }
                this.refresh_now(window, cx);
            },
        );
        self.inflight.push(task);
    }

    /// 写操作的失败说明：显示在表格上方的错误条里。
    fn report(&mut self, notice: Notice, cx: &mut Context<Self>) {
        let table = self.table.clone();
        table.update(cx, |state, cx| {
            if state.delegate_mut().set_write_error(notice) {
                state.refresh(cx);
            }
        });
        cx.notify();
    }

    fn set_pending(&mut self, name: &str, pending: bool, cx: &mut Context<Self>) {
        let name = name.to_string();
        let table = self.table.clone();
        table.update(cx, |state, cx| {
            state.delegate_mut().set_pending(&name, pending);
            state.refresh(cx);
        });
        cx.notify();
    }

    /// 打开（或换到）某个环境的详情面板。
    ///
    /// 面板实体只建一次：`overlay(false)` 让主列表照常可点，换行时改名字而不是
    /// 重开一个面板 —— 与 web 形态"单击切换不关面板"的行为一致。
    pub fn open_detail(&mut self, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let table = self.table.clone();
        let detail = match &self.detail {
            Some(detail) => {
                detail.update(cx, |detail, cx| detail.rename(name.clone(), window, cx));
                detail.clone()
            }
            None => {
                let view = cx.weak_entity();
                let created = cx.new(|cx| Detail::new(name.clone(), table, view, window, cx));
                self.detail = Some(created.clone());
                created
            }
        };
        window.open_sheet(cx, move |sheet, _, _| {
            sheet
                .title(format!("环境 {name}"))
                .size(px(520.))
                .overlay(false)
                .resizable(true)
                .child(detail.clone())
        });
    }

    /// 快照到了：让面板重绘（它读的是同一份表数据，只是需要一次 notify）。
    fn touch_detail(&self, cx: &mut Context<Self>) {
        if let Some(detail) = &self.detail {
            detail.update(cx, |_, cx| cx.notify());
        }
    }

    /// 切过滤档（工具条的芯片与状态栏的聚合都走这一条路：**一个入口，一处真相**）。
    pub(crate) fn apply_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.filter = filter;
        let table = self.table.clone();
        table.update(cx, |state, cx| {
            if state.delegate_mut().set_filter(filter) {
                state.refresh(cx);
            }
        });
        cx.notify();
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let counts = self.table.read(cx).delegate().tier_counts();
        let view = cx.weak_entity();

        // 四档过滤是**分段控件**：一个轮廓、一条分隔，选中档靠自己的填充说话
        // （不再用四枚彼此独立的 ghost 钮 —— 它们看上去像四个可随手点的标签，
        // 而不是"从四档里选一档"）。
        let filters = ButtonGroup::new("env-filters")
            .outline()
            .xsmall()
            .children(Filter::ALL.into_iter().zip(counts).map(|(filter, count)| {
                let label = format!("{} ({})", filter.label(), count);
                let active = self.filter == filter;
                Button::new(SharedString::from(format!("filter-{}", filter.label())))
                    .label(label.clone())
                    .accessibility_label(label)
                    .selected(active)
                    // 组件给选中档的填充很浅（暗色下几乎看不出）：再加一处**不靠颜色**
                    // 的编码。字重是唯一不会被组件覆盖的通道（bg/fg 由 selected 样式定）。
                    .font_weight(if active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .tooltip("按状态筛选")
            }))
            .on_click({
                let view = view.clone();
                move |clicks, _, cx| {
                    // 单选：分组回调给的是"点中的那一档"（索引）。
                    let Some(ix) = clicks.first().copied() else {
                        return;
                    };
                    let Some(filter) = Filter::ALL.get(ix).copied() else {
                        return;
                    };
                    let _ = view.update(cx, |view, cx| view.apply_filter(filter, cx));
                }
            });
        div()
            .id("env-toolbar")
            .flex()
            .items_center()
            .gap_2()
            .child(div().w_64().child(Input::new(&self.search)))
            .child(
                // 给无头测试留的定位锚点：分段控件是组件库画的，不注册 selector，
                // 只能靠外层这个 div 拿到它的矩形，再按偏移点某一档。
                div()
                    .debug_selector(|| "env-filters".to_string())
                    .child(filters),
            )
            // 每屏只一个主操作，放在最右（ml-auto 的等价写法：前面吃掉剩余空间）。
            .child(div().flex_1())
            .child(
                Button::new("create-env")
                    .xsmall()
                    .icon(Icon::new(IconName::Plus))
                    .label("新建环境")
                    .tooltip("新建环境")
                    .on_click(move |_, window, cx| {
                        let _ = view.update(cx, |view, cx| view.open_create_dialog(window, cx));
                    }),
            )
            .into_any_element()
    }
}

/// 概览聚合（纯函数：与过滤档共用同一组判据，便于单测）。
fn overview_of(rows: &[EnvView]) -> Overview {
    let mut overview = Overview {
        total: rows.len(),
        ..Overview::default()
    };
    for env in rows {
        if env.health == "running" {
            overview.running += 1;
        }
        if env.health == "stopped" {
            overview.stopped += 1;
        }
        if needs_attention(env) {
            overview.attention += 1;
        }
        if env.capture {
            overview.capture += 1;
        }
        if env.rules.is_some() {
            overview.rules_bound += 1;
        }
        if env.upstream.is_some() {
            overview.upstream += 1;
        }
    }
    overview
}

/// 环境名的形状规则（与服务端 `domain::validate_name` 同一判据；服务端仍是裁决方）。
fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

impl Render for Environments {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let delegate = self.table.read(cx).delegate();
        let error = delegate.error();
        let filtered_out = delegate.filtered_out();
        let rows = delegate.rows_count(cx);
        let total = delegate.total();
        // 这行只说**过滤后多出来的事实**：可见条数与选中项。总数已经在工具条
        // 的四档计数里（「全部 (3)」），再说一遍"3 个环境"是同一件事说两遍。
        // 没有要说的就留空行 —— 行本身保留，选中时表格不会跳一下。
        let mut status = String::new();
        if rows != total {
            status.push_str(&format!("显示 {rows} / {total} 个环境"));
        }
        if filtered_out {
            if !status.is_empty() {
                status.push_str(" · ");
            }
            status.push_str("无匹配项");
        }
        if let Some(name) = &self.selected {
            if !status.is_empty() {
                status.push_str(" · ");
            }
            status.push_str(&format!("选中 {name}"));
        }
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
            .child(
                div()
                    .id("env-status")
                    .test_support()
                    .aria_label(status.clone())
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(status),
            )
            .children(error.map(|message| error_banner("env-error", message)))
            // DataTable 自己画满容器（内部 size_full），所以容器要给足高度。
            .child(
                div()
                    .id("env-table")
                    .flex_1()
                    .min_h_0()
                    // 键盘打开详情：↑/↓ 只移动高亮（见订阅里的 `keyboard` 判断），
                    // Enter 才真正打开当前行。
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        if event.keystroke.key != "enter" {
                            return;
                        }
                        let name = {
                            let table = this.table.read(cx);
                            table
                                .selected_row()
                                .and_then(|row| table.delegate().name_at(row))
                        };
                        if let Some(name) = name {
                            this.open_detail(name, window, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(DataTable::new(&self.table)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(name: &str, health: &str, desired: Desired) -> EnvView {
        EnvView {
            name: name.into(),
            listen: envboard_protocol::Endpoint {
                host: "127.0.0.1".into(),
                port: 16_600,
            },
            rules: None,
            upstream: None,
            insecure_hosts: Vec::new(),
            mitm_hosts: Vec::new(),
            description: String::new(),
            desired,
            health: health.into(),
            health_reason: None,
            rules_count: 0,
            rules_missing: false,
            proxy_command: String::new(),
            proxy_auth_enabled: false,
            capture: false,
        }
    }

    #[test]
    fn env_name_shape_matches_the_server_rule() {
        assert!(valid_env_name("beta"));
        assert!(valid_env_name("kad_offline-2"));
        assert!(!valid_env_name(""));
        assert!(!valid_env_name("Beta"));
        assert!(!valid_env_name("1beta"));
        assert!(!valid_env_name("-beta"));
        assert!(!valid_env_name("be ta"));
        assert!(!valid_env_name("beta:8080"));
    }

    #[test]
    fn overview_aggregates_use_the_same_predicates_as_the_filters() {
        let mut rows = [
            env("a", "running", Desired::Running),
            env("b", "stopped", Desired::Stopped),
            env("c", "failed", Desired::Running),
            env("d", "running", Desired::Running),
        ];
        rows[0].capture = true;
        rows[1].rules = Some("corp".into());
        rows[2].upstream = Some("office".into());
        rows[3].rules = Some("corp".into());
        rows[3].capture = true;

        let overview = overview_of(&rows);
        assert_eq!(overview.total, 4);
        assert_eq!(overview.running, 2);
        assert_eq!(overview.stopped, 1);
        // 只有 c（failed 且期望运行）需处理；b 期望停止、实际停止，不算。
        assert_eq!(overview.attention, 1);
        assert_eq!(overview.capture, 2);
        assert_eq!(overview.rules_bound, 2);
        assert_eq!(overview.upstream, 1);
        // 卡片数字必须等于点进对应过滤档看到的行数。
        assert_eq!(
            rows.iter()
                .filter(|env| Filter::Running.matches(env))
                .count(),
            overview.running
        );
        assert_eq!(
            rows.iter()
                .filter(|env| Filter::Attention.matches(env))
                .count(),
            overview.attention
        );
    }

    #[test]
    fn filters_partition_by_health_and_attention() {
        let rows = [
            env("a", "running", Desired::Running),
            env("b", "stopped", Desired::Stopped),
            env("c", "starting", Desired::Running),
            env("d", "failed", Desired::Running),
        ];
        let count = |filter: Filter| rows.iter().filter(|e| filter.matches(e)).count();
        assert_eq!(count(Filter::All), 4);
        assert_eq!(count(Filter::Running), 1);
        assert_eq!(count(Filter::Stopped), 1);
        // starting（还没起来）与 failed 都算需处理：与 web 形态同一判据。
        assert_eq!(count(Filter::Attention), 2);
    }

    #[test]
    fn sort_keys_put_unbound_last_and_use_displayed_text() {
        let mut bound = env("a", "running", Desired::Running);
        bound.rules = Some("corp".into());
        assert!(
            sort_key_of(&env("z", "running", Desired::Running), RULES) > sort_key_of(&bound, RULES),
            "未绑定的要排在最后"
        );
        // 健康按中文标签排，不按英文字面量排（用户看到的是中文）。
        assert_eq!(
            sort_key_of(&env("a", "stopped", Desired::Stopped), HEALTH),
            "已停止"
        );
        assert_eq!(
            sort_key_of(&env("a", "running", Desired::Running), LISTEN),
            "127.0.0.1:16600"
        );
    }

    /// F1 回归：排序状态必须能扛住服务端每秒一次的全量账本序帧。
    /// 只有把新帧按保存的键重排之后再比较，`changed` 才不会因为"排过序
    /// 就与账本序不等"而每帧误报变化（F15 依赖这个返回值收敛重绘）。
    #[test]
    fn a_saved_sort_survives_a_full_ledger_ordered_snapshot() {
        // 账本序：c, a, b（不是按名字排的）。
        let ledger = || {
            vec![
                env("c", "running", Desired::Running),
                env("a", "running", Desired::Running),
                env("b", "running", Desired::Running),
            ]
        };
        let sort = Some((NAME, ColumnSort::Ascending));

        // 用户点表头：当前帧按名字排好。
        let mut shown = ledger();
        sort_rows(&mut shown, sort);
        assert_eq!(
            shown.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["a", "b", "c"]
        );

        // 下一帧快照仍是账本序：先按同一个键重排，再与当前行比较 ——
        // 应该判定"没变"，且行序仍是用户选的那档。
        let mut next_frame = ledger();
        sort_rows(&mut next_frame, sort);
        assert_eq!(shown, next_frame, "排序过的帧不该与重排后的新帧比出差异");
        assert_eq!(
            next_frame
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "c"],
            "新帧不能悄悄退回账本序"
        );

        // 数据真变了（新增一行）仍要报变化；排序方向也照样生效。
        let mut changed_frame = next_frame.clone();
        changed_frame.push(env("d", "stopped", Desired::Stopped));
        sort_rows(&mut changed_frame, sort);
        assert_ne!(shown, changed_frame);

        let mut descending = ledger();
        sort_rows(&mut descending, Some((NAME, ColumnSort::Descending)));
        assert_eq!(
            descending
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            ["c", "b", "a"]
        );
        // 清掉排序（回到 Default）不该自己改行序。
        let mut untouched = ledger();
        sort_rows(&mut untouched, None);
        assert_eq!(untouched, ledger());
    }
}

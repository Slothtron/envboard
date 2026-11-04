//! envboard-desktop —— envboard 的**桌面形态**：GPUI 原生工作台，也是现在唯一的
//! 界面形态与唯一发布物（二进制名 `envboard`）。它消费词汇（`envboard-protocol` 的
//! 视图 DTO）与门面（`envboard-admin`），与已经出库的 web 形态用的是同一份。
//!
//! 取数方式与 web 不同：**进程内直连**。
//!
//! * 装配来自 `envboard-host`（唯一组合根），本 crate 拿不到管理器类型，
//!   也就拿不到引擎装配符号 —— 判据在 `envboard-policy-tests` 的 `deps` 门禁第 8 条；
//! * 没有 HTTP 客户端、没有 SSE、没有 token：web 那三条推送流本来就是**轮询器**
//!   （`snapshot` 每秒全量、debug 每 500ms 增量、轨迹按字节偏移 tail），
//!   桌面形态用界面自己的定时器驱动同一个 `AdminService::snapshot()` 即可；
//! * tokio 运行时由 `main` 的栈帧持有，界面只拿 `Handle`；跨界只有 [`host::hop`]
//!   这一条通道。
//!
//! 框架只用一个 crate：`gpui-kit`（GPUI + gpui-component + gpui-base + 图标）。
//! 图标取 `gpui-kit-assets` 的**完整目录**（入口注册 `AllAssets`）：名字不在资产源里
//! 时 GPUI 不报错、不占位，只画空白 svg —— 默认集之外的名字等于一枚看不见的按钮。
//! 组件一律取现成的，不另造视觉语汇；本 crate 只写「取数 + 组装」那一层。
//!
//! 两条上游文档里不显眼、但踩中就是功能性缺陷的坑（都在这台机器的截图走查里抓到）：
//!
//! * **普通 `Dialog` 不画按钮**：`button_props`/`on_ok` 只装回调，页脚要自己给
//!   `DialogFooter::new().child(DialogClose…) .child(DialogAction…)` —— 不给就只有右上角
//!   一个 ✕，用户根本提交不了（`AlertDialog` 才有默认页脚）。
//! * **`Context::on_next_frame` 的闭包收到的第一个参数就是 `&mut 本实体`**：在里面再
//!   `weak_entity.update(同一个实体)` 会 panic（`cannot update X while it is already
//!   being updated`）。要动自己就用那个接收者；`app.rs` 里挂轮询器之所以能 update，
//!   是因为它动的是另一个实体。
//!
//! 分层：
//!
//! ```text
//! main.rs        CLI 翻译 + 运行时归属 + 装 Global + 开窗口
//! app.rs         根视图（标题栏 / 侧栏 / 状态栏）
//! views/*        各屏（DataTable、表单、面板…）
//! host.rs        tokio ↔ GPUI 的唯一通道
//! display.rs     展示层派生（纯函数，可单测）
//! errors.rs      错误的界面呈现（错误码 + 英文原文 + 中文提示）
//! ```

pub mod app;
pub mod display;
pub mod errors;
pub mod host;
pub mod ui_prefs;
pub mod views;

pub use app::{Destination, Workbench};
pub use host::Host;
pub use ui_prefs::{ThemePreference, UiPrefs};
pub use views::environments::Filter;

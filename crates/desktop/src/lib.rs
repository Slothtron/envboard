//! envboard-desktop —— envboard 的**桌面形态**：GPUI 原生工作台。
//!
//! 与 web 形态（`envboard-web` + `crates/server`）平级，消费同一份词汇
//! （`envboard-protocol` 的视图 DTO）与同一个门面（`envboard-admin`）。
//!
//! 取数方式与 web 不同：**进程内直连**。
//!
//! * 装配来自 `envboard-host`（唯一组合根），本 crate 拿不到管理器类型，
//!   也就拿不到引擎装配符号 —— 判据在 `envboard-policy-tests` 的 `deps` 门禁第 8 条；
//! * 没有 HTTP 客户端、没有 SSE、没有 token：web 那三条推送流本来就是**轮询器**
//!   （`snapshot` 每秒全量、debug 每 500ms 增量、轨迹按字节偏移 tail），
//!   桌面形态用界面自己的定时器驱动同一个 `AdminService::snapshot()` 即可；
//! * tokio 运行时由 `main` 的栈帧持有（与 `envboard-server` 同一手法），
//!   界面只拿 `Handle`；跨界只有一条通道（见 `host` 模块）。
//!
//! 框架只用一个 crate：`gpui-kit`（GPUI + gpui-component + gpui-base + 默认图标）。
//! 组件一律取现成的，不另造视觉语汇；本 crate 只写"取数 + 组装"那一层。

pub mod app;

pub use app::{Destination, HostInfo, Workbench};

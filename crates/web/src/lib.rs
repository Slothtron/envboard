//! envboard 工作台的 **web 形态**：管理器的一个界面。
//!
//! 管理器与工作台同进程（宿主 crate `envboard-server` 的唯一二进制把它们装在
//! 一起）：管理器本来就是常驻进程，web 只是它的一个界面 —— 免掉一条内部 IPC
//! 通道与一套鉴权，"一跑就有界面"是最低的启动成本。宿主负责编排（reconcile、
//! 日志照看、引擎装配），本 crate 只负责 HTTP 面的绑定。
//!
//! 前端的取舍
//!
//! * 资产用 `include_str!` 内嵌（标准库宏，零依赖）→ 管理器/工作台无运行时依赖；
//! * **不做**模板引擎 SSR、**不做** WASM、**不做**前端构建链 —— 理由是复杂度，
//!   不是"取不到"（那些 crate 本地缓存里没有，但 crates.io 是可达的）；
//! * CSP 严格（无 `unsafe-inline`），所以**样式与脚本必须外置**，且验收要断言
//!   "JS 真的跑了 + 样式真的生效 + 控制台没有 CSP 报错"三件事。
//!
//! 与未来界面形态的关系：本 crate 与将来的桌面形态（`envboard-desktop`）平级，
//! 消费同一份 `envboard-protocol` 词汇（视图 DTO / SSE 帧形状 / 游标），各自只
//! 做传输绑定 —— 协议不在 HTTP 里，HTTP 只是协议的一种载体。

pub mod api;
pub mod config;

pub use api::{CaAssets, router, serve};
pub use config::{CONTENT_SECURITY_POLICY, REQUEST_HEADER, TOKEN_HEADER, WebConfig};

//! envboard 工作台：管理器的界面。
//!
//! **管理器与工作台同进程**（一个二进制 `envboard`）：管理器本来就是常驻进程，
//! web 只是它的一个界面 —— 免掉一条内部 IPC 通道与一套鉴权，"一跑就有界面"
//! 是最低的启动成本。
//!
//! 前端的取舍
//!
//! * 资产用 `include_str!` 内嵌（标准库宏，零依赖）→ 管理器/工作台无运行时依赖；
//! * **不做**模板引擎 SSR、**不做** WASM、**不做**前端构建链 —— 理由是复杂度，
//!   不是"取不到"（那些 crate 本地缓存里没有，但 crates.io 是可达的）；
//! * CSP 严格（无 `unsafe-inline`），所以**样式与脚本必须外置**，且验收要断言
//!   "JS 真的跑了 + 样式真的生效 + 控制台没有 CSP 报错"三件事。

pub mod api;
pub mod config;

pub use api::{CaAssets, router, serve};
pub use config::{CONTENT_SECURITY_POLICY, REQUEST_HEADER, TOKEN_HEADER, WebConfig};

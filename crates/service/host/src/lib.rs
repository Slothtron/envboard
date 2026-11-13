//! envboard-host —— **唯一组合根**：把「引擎装配 + 管理器构造 + 宿主编排」收在一处。
//!
//! 为什么要有这个 crate
//!
//! 组合根本来写在宿主二进制里（一个界面形态一个 `main.rs`）。一旦界面形态不止一种
//! （今天的 web 控制台、将来的 Tauri 壳），或者测试要起一个真管理器，这份装配就得
//! 被复制第二遍 —— 而它是全仓最容易漂的一段：引擎选择、共享 CA 物化、单实例锁、
//! reconcile 顺序、日志照看循环，每一项都是"漏了就出问题"的编排职责。
//! 历次形态迁移（GPUI → web → Tauri）本 crate 一行未改，是这份拆分的直接兑现。
//!
//! 分层（依赖单向）
//!
//! ```text
//! envboard-protocol（词汇） ← envboard-manager ← envboard-admin（传输无关门面）
//!                                    ↑                    ↑
//!                              envboard-engine ────── envboard-host（本 crate）
//! ```
//!
//! 职责边界
//!
//! - **引擎装配符号只允许出现在本 crate**（`EngineBackend` / `SharedCa` /
//!   `EngineInstance` / `EngineConfig` / `CompiledConfig`），其他 crate 经
//!   [`Prepared`] 拿装配好的成品；
//! - **不认识传输**：没有 axum / HTTP / SSE，也没有任何界面类型；唯一的入口
//!   （桌面宿主 `envboard-desktop` 的 `lib.rs::boot`）只负责把命令行翻译成
//!   [`HostOptions`]，再把 [`Prepared`] 交给自己的界面；
//! - **不做领域判定**：配置合法性由 [`ManagerConfig::validate`](envboard_manager::ManagerConfig::validate) 与 domain 裁决，
//!   这里只负责按既定顺序装配与编排。
//!
//! 用法（宿主二进制的最小形状）
//!
//! ```no_run
//! use std::sync::Arc;
//!
//! use envboard_host::{HostOptions, Prepared};
//!
//! # fn demo() -> Result<(), envboard_engine::Error> {
//! let runtime = tokio::runtime::Builder::new_multi_thread()
//!     .enable_all()
//!     .build()
//!     .map_err(|error| envboard_engine::Error::internal_error(error.to_string()))?;
//! let prepared: Prepared = envboard_host::prepare(HostOptions {
//!     // 应用目录名是**产品身份**，由宿主二进制从自己的 `Cargo.toml` 取来传进来
//!     // （`env!("CARGO_BIN_NAME")`）；组合根不猜也不硬编码。
//!     app_dir_name: Some("your-app".to_owned()),
//!     ..HostOptions::default()
//! })?;
//! let report = runtime.block_on(prepared.startup())?;
//! for (environment, action) in &report.actions {
//!     println!("reconcile: {action:<14} {environment}");
//! }
//! // 界面形态只拿到门面：它拿不到 Manager，也就拿不到引擎装配。
//! let _admin: Arc<envboard_admin::AdminService> = Arc::clone(&prepared.admin);
//! # Ok(())
//! # }
//! ```

pub mod assembly;
pub mod ca_control;
pub mod options;
pub mod startup;
pub mod trust_store;

pub use assembly::Prepared;
pub use assembly::{prepare, prepare_with};
pub use ca_control::{CaControl, LeafPem};
pub use options::HostOptions;
pub use trust_store::{StorePresence, TrustProbe};

/// 状态目录的**布局表**（转出管理器的定义）。
///
/// 界面形态要拼状态目录里的路径时（例如界面偏好文件），路径规则必须只有一份 ——
/// 所以这里把布局表转出来，而不是让界面自己去 `join("ui.json")`。
pub use envboard_manager::layout;

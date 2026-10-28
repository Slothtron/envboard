//! envboard-host —— **唯一组合根**：把「引擎装配 + 管理器构造 + 宿主编排」收在一处。
//!
//! 为什么要有这个 crate
//!
//! 组合根本来写在宿主二进制里（一个界面形态一个 `main.rs`）。一旦界面形态不止一种
//! （今天的 web 工作台、正在建设的桌面 GUI），或者测试要起一个真管理器，这份装配就得
//! 被复制第二遍 —— 而它是全仓最容易漂的一段：引擎选择、共享 CA 物化、单实例锁、
//! reconcile 顺序、日志照看循环，每一项都是"漏了就出问题"的编排职责。
//! 决策记录见 docs/adr-002-composition-root-in-envboard-host.md。
//!
//! 分层（依赖单向，由 `envboard-policy-tests` 的 `deps` 门禁强制）
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
//!   `EngineInstance` / `EngineConfig` / `CompiledConfig`）—— 门禁判据由
//!   `envboard-server` 的 `src/main.rs` 一处改成本 crate 全部源码；
//! - **不认识传输**：没有 axum / HTTP / SSE，也没有任何界面类型；宿主二进制的
//!   `main.rs` 只负责把命令行翻译成 [`HostOptions`]，再把 [`Prepared`] 交给自己的界面；
//! - **不做领域判定**：配置合法性由 [`ManagerConfig::validate`] 与 domain 裁决，
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
//! let prepared: Prepared = envboard_host::prepare(HostOptions::default())?;
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
pub mod options;
pub mod startup;

pub use assembly::{CaBundle, Prepared};
pub use assembly::{prepare, prepare_with};
pub use options::{HostOptions, parse_port_range};

//! envboard v3 引擎后端的**测试替身**（FakeEngine）。
//!
//! 它的价值不变：让管理器的测试完全不依赖真引擎 —— 编排、端口分配、健康判定、
//! reconcile、装配回执这些逻辑占了管理器的大部分代码，与"谁来代理流量"无关。
//! 与真后端（EngineBackend）**同一抽象面**（ProxyEngine），生命周期、端口占用、
//! 状态回写全部同构，替身必须给真值，否则测试会绿得没有内容。
//!
//! 它刻意**不做**的事：不代理任何流量、不签证书、不解析规则。
//! 故障注入是**显式 setter**（inject_state / inject_log_drops / set_apply_failure），
//! 生产路径没有任何入口调用它们。

pub mod engine;

pub use engine::FakeEngine;

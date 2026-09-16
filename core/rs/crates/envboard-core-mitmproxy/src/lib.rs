//! `ProxyCore` 的 mitmproxy 实现。
//!
//! 编排层只认识 [`envboard_core_api::ProxyCore`]，而本 crate 是它的第一个真实实现：
//! 把 mitmproxy 藏在"启动参数 + 状态回传"这个粒度后面。
//!
//! 三个模块的分工：
//!
//! * [`python`]：解释器发现与自检（`float` 之类的宿主要求在这里就暴露）；
//! * [`ca`]：共享 CA 的预物化与**配对校验**；
//! * [`injector`]：单文件注入器的内嵌与物化；
//! * [`core`]：按契约拼参数、spawn、等状态文件、监督与探活。

pub mod ca;
pub mod core;
pub mod injector;
pub mod python;

pub use core::{MitmproxyCore, MitmproxyCoreConfig, default_core_bin};
pub use python::{CoreVersion, PythonInterpreter, parse_core_version};

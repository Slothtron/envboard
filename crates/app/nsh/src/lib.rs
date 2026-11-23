//! `envboard-nsh` 的库：把业务语义（`model`）、布局（`pages`）与壳（`shell`）
//! 摆在一起，给 `main.rs` 一个 `App` 组件。
//!
//! 为什么是 lib + bin 而不是一个 main：golden 测试要能挂到这个 crate 上，
//! 而且 `model` 的三条纯函数（`label` / `tone_shape` / `counts`）必须能在
//! 不开窗的情况下被单测钉住。

mod model;
mod pages;
mod shell;

pub use shell::App;

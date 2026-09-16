//! 契约测试的宿主 crate —— 真正的断言在 `tests/contract.rs`。
//!
//! 这里没有库代码，只有那条"语言中立契约必须被每个实现消费"的要求
//! 与 Rust 侧共用同一批 `core/spec/fixtures`；Python 侧由
//! `scripts/verify_contract.py` 跑，Rust 侧由本 crate 跑。

#![doc = ""]

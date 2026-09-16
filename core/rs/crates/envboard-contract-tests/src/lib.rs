//! 契约测试的宿主 crate —— 真正的断言在 `tests/contract.rs`。
//!
//! 这里没有库代码，只有那条"语言中立契约必须被每个实现消费"的要求：
//! 所有 fixture 的语义断言都在本 crate 的 `tests/contract.rs`。

#![doc = ""]

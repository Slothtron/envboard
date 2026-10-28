//! 各屏视图。一屏一模块文件，共享的展示词汇在 `crate::display`。

pub mod catalog;
pub mod detail;
pub mod environments;

pub use catalog::{ProxiesCatalog, RulesCatalog};
pub use detail::Detail;
pub use environments::Environments;

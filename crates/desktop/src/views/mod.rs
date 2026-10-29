//! 各屏视图。一屏一模块文件，共享的展示词汇在 `crate::display`。

pub mod ca;
pub mod catalog;
pub mod debug;
pub mod detail;
pub mod environments;
pub mod settings;

pub use ca::CaScreen;
pub use catalog::{ProxiesCatalog, RulesCatalog};
pub use debug::DebugScreen;
pub use detail::Detail;
pub use environments::Environments;
pub use settings::Settings;

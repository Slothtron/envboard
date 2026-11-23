//! `envboard-ui` —— 设计系统的全部：24 个颜色角色、12 个度量、4 个字号、8 个原语。
//!
//! 这个 crate 的边界靠 Cargo 守：`Cargo.toml` 里没有 `envboard-protocol`、
//! 没有 `envboard-manager`、没有 `envboard-admin`。想在这里读一次实例列表
//! 或调一次启停，编译就会失败 —— 这就是把它单独成 crate 的唯一硬理由。
//!
//! 它也不知道“环境”“规则”“端口”是什么：`primitives` 只接内容与回调，
//! 业务词汇（`Health`、`Env`、列宽）在 `crates/app/nsh` 的 `model` 与 `pages` 里。
//!
//! 主题是数据：`themes/default.json`（中国传统色，38 键 × 浅/深两套）。
//! 唯一源在这一个文件里，宿主用 `include_str!` 编进二进制，改色不必改代码。
//! 文件名按约定的 `default` 命名；具体是哪套色写在 JSON 的 `name` 字段里。

pub mod font;
pub mod metrics;
pub mod primitives;
pub mod tokens;

pub use primitives::{Shape, Tone};

/// 主题的唯一源：中国传统色（宣纸 / 夜黛），38 键 × 浅/深两套。
/// 宿主用 `Theme::from_json(envboard_ui::THEME_JSON)` 取用，改色不改代码。
pub const THEME_JSON: &str = include_str!("../themes/default.json");

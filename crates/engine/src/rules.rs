//! envboard 的 hosts 规则解析与确定性渲染（原 rules crate，并入 engine）。
//!
//! 契约是 `spec/rules.md`（BNF）—— golden fixture 的期望正文由本实现重算，
//! 契约测试逐例消费（v2 的"与 Python 注入器 `--render` 逐字节对拍"随第二份
//! 实现一起退场）。
//!
//! 纯逻辑：不碰文件系统、不碰网络（原子落盘与 0600 权限由 manager 负责）。

pub mod host;
pub mod insecure;
pub mod parse;
pub mod render;

pub use host::{normalize_host, validate_host, validate_normalized_host};
pub use insecure::{
    INSECURE_HOSTS_MAX, has_wildcard, matches as insecure_matches, normalize_insecure_host,
    normalize_insecure_hosts, validate_insecure_host,
};
pub use parse::{HostConflict, RulesImport, SkipReason, SkippedItem, parse_hosts_text};
pub use render::{ENTRIES_FIELD, render_rules};

/// 规则文件后缀：`<rules_dir>/<name>.rules`。
pub const RULES_SUFFIX: &str = ".rules";

/// 规则名 → 文件名。**只接受白名单名字**（校验见 `domain` 的
/// `validate_rules_name`），这里再拼后缀，绝不接受调用方给的路径片段。
pub fn file_name_for(name: &str) -> String {
    format!("{name}{RULES_SUFFIX}")
}

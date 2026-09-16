//! envboard 的 hosts 规则解析与确定性渲染。
//!
//! 契约是 `core/spec/rules.md`（BNF）——**两份实现的共同仲裁**：本 crate（管理器侧，
//! 负责导入 / 规范化 / 落盘）与 Python 注入器（运行时解析 + 热重载）必须对着同一份
//! BNF 写，且对同一批 `core/spec/fixtures/rules/*.json` 输出**逐字节一致**。
//!
//! 纯逻辑：不碰文件系统、不碰网络（原子落盘与 0600 权限由 `envboard-manager` 负责）。

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

/// 规则名 → 文件名。**只接受白名单名字**（校验见 `envboard-domain` 的
/// `validate_rules_name`），这里再拼后缀，绝不接受调用方给的路径片段。
pub fn file_name_for(name: &str) -> String {
    format!("{name}{RULES_SUFFIX}")
}

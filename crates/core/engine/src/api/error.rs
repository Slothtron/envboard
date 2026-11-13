//! 错误码与错误类型 —— **本模块文档就是错误码契约**。
//!
//! **core 判定，适配器只翻译**：REST 面把 [`Error`] 原样序列化成
//! `{"error": {"code", "message", "field"}}`（`field` 是可选的点分路径，如
//! `environment.listen.port`），不重新判定。错误码字面量、HTTP 映射与可重试性
//! 以下表为准，改动即 breaking。
//!
//! | code | HTTP | 可重试 | 含义 |
//! |---|---|---|---|
//! | `invalid_config` | 400 | ✗ | 配置非法，编译即失败（含热更被拒：旧快照继续服务并留标记） |
//! | `not_found` | 404 | ✗ | 目标不存在 |
//! | `conflict` | 409 | ✗ | 状态冲突（重名、删除被绑定/引用的实体，错误点名引用方） |
//! | `port_conflict` | 409 | △ | 端口被别的程序占用；**已持久化端口禁止静默重分配**，须显式动作 |
//! | `port_range_exhausted` | 409 | ✓ | 区间内没有可用端口（连续 N 次候选失败，默认 32） |
//! | `store_failure` | 500 | ✓ | 状态文件读写失败（权限 / JSON 损坏 / 磁盘满） |
//! | `internal_error` | 500 | ✓ | 未归类的内部错误，兜底，不应出现 |
//!
//! `△` = 重试要换输入：端口冲突本身可重试，但不允许自动重分配已持久化的端口。
//! [`ErrorCode::ConfigMismatch`] 是历史兼容占位（热更新同步生效后不存在
//! "起来了但配置没生效"的中间态），不再出现在新判定里。
//!
//! **可重试性是契约的一部分**：401/403 禁止静默重试；界面与脚本判定的一律是
//! `code` 与 [`Retryability`]，HTTP 状态码只是传输绑定（web 层按
//! [`ErrorCode::http_status`] 映射）。
//!
//! ## 归一化规则
//!
//! 1. **配置错误与运行错误必须分开**：`invalid_config` 需要改配置；
//!    `port_conflict` 与被拒配置标记是运行态问题。禁止互相伪装。
//! 2. **禁止静默降级**：未知字段、缺依赖、端口被占都必须响亮失败。唯一不算
//!    降级的是 `rules = null` —— 那是显式语义"本环境不覆盖任何域名"。
//! 3. **`port_conflict` 不得自动换端口**：已持久化的端口是环境的对外身份。
//!    自动换端口只允许发生在**新建环境**的启动重试里，且仅一次。
//! 4. **错误信息必须可行动**：涉及端口的错误给出端口号与"谁占着/怎么释放"。
//!
//! ## 界面呈现：错误码 + 英文 msg + 中文提示
//!
//! 界面上每条错误由三层组成，缺一不可：错误码（core 判定，不翻译不改写）、
//! 英文原文（服务端 `message` + `field`，**不翻译、不截断、不删除**，含唯一
//! 可行动细节）、中文主句（`{动作}{对象}失败：{该码的中文结论}`，按码映射）。
//! 映射只有一处实现：`frontend/src/errors.ts`（`ERROR_HINTS` 覆盖每个码，
//! `formatErrorNotice` 拼主句与原文）。界面自判的失败（本地表单校验等）没有
//! 服务端回执，只给中文一句，不编造码与英文原文。
//!
//! **错误的生命周期**：取数失败的提示在下一次成功取数时消失；**写操作失败
//! 不会** —— 随后取数成功只说明"读得到"，不能证明刚才那次写成功
//! （`frontend/src/Workbench.tsx` 的反馈区：写失败进固定 Alert，不随快照撤除）。

use std::path::Path;

use serde::{Deserialize, Serialize};

/// 统一错误码。`serde` 的 snake_case 名字就是对外契约里的字面量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// 配置非法，**加载即失败**。需要改配置，不可重试。
    InvalidConfig,
    /// 目标不存在。
    NotFound,
    /// 状态冲突（重名、删除被绑定的规则文件…）。
    Conflict,
    /// 端口被别的程序占用。**已持久化的端口禁止静默重分配**，要靠一次显式动作。
    PortConflict,
    /// 区间内没有可用端口（连续 N 次候选都不可用）。
    PortRangeExhausted,
    /// 实例起来了，但生效配置与期望不一致（宿主对未知 `--set` 是静默忽略的）。
    ConfigMismatch,
    /// 状态文件读写失败。
    StoreFailure,
    /// 未归类的内部错误，兜底。
    InternalError,
}

/// 可重试性 —— 契约里的"可重试"一列（必须区分可重试与需改输入）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retryability {
    /// 改配置才能过。
    No,
    /// 可重试，但要换输入（例如显式重分配端口）。
    WithNewInput,
    /// 可重试。
    Yes,
}

impl ErrorCode {
    /// REST 面的 HTTP 状态码（映射表见本模块文档）。
    pub fn http_status(self) -> u16 {
        match self {
            ErrorCode::InvalidConfig => 400,
            ErrorCode::NotFound => 404,
            ErrorCode::Conflict | ErrorCode::PortConflict | ErrorCode::PortRangeExhausted => 409,
            ErrorCode::ConfigMismatch => 503,
            ErrorCode::StoreFailure | ErrorCode::InternalError => 500,
        }
    }

    pub fn retryability(self) -> Retryability {
        match self {
            ErrorCode::InvalidConfig | ErrorCode::NotFound | ErrorCode::Conflict => {
                Retryability::No
            }
            ErrorCode::PortConflict => Retryability::WithNewInput,
            ErrorCode::PortRangeExhausted
            | ErrorCode::ConfigMismatch
            | ErrorCode::StoreFailure
            | ErrorCode::InternalError => Retryability::Yes,
        }
    }

    /// 契约里的字面量（用于日志与断言；serde 已保证序列化一致）。
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::InvalidConfig => "invalid_config",
            ErrorCode::NotFound => "not_found",
            ErrorCode::Conflict => "conflict",
            ErrorCode::PortConflict => "port_conflict",
            ErrorCode::PortRangeExhausted => "port_range_exhausted",
            ErrorCode::ConfigMismatch => "config_mismatch",
            ErrorCode::StoreFailure => "store_failure",
            ErrorCode::InternalError => "internal_error",
        }
    }
}

/// 带 `field` 路径的错误。`field` 是点分路径（如 `environment.listen.port`），
/// fixture 与 REST 面都按它断言。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            field: None,
        }
    }

    /// 带字段路径的错误 —— 契约要求"非法配置加载即失败，附字段路径"。
    pub fn at(code: ErrorCode, field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            field: Some(field.into()),
        }
    }

    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InternalError, message)
    }

    pub fn invalid_config(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::at(ErrorCode::InvalidConfig, field, message)
    }

    /// 文件系统失败的**带路径**形态。
    ///
    /// 走 `From<std::io::Error>` 只能得到裸的 `io error: 拒绝访问。 (os error 5)` ——
    /// 里面没有路径，而"哪个目录被拒"恰恰是排障的唯一线索（状态目录、CA 材料、
    /// 单实例锁都可能在启动时写不进去）。状态目录相关的 IO 一律用这个构造，
    /// 让错误自己说清楚它动的是哪个文件。
    pub fn io_at(operation: &str, path: &Path, error: std::io::Error) -> Self {
        Self::new(
            ErrorCode::StoreFailure,
            format!("cannot {operation} {}: {error}", path.display()),
        )
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Error::new(ErrorCode::StoreFailure, format!("io error: {value}"))
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Error::new(ErrorCode::StoreFailure, format!("json error: {value}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_match_the_contract_literals() {
        // 这些字面量是本模块文档契约表的内容，改字就是 breaking change。
        assert_eq!(ErrorCode::InvalidConfig.as_str(), "invalid_config");
        assert_eq!(ErrorCode::PortConflict.as_str(), "port_conflict");
        assert_eq!(
            ErrorCode::PortRangeExhausted.as_str(),
            "port_range_exhausted"
        );
        assert_eq!(ErrorCode::ConfigMismatch.as_str(), "config_mismatch");
        assert_eq!(ErrorCode::StoreFailure.as_str(), "store_failure");
        assert_eq!(ErrorCode::InternalError.as_str(), "internal_error");
        assert_eq!(ErrorCode::NotFound.as_str(), "not_found");
        assert_eq!(ErrorCode::Conflict.as_str(), "conflict");
    }

    #[test]
    fn error_serializes_with_field_path() {
        let error = Error::invalid_config("environment.listen.port", "port 0 is invalid");
        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(json["code"], "invalid_config");
        assert_eq!(json["field"], "environment.listen.port");
    }
}

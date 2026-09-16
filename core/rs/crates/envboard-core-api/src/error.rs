//! 错误码与错误类型 —— 契约见 `core/spec/errors.md`。
//!
//! **core 判定，适配器只翻译**：REST 面把 [`Error`] 原样序列化成
//! `{"error": {"code", "message", "field"}}`，不重新判定；错误码字面量与
//! `core/spec/errors.md` 的表一一对应，改动即 breaking。

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
    /// REST 面的 HTTP 状态码，见 `core/spec/errors.md`。
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
        // 这些字面量是 core/spec/errors.md 的表内容，改字就是 breaking change。
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

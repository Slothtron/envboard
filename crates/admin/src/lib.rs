//! envboard-admin —— **Admin 管理 API 的门面**（传输无关）。
//!
//! 分层（依赖单向，由 deps 门禁强制）：
//!
//! ```text
//! envboard-protocol（词汇） ← envboard-manager（账本与装配编排） ← envboard-admin（本 crate）
//!                                                      ↑
//!                                        envboard-web（HTTP/SSE 传输绑定）
//! ```
//!
//! 职责边界：
//! - **入口校验词汇化**：写端点请求体先过 [`protocol`](envboard_protocol) 的类型化
//!   请求 DTO（`deny_unknown_fields`，未知字段响亮拒绝），再转发 manager；
//! - **统一错误信封**：所有失败以 [`Error`](envboard_engine::Error) 返回，
//!   错误码契约见 `spec/errors.md`；
//! - **不认识传输**：没有 axum / HTTP 状态码 / SSE —— 那些是 web 形态的绑定；
//! - **不认识引擎装配**：装配符号只允许 envboard-server 的组合根（deps 门禁）。
//!
//! 契约文档见包根 `spec/admin-api.md`。

mod activity;
mod debug;
mod environments;
mod har;
mod proxies;
mod rules;
mod settings;

use std::sync::Arc;

use envboard_engine::Error;
use envboard_manager::Manager;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Admin API 的服务面：包一个 [`Manager`]，一域一模块（见各文件头注释）。
#[derive(Clone)]
pub struct AdminService {
    manager: Arc<Manager>,
}

impl AdminService {
    pub fn new(manager: Arc<Manager>) -> Self {
        Self { manager }
    }

    /// 从裸 JSON 解析类型化请求；失败归 `invalid_config`（field 指向请求体）。
    ///
    /// DTO 只做**形状**校验（键集合与类型）；字段级语义校验仍归 domain
    /// （`Environment::from_json` 等），两层共用同一份词汇。
    fn parse_request<T: DeserializeOwned>(field: &str, body: &Value) -> Result<T, Error> {
        serde_json::from_value(body.clone()).map_err(|error| {
            Error::invalid_config(field, format!("malformed request body: {error}"))
        })
    }
}

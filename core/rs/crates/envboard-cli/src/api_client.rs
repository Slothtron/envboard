//! 本地 API 的**瘦客户端**。
//!
//! 为什么必须有它：状态只有一个写者。管理器（工作台）常驻时持有 flock，
//! 如果 CLI 还直接去驱动管理器，就会变成"起了工作台就没法用命令行"——
//! 我在 M0 冒烟里实测到的正是这条冲突。所以 CLI 先探测本地 API：
//!
//! * **有常驻实例** → 走 HTTP，绝不碰状态文件、也不抢锁；
//! * **没有** → 退回"自己驱动管理器并持锁"的模式（仍然只有一个写者）。
//!
//! 刻意手写一个极小的 HTTP/1.1 客户端而不引 HTTP 客户端库：目标是**回环上的
//! 单个 JSON 请求**，一个库带来的依赖闭包与配置面（超时、重定向、TLS、代理环境变量）
//! 远大于它解决的问题。请求成功后连接即关闭，不做连接复用。

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use envboard_core_api::{Error, ErrorCode};
use serde_json::Value;

/// 变更类请求必须带的头（与服务端的 CSRF 防线对应：服务端拒绝缺这个头的变更请求）。
const REQUEST_HEADER: &str = "x-envboard-request";
const TOKEN_HEADER: &str = "x-envboard-token";

#[derive(Debug, Clone)]
pub struct ApiClient {
    pub addr: SocketAddr,
    pub token: Option<String>,
}

impl ApiClient {
    /// 探测回环上是否真的有一个 envboard 管理器在服务。
    ///
    /// 只用 `GET /api/status`：它无副作用，且能证明"这是我们自己的服务"——
    /// 端口上蹲着别的程序时，返回的不是我们能解析的 JSON，于是判定为没有。
    pub fn probe(addr: SocketAddr, token: Option<String>) -> Option<Self> {
        let client = Self { addr, token };
        match client.request("GET", "/api/status", None) {
            Ok(value) if value.get("core").is_some() => Some(client),
            _ => None,
        }
    }

    pub fn get(&self, path: &str) -> Result<Value, Error> {
        self.request("GET", path, None)
    }

    pub fn post(&self, path: &str, body: Option<&Value>) -> Result<Value, Error> {
        self.request("POST", path, body)
    }

    pub fn patch(&self, path: &str, body: &Value) -> Result<Value, Error> {
        self.request("PATCH", path, Some(body))
    }

    pub fn delete(&self, path: &str) -> Result<Value, Error> {
        self.request("DELETE", path, None)
    }

    fn request(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, Error> {
        let payload = match body {
            Some(value) => serde_json::to_string(value)
                .map_err(|error| Error::internal_error(format!("cannot encode body: {error}")))?,
            None => String::new(),
        };

        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAccept: application/json\r\n",
            self.addr
        );
        if !payload.is_empty() {
            request.push_str("Content-Type: application/json\r\n");
            request.push_str(&format!("Content-Length: {}\r\n", payload.len()));
        }
        if method != "GET" {
            // 服务端要求变更类请求带这个头；不带头会被 403（那正是 CSRF 防线）。
            request.push_str(&format!("{REQUEST_HEADER}: 1\r\n"));
        }
        if let Some(token) = &self.token {
            request.push_str(&format!("{TOKEN_HEADER}: {token}\r\n"));
        }
        request.push_str("\r\n");
        request.push_str(&payload);

        let mut stream = TcpStream::connect_timeout(&self.addr, Duration::from_millis(800))
            .map_err(|error| {
                Error::new(
                    ErrorCode::NotFound,
                    format!("cannot reach the envboard API on {}: {error}", self.addr),
                )
            })?;
        stream.set_read_timeout(Some(Duration::from_secs(20))).ok();
        stream.set_write_timeout(Some(Duration::from_secs(10))).ok();
        stream.write_all(request.as_bytes()).map_err(|error| {
            Error::new(
                ErrorCode::InternalError,
                format!("cannot send request: {error}"),
            )
        })?;

        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).map_err(|error| {
            Error::new(
                ErrorCode::InternalError,
                format!("cannot read response: {error}"),
            )
        })?;
        let text = String::from_utf8_lossy(&raw).to_string();

        let (head, body) = text.split_once("\r\n\r\n").ok_or_else(|| {
            Error::new(
                ErrorCode::InternalError,
                "malformed HTTP response (no header terminator)",
            )
        })?;
        let status: u16 = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);

        let value: Value = if body.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(body).map_err(|error| {
                Error::new(
                    ErrorCode::InternalError,
                    format!("API returned non-JSON (HTTP {status}): {error}"),
                )
            })?
        };

        if (200..300).contains(&status) {
            return Ok(value);
        }
        // 服务端的错误是契约化的 `{"error": {code, message, field}}`：只翻译，不重新判定。
        let error = &value["error"];
        let code = match error["code"].as_str() {
            Some("invalid_config") => ErrorCode::InvalidConfig,
            Some("not_found") => ErrorCode::NotFound,
            Some("conflict") => ErrorCode::Conflict,
            Some("port_conflict") => ErrorCode::PortConflict,
            Some("port_range_exhausted") => ErrorCode::PortRangeExhausted,
            Some("config_mismatch") => ErrorCode::ConfigMismatch,
            Some("store_failure") => ErrorCode::StoreFailure,
            _ => ErrorCode::InternalError,
        };
        let message = error["message"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("HTTP {status}"));
        let field = error["field"].as_str().map(str::to_string);
        Err(Error {
            code,
            message,
            field,
        })
    }
}

/// 常驻实例把自己监听的地址写到 `<state_dir>/runtime/api.json`，
/// CLI 靠它发现自定义端口的工作台（而不是只会试默认的 8900）。
pub fn api_record_path(state_dir: &std::path::Path) -> std::path::PathBuf {
    state_dir.join("runtime").join("api.json")
}

pub fn write_api_record(state_dir: &std::path::Path, addr: SocketAddr) -> std::io::Result<()> {
    let path = api_record_path(state_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::json!({"listen": addr.to_string()}).to_string();
    std::fs::write(&path, body)
}

pub fn read_api_record(state_dir: &std::path::Path) -> Option<SocketAddr> {
    let raw = std::fs::read_to_string(api_record_path(state_dir)).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    value.get("listen")?.as_str()?.parse().ok()
}

pub fn remove_api_record(state_dir: &std::path::Path) {
    let _ = std::fs::remove_file(api_record_path(state_dir));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_reports_nothing_when_nobody_listens() {
        // 绑一个端口再立刻释放：这个地址上一定没有服务
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        assert!(ApiClient::probe(addr, None).is_none());
    }

    #[test]
    fn api_record_round_trips() {
        let dir = std::env::temp_dir().join(format!("envboard-api-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let addr: SocketAddr = "127.0.0.1:8901".parse().unwrap();
        write_api_record(&dir, addr).unwrap();
        assert_eq!(read_api_record(&dir), Some(addr));
        remove_api_record(&dir);
        assert_eq!(read_api_record(&dir), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}

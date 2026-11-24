//! 一条抓包记录的形状（从 `capture_detail` 的 JSON 直接读）。
//!
//! 解析**宽容**：字段缺了给中性值，不 panic、也不猜——详情面板宁可少一行，
//! 不能因为某条记录的头字段形状怪就整块报错。正文三形状交给 `body::Body`。

use envboard_ui::primitives::Tone;

use crate::model::body::Body;
use crate::model::capture::format_bytes;

/// 详情面板要显示的全部内容（列表的瘦行之外，外加头部与正文）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Detail {
    pub request_id: u64,
    pub time_ms: u64,
    pub method: String,
    pub authority: String,
    /// 纯路径（不含 query）。
    pub path: String,
    /// 原始查询串（不含 `?`；空串 = 没有）。
    pub query: String,
    pub status: Option<u16>,
    pub error: Option<String>,
    pub request_headers: Vec<(String, String)>,
    pub response_headers: Vec<(String, String)>,
    pub request_body: Body,
    pub response_body: Body,
    pub request_content_type: Option<String>,
    pub response_content_type: Option<String>,
}

impl Detail {
    pub fn from_value(value: &serde_json::Value) -> Self {
        let request = value.get("request");
        let response = value.get("response");
        let text = |v: Option<&serde_json::Value>, key: &str| {
            v.and_then(|v| v.get(key))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let request_headers = headers_of(request);
        let response_headers = headers_of(response);
        Detail {
            request_id: value
                .get("request_id")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            time_ms: value
                .get("time")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            method: text(request, "method"),
            authority: text(request, "authority"),
            path: text(request, "path"),
            query: text(request, "query"),
            status: response
                .and_then(|r| r.get("status"))
                .and_then(serde_json::Value::as_u64)
                .map(|s| s as u16),
            error: value.get("error").filter(|e| !e.is_null()).map(|e| match e {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            }),
            request_content_type: content_type_of(&request_headers),
            response_content_type: content_type_of(&response_headers),
            request_body: Body::from_value(request.and_then(|r| r.get("body"))),
            response_body: Body::from_value(response.and_then(|r| r.get("body"))),
            request_headers,
            response_headers,
        }
    }

    /// 拼回可读的一行：`https://authority/path?query`（代理不记 scheme，
    /// 统一按 https 展示——envboard 是 HTTPS 中间人）。
    pub fn url(&self) -> String {
        let mut out = format!("https://{}{}", self.authority, self.path);
        if !self.query.is_empty() {
            out.push('?');
            out.push_str(&self.query);
        }
        out
    }

    /// `HH:MM:SS.mmm`（本地时区）。时间为 0（缺字段）就给「—」。
    pub fn time_text(&self) -> String {
        if self.time_ms == 0 {
            return "—".to_owned();
        }
        match chrono::DateTime::from_timestamp_millis(self.time_ms as i64) {
            Some(utc) => {
                let local = utc.with_timezone(&chrono::Local);
                local.format("%H:%M:%S.%3f").to_string()
            }
            None => "—".to_owned(),
        }
    }

    pub fn status_text(&self) -> String {
        if self.error.is_some() {
            return "ERR".to_owned();
        }
        match self.status {
            Some(status) => status.to_string(),
            None => "—".to_owned(),
        }
    }

    pub fn status_tone(&self) -> Tone {
        if self.error.is_some() {
            return Tone::Critical;
        }
        match self.status {
            Some(status) if status >= 500 => Tone::Critical,
            Some(status) if status >= 400 => Tone::Caution,
            _ => Tone::Neutral,
        }
    }

    pub fn request_size_text(&self) -> String {
        format_bytes(self.request_body.size)
    }

    pub fn response_size_text(&self) -> String {
        format_bytes(self.response_body.size)
    }
}

fn headers_of(value: Option<&serde_json::Value>) -> Vec<(String, String)> {
    let Some(list) = value
        .and_then(|v| v.get("headers"))
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|pair| {
            let pair = pair.as_array()?;
            let name = pair.first()?.as_str()?;
            let value = pair.get(1)?.as_str().unwrap_or_default();
            Some((name.to_owned(), value.to_owned()))
        })
        .collect()
}

/// content-type 取原值（不带参数），大小写不敏感地找。
fn content_type_of(headers: &[(String, String)]) -> Option<String> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.split(';').next().unwrap_or(value).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record() -> serde_json::Value {
        json!({
            "version": 2,
            "session": 7,
            "request_id": 42,
            "time": 1_700_000_000_123u64,
            "request": {
                "method": "POST",
                "path": "/v1/orders",
                "query": "page=2",
                "authority": "api.example.com",
                "headers": [["Content-Type", "application/json"], ["X-Trace", "a"]],
                "body": {"encoding": "utf8", "size": 12, "content": "{\"n\":1,\"m\":2}"},
            },
            "response": {
                "status": 502,
                "headers": [["content-type", "text/html; charset=utf-8"]],
                "body": {"omitted": true, "size": 400_000},
            },
            "error": null,
        })
    }

    #[test]
    fn parses_record_with_bodies_and_headers() {
        let d = Detail::from_value(&record());
        assert_eq!(d.request_id, 42);
        assert_eq!(d.method, "POST");
        assert_eq!(d.url(), "https://api.example.com/v1/orders?page=2");
        assert_eq!(d.status, Some(502));
        assert_eq!(d.status_tone(), Tone::Critical);
        assert_eq!(d.request_headers.len(), 2);
        assert_eq!(d.request_headers[0].0, "Content-Type");
        // content-type 只留媒体类型；响应头大小写不敏感也能找到
        assert_eq!(d.request_content_type.as_deref(), Some("application/json"));
        assert_eq!(d.response_content_type.as_deref(), Some("text/html"));
        // 请求体可美化、响应体 omitted 只能给说明
        assert!(d.request_body.pretty(d.request_content_type.as_deref()).is_some());
        assert!(d.response_body.note().unwrap().contains("未记录"));
        assert_eq!(d.response_size_text(), "390.6 KB");
    }

    #[test]
    fn missing_fields_stay_neutral() {
        let d = Detail::from_value(&json!({ "request_id": 1 }));
        assert_eq!(d.status, None);
        assert_eq!(d.status_text(), "—");
        assert!(d.request_headers.is_empty());
        assert_eq!(d.time_text(), "—");
        assert_eq!(d.url(), "https://");
        // error 非 null 时状态列写 ERR，哪怕有状态码
        let d = Detail::from_value(&json!({
            "request_id": 2,
            "request": {"method": "GET", "path": "/x", "authority": "a"},
            "response": {"status": 200, "headers": []},
            "error": "connection reset",
        }));
        assert_eq!(d.status_text(), "ERR");
        assert_eq!(d.status_tone(), Tone::Critical);
        assert_eq!(d.error.as_deref(), Some("connection reset"));
    }
}

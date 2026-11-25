//! HAR 1.2 导出（EB-DESIGN-004 §9 问题 2）：只含我们记录到的字段。
//!
//! 输入是 typed [`Record`]（不读取任何 JSON 中间形态）；输出是一份 HAR 文档 ——
//! HAR 本身就是 JSON 文件格式，这里的一次编码属于**落盘边界**，不是进程内传输。
//! 纯函数，落盘与界面都不掺进来，好单测。

use std::sync::Arc;

use envboard_protocol::{Record, RecordBody};
use serde_json::{Value, json};

/// 拼一整个 HAR：`log.version` 固定 1.2，creator 写本进程版本。
pub fn har(env: &str, records: &[Arc<Record>]) -> Value {
    let entries: Vec<Value> = records
        .iter()
        .filter_map(|record| entry_of(record))
        .collect();
    json!({
        "log": {
            "version": "1.2",
            "creator": {"name": "envboard", "version": env!("CARGO_PKG_VERSION")},
            "pages": [],
            "_env": env,
            "entries": entries,
        }
    })
}

/// 一条记录 → 一条 entry；无响应的记录（比如以错误收尾的那条）直接跳过——
/// HAR 没有「无响应」的表达，硬塞一个空 response 会把下游解析器带偏。
fn entry_of(record: &Record) -> Option<Value> {
    let response = record.response.as_ref()?;

    // 抓包记录 v2 起 path 与 query 分存（query 不带前导 `?`）；URL 必须拼回查询串，
    // 否则导出丢参数。协议照记录里的 `scheme` 写；空值（旧记录 / 夹具）沿用历史行为 `http`。
    let scheme = if record.request.scheme.is_empty() {
        "http"
    } else {
        record.request.scheme.as_str()
    };
    let url = format!(
        "{scheme}://{}{}{}",
        record.request.authority,
        record.request.path,
        if record.request.query.is_empty() {
            String::new()
        } else {
            format!("?{}", record.request.query)
        },
    );

    Some(json!({
        // HAR 要 ISO-8601；我们记的是毫秒数，原样写成字符串并保留毫秒（v2 才补 ISO 格式）。
        "startedDateTime": if record.time == 0 {
            String::new()
        } else {
            record.time.to_string()
        },
        "time": 0,
        "_envboard": {
            "session": record.session,
            "request_id": record.request_id,
        },
        "request": {
            "method": record.request.method,
            "url": url,
            "httpVersion": "HTTP/1.1",
            "headers": headers(&record.request.headers),
            "queryString": [],
            "headersSize": -1,
            "bodySize": body_size(record.request.body.as_ref()),
            "postData": content(record.request.body.as_ref()),
        },
        "response": {
            "status": match response.status {
                Some(status) => json!(status),
                None => Value::Null,
            },
            "statusText": "",
            "httpVersion": "HTTP/1.1",
            "headers": headers(&response.headers),
            "content": content(response.body.as_ref()),
            "headersSize": -1,
            "bodySize": body_size(response.body.as_ref()),
            "redirectURL": "",
        },
        "cache": {},
        "timings": {"send": 0, "wait": -1, "receive": 0},
    }))
}

/// 正文块：`omitted`（超后端上限）只留体积，`content` 照原文给（base64 的也原样带走）。
fn content(body: Option<&RecordBody>) -> Value {
    let Some(body) = body else {
        return json!({"size": 0, "mimeType": null, "text": ""});
    };
    match body {
        RecordBody::Omitted { size, .. } => json!({"size": size, "mimeType": null, "text": null}),
        RecordBody::Present { size, content, .. } => {
            json!({"size": size, "mimeType": null, "text": content})
        }
    }
}

/// 正文的字节数；没有正文给 -1（不编数字）。
fn body_size(body: Option<&RecordBody>) -> Value {
    match body {
        Some(body) => json!(body.size()),
        None => json!(-1),
    }
}

/// `[(k, v), …]` → `[{name, value}, …]`。
fn headers(pairs: &[(String, String)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(name, value)| json!({"name": name, "value": value}))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use envboard_protocol::{RecordError, RecordRequest, RecordResponse};

    fn record(id: u64, query: &str) -> Record {
        Record {
            version: 2,
            session: 7,
            request_id: id,
            time: 1_700_000_000_123,
            duration_ms: 4,
            request: RecordRequest {
                method: "GET".to_owned(),
                path: "/v1/orders".to_owned(),
                query: query.to_owned(),
                authority: "api.example.com".to_owned(),
                scheme: "https".to_owned(),
                headers: vec![("accept".to_owned(), "application/json".to_owned())],
                body: None,
            },
            response: Some(RecordResponse {
                status: Some(200),
                headers: vec![("content-type".to_owned(), "application/json".to_owned())],
                body: Some(RecordBody::Present {
                    encoding: "utf8".to_owned(),
                    size: 11,
                    content: "{\"ok\":true}".to_owned(),
                }),
            }),
            error: None,
        }
    }

    fn records(items: Vec<Record>) -> Vec<Arc<Record>> {
        items.into_iter().map(Arc::new).collect()
    }

    #[test]
    fn url_keeps_query_string() {
        let with_query = har("proxy-a", &records(vec![record(1, "page=2")]));
        let url = with_query["log"]["entries"][0]["request"]["url"]
            .as_str()
            .unwrap_or_default();
        assert_eq!(url, "https://api.example.com/v1/orders?page=2");
        // 没有查询串时不留孤零零的 `?`
        let plain = har("proxy-a", &records(vec![record(2, "")]));
        assert_eq!(
            plain["log"]["entries"][0]["request"]["url"]
                .as_str()
                .unwrap_or_default(),
            "https://api.example.com/v1/orders"
        );
    }

    /// 导出照记录里的协议写：明文流量是 `http://`，旧记录（scheme 空）沿用 `http`。
    #[test]
    fn url_uses_recorded_scheme() {
        let mut plain = record(4, "page=2");
        plain.request.scheme = "http".to_owned();
        let exported = har("proxy-a", &records(vec![plain]));
        assert_eq!(
            exported["log"]["entries"][0]["request"]["url"]
                .as_str()
                .unwrap_or_default(),
            "http://api.example.com/v1/orders?page=2"
        );

        let mut legacy = record(5, "");
        legacy.request.scheme.clear();
        let exported = har("proxy-a", &records(vec![legacy]));
        assert_eq!(
            exported["log"]["entries"][0]["request"]["url"]
                .as_str()
                .unwrap_or_default(),
            "http://api.example.com/v1/orders"
        );
    }

    #[test]
    fn header_and_meta_shapes() {
        let with_headers = har("proxy-a", &records(vec![record(3, "")]));
        let entry = &with_headers["log"]["entries"][0];
        assert_eq!(entry["request"]["headers"][0]["name"], "accept");
        assert_eq!(entry["request"]["headers"][0]["value"], "application/json");
        assert_eq!(entry["_envboard"]["request_id"], 3);
        assert_eq!(entry["_envboard"]["session"], 7);
        // 没记请求正文：`postData.content` 给空串、`bodySize` 给 -1，不编数字
        assert_eq!(entry["request"]["bodySize"], -1);
        assert_eq!(entry["request"]["postData"]["text"], "");
        assert_eq!(entry["response"]["bodySize"], 11);
        assert_eq!(with_headers["log"]["version"], "1.2");
        assert_eq!(with_headers["log"]["_env"], "proxy-a");
    }

    #[test]
    fn omitted_body_has_no_text_and_errors_are_skipped() {
        let omitted = Record {
            version: 2,
            session: 7,
            request_id: 4,
            time: 1,
            duration_ms: 0,
            request: RecordRequest {
                method: "POST".to_owned(),
                path: "/x".to_owned(),
                authority: "h".to_owned(),
                ..RecordRequest::default()
            },
            response: Some(RecordResponse {
                status: Some(200),
                headers: Vec::new(),
                body: Some(RecordBody::Omitted {
                    omitted: true,
                    size: 300_000,
                }),
            }),
            error: None,
        };
        // 收尾错误的那条没有 response，HAR 表达不了，跳过
        let failed = Record {
            version: 2,
            session: 7,
            request_id: 5,
            time: 2,
            duration_ms: 0,
            request: RecordRequest {
                method: "GET".to_owned(),
                path: "/y".to_owned(),
                authority: "h".to_owned(),
                ..RecordRequest::default()
            },
            response: None,
            error: Some(RecordError::Text("connection reset".to_owned())),
        };
        let exported = har("proxy-a", &records(vec![omitted, failed]));
        assert_eq!(exported["log"]["entries"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            exported["log"]["entries"][0]["response"]["content"]["size"],
            300_000
        );
        assert_eq!(
            exported["log"]["entries"][0]["response"]["content"]["text"],
            Value::Null
        );
    }
}

//! HAR 1.2 导出（EB-DESIGN-004 §9 问题 2）：只含我们记录到的字段。
//!
//! 与桌面 `ops.rs` 的 `har_from_records` 同形（那一份自己注释「自 web 传输面原样移植」），
//! 这里只做纯函数：记录 JSON 进、`Value` 出，落盘与界面都不掺进来，好单测。

use serde_json::{json, Value};

/// 拼一整个 HAR：`log.version` 固定 1.2，creator 写本进程版本。
pub fn har(env: &str, records: &[Value]) -> Value {
    let entries: Vec<Value> = records
        .iter()
        .filter_map(entry_of)
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

/// 一条记录 → 一条 entry；缺 `request` 或 `response` 的记录（比如以错误收尾的那条）
/// 直接跳过——HAR 没有「无响应」的表达，硬塞一个空 response 会把下游解析器带偏。
/// 注意 `response` 在记录里是 `null` 而不是缺键，所以要按 `is_null` 判。
fn entry_of(record: &Value) -> Option<Value> {
    let request = record.get("request")?;
    let response = record.get("response").filter(|response| !response.is_null())?;

    // 抓包记录 v2 起 path 与 query 分存（query 不带前导 `?`）；URL 必须拼回查询串，
    // 否则导出丢参数。更老的记录没有 query 字段，按空串处理。
    let query = request.get("query").and_then(Value::as_str).unwrap_or("");
    let url = format!(
        "http://{}{}{}",
        request.get("authority").and_then(Value::as_str).unwrap_or(""),
        request.get("path").and_then(Value::as_str).unwrap_or(""),
        if query.is_empty() {
            String::new()
        } else {
            format!("?{query}")
        },
    );

    Some(json!({
        // HAR 要 ISO-8601；我们记的是毫秒数，原样写成字符串并保留毫秒（v2 才补 ISO 格式）。
        "startedDateTime": record
            .get("time")
            .and_then(Value::as_u64)
            .map(|ms| ms.to_string())
            .unwrap_or_default(),
        "time": 0,
        "_envboard": {
            "session": record.get("session").cloned().unwrap_or(Value::Null),
            "request_id": record.get("request_id").cloned().unwrap_or(Value::Null),
        },
        "request": {
            "method": request.get("method").cloned().unwrap_or(Value::Null),
            "url": url,
            "httpVersion": "HTTP/1.1",
            "headers": headers(request.get("headers")),
            "queryString": [],
            "headersSize": -1,
            "bodySize": body_size(request.get("body")),
            "postData": content(request.get("body").unwrap_or(&Value::Null)),
        },
        "response": {
            "status": response.get("status").cloned().unwrap_or(Value::Null),
            "statusText": "",
            "httpVersion": "HTTP/1.1",
            "headers": headers(response.get("headers")),
            "content": content(response.get("body").unwrap_or(&Value::Null)),
            "headersSize": -1,
            "bodySize": body_size(response.get("body")),
            "redirectURL": "",
        },
        "cache": {},
        "timings": {"send": 0, "wait": -1, "receive": 0},
    }))
}

/// 正文块：`omitted`（超后端上限）只留体积，`content` 照原文给（base64 的也原样带走）。
fn content(body: &Value) -> Value {
    let size = body.get("size").and_then(Value::as_u64).unwrap_or(0);
    if body.get("omitted").is_some() {
        return json!({"size": size, "mimeType": null, "text": null});
    }
    let text = body.get("content").and_then(Value::as_str).unwrap_or("");
    json!({"size": size, "mimeType": null, "text": text})
}

fn body_size(body: Option<&Value>) -> Value {
    body.and_then(|body| body.get("size"))
        .cloned()
        .unwrap_or(json!(-1))
}

/// `[[k, v], …]` → `[{name, value}, …]`；形状不对的整条丢掉，不猜。
fn headers(value: Option<&Value>) -> Value {
    let Some(pairs) = value.and_then(Value::as_array) else {
        return Value::Array(Vec::new());
    };
    Value::Array(
        pairs
            .iter()
            .filter_map(|pair| {
                let items = pair.as_array()?;
                Some(json!({
                    "name": items.first().cloned().unwrap_or(Value::Null),
                    "value": items.get(1).cloned().unwrap_or(Value::Null),
                }))
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: u64, query: &str) -> Value {
        json!({
            "version": 2,
            "session": 7,
            "request_id": id,
            "time": 1_700_000_000_123u64,
            "request": {
                "method": "GET",
                "path": "/v1/orders",
                "query": query,
                "authority": "api.example.com",
                "headers": [["accept", "application/json"]],
                "body": null,
            },
            "response": {
                "status": 200,
                "headers": [["content-type", "application/json"]],
                "body": {"encoding": "utf8", "size": 11, "content": "{\"ok\":true}"},
            },
            "error": null,
        })
    }

    #[test]
    fn url_keeps_query_string() {
        let with_query = har("proxy-a", &[record(1, "page=2")]);
        let url = with_query["log"]["entries"][0]["request"]["url"]
            .as_str()
            .unwrap_or_default();
        assert_eq!(url, "http://api.example.com/v1/orders?page=2");
        // 没有查询串时不留孤零零的 `?`
        let plain = har("proxy-a", &[record(2, "")]);
        assert_eq!(
            plain["log"]["entries"][0]["request"]["url"]
                .as_str()
                .unwrap_or_default(),
            "http://api.example.com/v1/orders"
        );
    }

    #[test]
    fn header_and_meta_shapes() {
        let with_headers = har("proxy-a", &[record(3, "")]);
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
        let omitted = json!({
            "request_id": 4,
            "time": 1u64,
            "request": {"method": "POST", "path": "/x", "authority": "h", "headers": []},
            "response": {"status": 200, "headers": [], "body": {"omitted": true, "size": 300_000}},
            "error": null,
        });
        // 收尾错误的那条没有 response，HAR 表达不了，跳过
        let failed = json!({
            "request_id": 5,
            "time": 2u64,
            "request": {"method": "GET", "path": "/y", "authority": "h", "headers": []},
            "response": null,
            "error": "connection reset",
        });
        let exported = har("proxy-a", &[omitted, failed]);
        assert_eq!(exported["log"]["entries"].as_array().map(Vec::len), Some(1));
        assert_eq!(exported["log"]["entries"][0]["response"]["content"]["size"], 300_000);
        assert_eq!(
            exported["log"]["entries"][0]["response"]["content"]["text"],
            Value::Null
        );
    }
}

//! 导入会话（HAR）：多个并存、只读、进程生命周期内易失。
//!
//! HAR 1.2 是抓包导出的事实标准（mitmproxy 的 savehar 同款形状）；导入时把
//! entries 反向映射成 envboard 的抓包记录形状 —— 调试会话与导入会话共用同一
//! 渲染组件。库有界：最多 [`MAX_SESSIONS`] 个会话、总量 [`MAX_TOTAL_BYTES`]，
//! 超限**拒收**（导入是用户显式动作，拒绝比静默淘汰合适）。

use std::sync::Mutex;

use envboard_engine::{Error, ErrorCode};
use serde_json::{Value, json};

/// 最多同时打开的导入会话数。
pub const MAX_SESSIONS: usize = 8;
/// 导入会话总字节预算（entries 体积估算）。
pub const MAX_TOTAL_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug)]
pub struct ImportedSession {
    pub id: u64,
    pub name: String,
    pub imported_at: u64,
    pub entries: Vec<Value>,
    pub bytes: usize,
}

/// 导入会话库。
#[derive(Default)]
pub struct HarLibrary {
    sessions: Mutex<Vec<ImportedSession>>,
    next_id: Mutex<u64>,
}

impl HarLibrary {
    pub fn next_id(&self) -> u64 {
        let mut next = self.next_id.lock().unwrap();
        *next += 1;
        *next
    }

    pub fn admit(&self, session: ImportedSession) -> Result<Value, Error> {
        let mut sessions = self.sessions.lock().unwrap();
        if sessions.len() >= MAX_SESSIONS {
            return Err(Error::new(
                ErrorCode::Conflict,
                format!(
                    "har library is full ({MAX_SESSIONS} sessions); delete one before importing"
                ),
            ));
        }
        if sessions.iter().map(|s| s.bytes).sum::<usize>() + session.bytes > MAX_TOTAL_BYTES {
            return Err(Error::new(
                ErrorCode::Conflict,
                format!("har library exceeds {MAX_TOTAL_BYTES} bytes; delete a session first"),
            ));
        }
        let view = session_view(&session, session.entries.len());
        sessions.push(session);
        Ok(view)
    }

    pub fn list(&self) -> Vec<Value> {
        let sessions = self.sessions.lock().unwrap();
        sessions
            .iter()
            .map(|session| {
                json!({
                    "id": session.id,
                    "name": session.name,
                    "imported_at": session.imported_at,
                    "entries": session.entries.len(),
                })
            })
            .collect()
    }

    pub fn get(&self, id: u64, limit: usize) -> Option<Value> {
        let sessions = self.sessions.lock().unwrap();
        let session = sessions.iter().find(|s| s.id == id)?;
        Some(session_view(session, limit))
    }

    pub fn delete(&self, id: u64) -> bool {
        let mut sessions = self.sessions.lock().unwrap();
        let before = sessions.len();
        sessions.retain(|session| session.id != id);
        sessions.len() != before
    }
}

fn session_view(session: &ImportedSession, limit: usize) -> Value {
    let start = session.entries.len().saturating_sub(limit);
    json!({
        "id": session.id,
        "name": session.name,
        "imported_at": session.imported_at,
        "entries": session.entries.len(),
        "records": &session.entries[start..],
    })
}

/// HAR 1.2 → envboard 抓包记录形状。畸形输入响亮拒绝（invalid_config 附路径）。
pub fn parse_session(id: u64, name: &str, body: &Value) -> Result<ImportedSession, Error> {
    const PATH: &str = "har";
    let log = body.get("log").and_then(Value::as_object).ok_or_else(|| {
        Error::invalid_config(PATH, "body must be a HAR object with a \"log\" key")
    })?;
    let entries = log
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            Error::invalid_config(format!("{PATH}.log.entries"), "entries must be an array")
        })?;

    let mut records = Vec::with_capacity(entries.len());
    let mut bytes = 0usize;
    for (index, entry) in entries.iter().enumerate() {
        let field = format!("{PATH}.log.entries[{index}]");
        let request = entry
            .get("request")
            .and_then(Value::as_object)
            .ok_or_else(|| Error::invalid_config(&field, "entry.request is required"))?;
        let response = entry.get("response").and_then(Value::as_object);
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("GET")
            .to_string();
        let url = request.get("url").and_then(Value::as_str).unwrap_or("");
        let (authority, path) = split_url(url);
        let mut request_bytes = 0usize;
        let body_shape = |body: Option<&Value>, counted: &mut usize| -> Value {
            let text = body
                .and_then(|b| b.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("");
            *counted += text.len();
            json!({ "encoding": "utf8", "size": text.len(), "content": text })
        };
        let request_body = body_shape(request.get("postData"), &mut request_bytes);
        let (response_shape, response_bytes) = match response {
            Some(response) => {
                let mut counted = 0usize;
                let body = body_shape(response.get("content"), &mut counted);
                let shape = json!({
                    "status": response.get("status").cloned().unwrap_or(json!(0)),
                    "headers": har_headers(response.get("headers")),
                    "body": body,
                });
                (shape, counted)
            }
            None => (
                json!({"status": 0, "headers": [], "body": {"omitted": true, "size": 0}}),
                0,
            ),
        };
        bytes += request_bytes + response_bytes;
        records.push(json!({
            "version": 1,
            "session": id,
            "request_id": index + 1,
            "time": entry.get("startedDateTime").cloned().unwrap_or(Value::Null),
            "request": {
                "method": method,
                "authority": authority,
                "path": path,
                "headers": har_headers(request.get("headers")),
                "body": request_body,
            },
            "response": response_shape,
            "error": Value::Null,
        }));
    }

    Ok(ImportedSession {
        id,
        name: name.to_string(),
        imported_at: 0,
        entries: records,
        bytes,
    })
}

fn har_headers(value: Option<&Value>) -> Value {
    let pairs = value.and_then(Value::as_array);
    match pairs {
        None => Value::Array(vec![]),
        Some(pairs) => Value::Array(
            pairs
                .iter()
                .filter_map(|pair| {
                    let name = pair.get("name")?.as_str()?.to_string();
                    let value = pair.get("value")?.as_str()?.to_string();
                    Some(json!([name, value]))
                })
                .collect(),
        ),
    }
}

/// URL → (authority, path)。导入数据是展示面，解析失败降级为原样 url。
fn split_url(url: &str) -> (String, String) {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    match rest.find('/') {
        Some(at) => (rest[..at].to_string(), rest[at..].to_string()),
        None => (rest.to_string(), "/".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_har() -> Value {
        json!({
            "log": {
                "version": "1.2",
                "entries": [
                    {
                        "startedDateTime": "2026-09-18T00:00:00Z",
                        "request": {
                            "method": "GET",
                            "url": "http://svc.test:8443/x?q=1",
                            "headers": [{"name": "host", "value": "svc.test:8443"}],
                            "postData": {"text": ""}
                        },
                        "response": {
                            "status": 200,
                            "headers": [{"name": "content-type", "value": "text/plain"}],
                            "content": {"text": "ok"}
                        }
                    }
                ]
            }
        })
    }

    #[test]
    fn import_maps_har_entries_to_record_shape() {
        let session = parse_session(1, "sample.har", &sample_har()).unwrap();
        assert_eq!(session.entries.len(), 1);
        let record = &session.entries[0];
        assert_eq!(record["request"]["method"], "GET");
        assert_eq!(record["request"]["authority"], "svc.test:8443");
        assert_eq!(record["request"]["path"], "/x?q=1");
        assert_eq!(record["response"]["status"], 200);
        assert_eq!(
            record["request"]["headers"][0],
            json!(["host", "svc.test:8443"])
        );
    }

    #[test]
    fn malformed_har_is_rejected_loudly() {
        let error = parse_session(1, "bad", &json!({"nope": true})).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidConfig);
    }

    #[test]
    fn library_bounds_sessions() {
        let library = HarLibrary::default();
        for id in 1..=(MAX_SESSIONS as u64) {
            library
                .admit(parse_session(id, &format!("s{id}"), &sample_har()).unwrap())
                .unwrap();
        }
        let error = library
            .admit(parse_session(99, "overflow", &sample_har()).unwrap())
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Conflict);
        assert_eq!(library.list().len(), MAX_SESSIONS);
        assert!(library.delete(1));
        assert!(!library.delete(1));
        assert_eq!(library.list().len(), MAX_SESSIONS - 1);
        let view = library.get(2, 10).unwrap();
        assert_eq!(view["records"].as_array().unwrap().len(), 1);
    }
}

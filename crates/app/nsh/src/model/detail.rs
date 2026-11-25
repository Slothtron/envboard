//! 一条抓包记录的形状（从 typed `Record` 直接读，不经过 JSON）。
//!
//! 解析**宽容**：字段缺了给中性值，不 panic、也不猜——详情面板宁可少一行，
//! 不能因为某条记录的头字段形状怪就整块报错。正文三形状交给 `body::Body`。
//!
//! **S4：一个 `Arc`，不再深拷贝**（方案 §4.1 E）。这一份内容会被信号快照、
//! 页签子树、正文块、复制按钮各取一次；早先每次取都是整段克隆（headers + 两侧正文
//! + 美化文本）。现在 `Detail` 只是 `Arc<Inner>` 的把手：`clone` 是计数 +1，
//! 相等按指针比（同一份实例就是同一条详情），字段走访问器借用。
//!
//! 物化只发生一次：`from_record` 把 `Record` 的 `String` 正文转成 `Arc<str>`（≤ 后端
//! 256 KiB 上限），之后所有持有都是 O(1)。

use std::sync::Arc;

use envboard_protocol::Record;
use envboard_ui::primitives::Tone;

use crate::model::body::Body;
use crate::model::capture::format_bytes;

/// 记录没带协议（旧记录、夹具）时的兜底：envboard 是 HTTPS 中间人，按 https 显示。
const DEFAULT_SCHEME: &str = "https";

/// 详情面板要显示的全部内容（列表的瘦行之外，外加头部与正文）。
///
/// 把手只有 `inner` 一个字段；需要字段时用同名方法（借用，不搬）。
#[derive(Clone, Debug, Default)]
pub struct Detail {
    inner: Arc<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    time_ms: u64,
    method: String,
    authority: String,
    /// 协议（`http` / `https`；空串 = 未知 —— 旧记录与夹具）。
    scheme: String,
    /// 纯路径（不含 query）。
    path: String,
    /// 原始查询串（不含 `?`；空串 = 没有）。
    query: String,
    status: Option<u16>,
    error: Option<String>,
    request_headers: Vec<(String, String)>,
    response_headers: Vec<(String, String)>,
    request_body: Body,
    response_body: Body,
    request_content_type: Option<String>,
    response_content_type: Option<String>,
}

/// 内容相等按**同一份实例**判：`Detail` 只会整份重建（`from_record`），
/// 逐字段比没有意义，还会把大正文再比一遍。
impl PartialEq for Detail {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Detail {
    /// 从记录本体读出一屏详情（headers 与两侧正文都在）。
    ///
    /// 接受 `&Record` 是为了调用点不改形状；内部只做一次搬运，正文转 `Arc<str>`。
    pub fn from_record(record: &Record) -> Self {
        let response = record.response.as_ref();
        let request_headers = record.request.headers.clone();
        let response_headers = response
            .map(|response| response.headers.clone())
            .unwrap_or_default();
        Detail {
            inner: Arc::new(Inner {
                time_ms: record.time,
                method: record.request.method.clone(),
                authority: record.request.authority.clone(),
                scheme: record.request.scheme.clone(),
                path: record.request.path.clone(),
                query: record.request.query.clone(),
                status: response.and_then(|response| response.status),
                error: record.error.as_ref().map(envboard_protocol::RecordError::text),
                request_content_type: content_type_of(&request_headers),
                response_content_type: content_type_of(&response_headers),
                request_body: Body::from_record(record.request.body.as_ref()),
                response_body: Body::from_record(
                    response.and_then(|response| response.body.as_ref()),
                ),
                request_headers,
                response_headers,
            }),
        }
    }

    /// 用列表那一行撑起来的骨架（还没有头部与正文）。骨架与详情同样是 `Detail`，
    /// 面板不用为「两种态」写两套取值路径。
    pub fn skeleton(
        time_ms: u64,
        method: String,
        authority: String,
        scheme: String,
        path: String,
        status: Option<u16>,
    ) -> Self {
        Detail {
            inner: Arc::new(Inner {
                time_ms,
                method,
                authority,
                scheme,
                path,
                status,
                ..Inner::default()
            }),
        }
    }

    pub fn method(&self) -> &str {
        &self.inner.method
    }

    pub fn error(&self) -> Option<&str> {
        self.inner.error.as_deref()
    }

    pub fn request_headers(&self) -> &[(String, String)] {
        &self.inner.request_headers
    }

    pub fn response_headers(&self) -> &[(String, String)] {
        &self.inner.response_headers
    }

    pub fn request_body(&self) -> &Body {
        &self.inner.request_body
    }

    pub fn response_body(&self) -> &Body {
        &self.inner.response_body
    }

    pub fn request_content_type(&self) -> Option<&str> {
        self.inner.request_content_type.as_deref()
    }

    pub fn response_content_type(&self) -> Option<&str> {
        self.inner.response_content_type.as_deref()
    }

    /// 拼回可读的一行：`scheme://authority/path?query`。协议取自记录的 `scheme`
    /// 字段（引擎按流量形态填 http / https）；空值（旧记录、夹具）按 https 兜底。
    pub fn url(&self) -> String {
        let scheme = if self.inner.scheme.is_empty() {
            DEFAULT_SCHEME
        } else {
            self.inner.scheme.as_str()
        };
        let mut out = format!("{scheme}://{}{}", self.inner.authority, self.inner.path);
        if !self.inner.query.is_empty() {
            out.push('?');
            out.push_str(&self.inner.query);
        }
        out
    }

    /// `HH:MM:SS.mmm`（本地时区）。时间为 0（缺字段）就给「—」。
    pub fn time_text(&self) -> String {
        if self.inner.time_ms == 0 {
            return "—".to_owned();
        }
        match chrono::DateTime::from_timestamp_millis(self.inner.time_ms as i64) {
            Some(utc) => {
                let local = utc.with_timezone(&chrono::Local);
                local.format("%H:%M:%S.%3f").to_string()
            }
            None => "—".to_owned(),
        }
    }

    pub fn status_text(&self) -> String {
        if self.inner.error.is_some() {
            return "ERR".to_owned();
        }
        match self.inner.status {
            Some(status) => status.to_string(),
            None => "—".to_owned(),
        }
    }

    pub fn status_tone(&self) -> Tone {
        if self.inner.error.is_some() {
            return Tone::Critical;
        }
        match self.inner.status {
            Some(status) if status >= 500 => Tone::Critical,
            Some(status) if status >= 400 => Tone::Caution,
            _ => Tone::Neutral,
        }
    }

    pub fn request_size_text(&self) -> String {
        format_bytes(self.inner.request_body.size)
    }

    pub fn response_size_text(&self) -> String {
        format_bytes(self.inner.response_body.size)
    }
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
    use envboard_protocol::{Record, RecordBody, RecordRequest, RecordResponse};

    fn record() -> Record {
        Record {
            version: 2,
            session: 7,
            request_id: 42,
            time: 1_700_000_000_123,
            duration_ms: 9,
            request: RecordRequest {
                method: "POST".to_owned(),
                path: "/v1/orders".to_owned(),
                query: "page=2".to_owned(),
                authority: "api.example.com".to_owned(),
                scheme: "https".to_owned(),
                headers: vec![
                    ("Content-Type".to_owned(), "application/json".to_owned()),
                    ("X-Trace".to_owned(), "a".to_owned()),
                ],
                body: Some(RecordBody::Present {
                    encoding: "utf8".to_owned(),
                    size: 12,
                    content: "{\"n\":1,\"m\":2}".to_owned(),
                }),
            },
            response: Some(RecordResponse {
                status: Some(502),
                headers: vec![("content-type".to_owned(), "text/html; charset=utf-8".to_owned())],
                body: Some(RecordBody::Omitted {
                    omitted: true,
                    size: 400_000,
                }),
            }),
            error: None,
        }
    }

    #[test]
    fn parses_record_with_bodies_and_headers() {
        let d = Detail::from_record(&record());
        assert_eq!(d.method(), "POST");
        assert_eq!(d.url(), "https://api.example.com/v1/orders?page=2");
        assert_eq!(d.status_text(), "502");
        assert_eq!(d.status_tone(), Tone::Critical);
        assert_eq!(d.request_headers().len(), 2);
        assert_eq!(d.request_headers()[0].0, "Content-Type");
        // content-type 只留媒体类型；响应头大小写不敏感也能找到
        assert_eq!(d.request_content_type(), Some("application/json"));
        assert_eq!(d.response_content_type(), Some("text/html"));
        // 请求体可美化、响应体 omitted 只能给说明
        assert!(d.request_body().pretty(d.request_content_type()).is_some());
        assert!(d.response_body().note().unwrap().contains("未记录"));
        assert_eq!(d.response_size_text(), "390.6 KB");
    }

    #[test]
    fn missing_fields_stay_neutral() {
        let d = Detail::from_record(&Record {
            request_id: 1,
            ..Record::default()
        });
        assert_eq!(d.status_text(), "—");
        assert!(d.request_headers().is_empty());
        assert_eq!(d.time_text(), "—");
        assert_eq!(d.url(), "https://");
        // error 非 none 时状态列写 ERR，哪怕有状态码
        let d = Detail::from_record(&Record {
            request_id: 2,
            request: RecordRequest {
                method: "GET".to_owned(),
                path: "/x".to_owned(),
                authority: "a".to_owned(),
                ..RecordRequest::default()
            },
            response: Some(RecordResponse {
                status: Some(200),
                ..RecordResponse::default()
            }),
            error: Some(envboard_protocol::RecordError::Text(
                "connection reset".to_owned(),
            )),
            ..Record::default()
        });
        assert_eq!(d.status_text(), "ERR");
        assert_eq!(d.status_tone(), Tone::Critical);
        assert_eq!(d.error(), Some("connection reset"));
    }

    /// S4 的核心断言：克隆是 O(1)，不是又一次整段搬运。
    #[test]
    fn clone_shares_one_instance() {
        let d = Detail::from_record(&record());
        let same = d.clone();
        assert_eq!(d, same, "克隆与本体是同一份实例");
        assert!(std::ptr::eq(
            d.request_headers().as_ptr(),
            same.request_headers().as_ptr()
        ));
        // 重新解析出来的另一份不算相等（信号据此判「变了」）
        assert_ne!(d, Detail::from_record(&record()));
    }

    /// `skeleton` 与详情用同一个形状：面板不必为骨架写第二套取值。
    #[test]
    fn skeleton_carries_row_fields_only() {
        let d = Detail::skeleton(
            1_700_000_000_000,
            "GET".to_owned(),
            "a.example.com".to_owned(),
            "https".to_owned(),
            "/x".to_owned(),
            Some(200),
        );
        assert_eq!(d.url(), "https://a.example.com/x");
        assert_eq!(d.status_text(), "200");
        assert!(d.request_headers().is_empty(), "骨架没有头部");
        assert_eq!(d.request_body().kind, crate::model::body::Kind::Absent);
    }

    /// 明文流量显示 `http://`（真机缺陷：详情 scheme 恒 https）。
    #[test]
    fn plain_traffic_shows_http_scheme() {
        let d = Detail::skeleton(
            1_700_000_000_000,
            "GET".to_owned(),
            "a.example.com".to_owned(),
            "http".to_owned(),
            "/x".to_owned(),
            Some(200),
        );
        assert_eq!(d.url(), "http://a.example.com/x");

        // 空 scheme（旧记录 / 夹具）仍按 https 兜底。
        let legacy = Detail::skeleton(
            1_700_000_000_000,
            "GET".to_owned(),
            "a.example.com".to_owned(),
            String::new(),
            "/x".to_owned(),
            Some(200),
        );
        assert_eq!(legacy.url(), "https://a.example.com/x");
    }
}

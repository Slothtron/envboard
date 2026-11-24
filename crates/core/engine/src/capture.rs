//! 抓包缓冲：请求/响应详情的**易失会话存储**。
//!
//! 语义（契约见 envboard-events crate 文档「抓包会话」）：
//!
//! * 会话 = 实例生命周期：缓冲随实例创建，停止/重启即全部消失；
//! * 开关只控记录：`capture=false` 停止记录新请求，已有记录原样保留；
//!   重新打开续用同一会话——开关来回切不丢任何记录；
//! * 丢弃路径只有两个：手动 `clear()`（generation +1，会话延续）与实例消亡；
//! * 缓冲满即淘最旧（字节预算恒定），淘汰计数可见——内存占用永不超预算。
//!
//! 记录本体是 [`envboard_protocol::Record`]（typed），缓冲里存 `Arc<Record>`：
//! 列表瘦行 / 详情 / HAR 导出拿到的都是同一份对象的句柄，不再按条复制一份
//! `serde_json::Value`。序列化只发生在导出与对外 wire 出口。

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use envboard_protocol::{CaptureRow, Record, RecordBody};

/// 单侧（请求或响应）正文的记录上限；超过则只记头与元数据、body 标 omitted
/// （对齐 mitmproxy「缺失而非截断」）。
pub const CAPTURE_BODY_LIMIT: usize = 256 * 1024;

/// 默认字节预算（每环境）；0 视为取默认。
pub const DEFAULT_CAPTURE_BUDGET: usize = 256 * 1024 * 1024;

/// 单条记录的估算字节数由写入方统计后传入；淘汰按字节预算执行。
pub struct CaptureBuffer {
    session_id: u64,
    started_at: u64,
    budget: usize,
    /// (request_id, 记录本体, 估算字节数)。记录只存一份，读写两侧共享 `Arc`。
    records: Mutex<VecDeque<(u64, Arc<Record>, usize)>>,
    bytes: AtomicU64,
    captured: AtomicU64,
    dropped: AtomicU64,
    generation: AtomicU64,
}

impl CaptureBuffer {
    pub fn new(session_id: u64, started_at: u64, budget: usize) -> Self {
        CaptureBuffer {
            session_id,
            started_at,
            budget,
            records: Mutex::new(VecDeque::new()),
            bytes: AtomicU64::new(0),
            captured: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            generation: AtomicU64::new(1),
        }
    }

    pub fn session_id(&self) -> u64 {
        self.session_id
    }

    pub fn started_at(&self) -> u64 {
        self.started_at
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn captured(&self) -> u64 {
        self.captured.load(Ordering::Acquire)
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Acquire)
    }

    /// 追加一条记录。永不失败：超预算就从最旧淘汰；字节数由调用方给 estimate。
    pub fn push(&self, request_id: u64, record: Arc<Record>, estimate: usize) {
        if self.budget > 0 && estimate > self.budget {
            // 单条超预算：记录进不来，如实计淘汰。
            self.dropped.fetch_add(1, Ordering::AcqRel);
            return;
        }
        let mut records = self.records.lock().unwrap();
        while self.budget > 0
            && self.bytes.load(Ordering::Acquire) + estimate as u64 > self.budget as u64
        {
            match records.pop_front() {
                Some((_, _, evicted)) => {
                    self.bytes.fetch_sub(evicted as u64, Ordering::AcqRel);
                    self.dropped.fetch_add(1, Ordering::AcqRel);
                }
                None => break,
            }
        }
        records.push_back((request_id, record, estimate));
        self.bytes.fetch_add(estimate as u64, Ordering::AcqRel);
        self.captured.fetch_add(1, Ordering::AcqRel);
    }

    /// 手动清空：记录清零、generation +1，会话延续。
    pub fn clear(&self) {
        let mut records = self.records.lock().unwrap();
        records.clear();
        self.bytes.store(0, Ordering::Release);
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// 尾部窗口（limit 条；时间序）。记录里带 session 元数据由调用方组装。
    pub fn tail(&self, limit: usize) -> Vec<Arc<Record>> {
        let records = self.records.lock().unwrap();
        let start = records.len().saturating_sub(limit);
        records
            .iter()
            .skip(start)
            .map(|(_, record, _)| Arc::clone(record))
            .collect()
    }

    /// 游标增量（request_id 严格大于 `after` 的记录，时间序，至多 limit 条）。
    /// 调试实时流的推送原语：游标由读侧（服务端流）自己维护，淘汰只会移除
    /// 游标之前的记录，因此增量永不出现缺口。
    pub fn since(&self, after: u64, limit: usize) -> Vec<Arc<Record>> {
        let records = self.records.lock().unwrap();
        records
            .iter()
            .filter(|(id, _, _)| *id > after)
            .take(limit)
            .map(|(_, record, _)| Arc::clone(record))
            .collect()
    }

    /// 缓冲内最旧的 request_id（空缓冲 = None）。
    pub fn oldest(&self) -> Option<u64> {
        self.records.lock().unwrap().front().map(|(id, _, _)| *id)
    }

    /// 单条详情（按 request_id；后写优先——重启后 id 复位时新记录覆盖旧认知）。
    pub fn get(&self, request_id: u64) -> Option<Arc<Record>> {
        let records = self.records.lock().unwrap();
        records
            .iter()
            .rev()
            .find(|(id, _, _)| *id == request_id)
            .map(|(_, record, _)| Arc::clone(record))
    }

    /// 全量（导出用；时间序）。
    pub fn all(&self) -> Vec<Arc<Record>> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .map(|(_, record, _)| Arc::clone(record))
            .collect()
    }

    /// 列表读原语：游标之后的**瘦行**（只投影表格与过滤要的字段，不 clone body）。
    ///
    /// 与 [`since`](Self::since) 的区别就是内存：`since` 每条整记录进界面，
    /// 35k 行等于把缓冲再复制一份；列表要的只是十来个数与几段短字符串。
    /// 详情仍走 [`get`](Self::get)。
    pub fn rows(&self, after: u64, limit: usize) -> Vec<CaptureRow> {
        let records = self.records.lock().unwrap();
        records
            .iter()
            .filter(|(id, _, _)| *id > after)
            .take(limit)
            .map(|(id, record, _)| {
                let mut row = record.row();
                // 缓冲内元组的 id 是权威（记录里的 `request_id` 与它同值）。
                row.request_id = *id;
                row
            })
            .collect()
    }

}

impl std::fmt::Debug for CaptureBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureBuffer")
            .field("session_id", &self.session_id)
            .field("generation", &self.generation())
            .field("captured", &self.captured())
            .field("dropped", &self.dropped())
            .field("bytes", &self.bytes.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// 把正文编成记录里的正文形状：UTF-8 直存，二进制 base64；超限标 omitted。
/// 与 mitmproxy 的 raw_content 语义对齐：存原始字节，解码延迟到展示端。
pub fn encode_body(body: &[u8]) -> RecordBody {
    if body.len() > CAPTURE_BODY_LIMIT {
        return RecordBody::Omitted {
            omitted: true,
            size: body.len() as u64,
        };
    }
    match std::str::from_utf8(body) {
        Ok(text) => RecordBody::Present {
            encoding: "utf8".to_owned(),
            size: body.len() as u64,
            content: text.to_owned(),
        },
        Err(_) => RecordBody::Present {
            encoding: "base64".to_owned(),
            size: body.len() as u64,
            content: b64(body),
        },
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use envboard_protocol::{RecordBody, RecordRequest, RecordResponse};

    fn buffer(budget: usize) -> CaptureBuffer {
        CaptureBuffer::new(1, 1000, budget)
    }

    /// 最小的可辨识记录：request_id 与 time 跟着 id 走，方便断言顺序。
    fn rec(id: u64) -> Record {
        Record {
            session: 1,
            request_id: id,
            time: 1_700_000_000_000 + id,
            request: RecordRequest {
                method: "GET".to_owned(),
                path: format!("/n/{id}"),
                authority: "h".to_owned(),
                ..RecordRequest::default()
            },
            ..Record::default()
        }
    }

    fn push(buffer: &CaptureBuffer, id: u64, estimate: usize) {
        buffer.push(id, Arc::new(rec(id)), estimate);
    }

    #[test]
    fn budget_evicts_oldest_and_counts() {
        let buffer = buffer(300);
        for id in 1..=5 {
            push(&buffer, id, 100);
        }
        // 预算 300 = 恰好 3 条：第 4、5 条各挤掉一条最旧的。
        assert_eq!(buffer.tail(10).len(), 3);
        let ids: Vec<u64> = buffer
            .tail(10)
            .iter()
            .map(|record| record.request_id)
            .collect();
        assert_eq!(ids, vec![3, 4, 5]);
        assert_eq!(buffer.captured(), 5);
        assert_eq!(buffer.dropped(), 2);
    }

    #[test]
    fn clear_keeps_session_and_bumps_generation() {
        let buffer = buffer(0); // 0 = 无预算
        push(&buffer, 1, 10);
        buffer.clear();
        assert_eq!(buffer.tail(10).len(), 0);
        assert_eq!(buffer.generation(), 2);
        assert_eq!(buffer.captured(), 1, "clear 不动计数——历史统计仍在");
    }

    #[test]
    fn get_prefers_the_latest_for_a_reused_request_id() {
        let buffer = buffer(0);
        buffer.push(
            7,
            Arc::new(rec(7)),
            10,
        );
        let later = Record {
            request: RecordRequest {
                path: "/later".to_owned(),
                ..rec(7).request
            },
            ..rec(7)
        };
        buffer.push(7, Arc::new(later), 10);
        assert_eq!(buffer.get(7).unwrap().request.path, "/later");
    }

    #[test]
    fn oversized_record_is_counted_dropped() {
        let buffer = buffer(100);
        push(&buffer, 1, 200);
        assert_eq!(buffer.tail(10).len(), 0);
        assert_eq!(buffer.dropped(), 1);
    }

    #[test]
    fn since_returns_the_increment_and_survives_eviction() {
        let roomy = buffer(0);
        for id in [3, 5, 8] {
            // request_id 允许有洞（并非每个请求都产生抓包记录）。
            push(&roomy, id, 10);
        }
        assert_eq!(roomy.oldest(), Some(3));
        let delta: Vec<u64> = roomy
            .since(3, 10)
            .iter()
            .map(|record| record.request_id)
            .collect();
        assert_eq!(delta, vec![5, 8], "严格大于游标，时间序");
        assert!(roomy.since(8, 10).is_empty());
        assert_eq!(roomy.since(0, 2).len(), 2, "limit 生效");

        // 淘汰只移除游标之前的记录：增量视角下无缺口可言。
        let tight = buffer(20);
        for id in 1..=5 {
            push(&tight, id, 10);
        }
        assert_eq!(tight.oldest(), Some(4), "预算 20 = 留 2 条");
        assert_eq!(tight.since(3, 10).len(), 2, "游标 3 之后的记录都还在");
    }

    #[test]
    fn bodies_are_utf8_or_base64_or_omitted() {
        let utf8 = encode_body(b"hello");
        assert_eq!(utf8.text(), Some("hello"));
        assert!(!utf8.is_base64());
        let binary = encode_body(&[0xff, 0xfe]);
        assert!(binary.is_base64(), "非 UTF-8 走 base64");
        let omitted = encode_body(&vec![0u8; CAPTURE_BODY_LIMIT + 1]);
        assert!(omitted.is_omitted());
        assert_eq!(omitted.size(), (CAPTURE_BODY_LIMIT + 1) as u64);
        assert_eq!(omitted.text(), None);
        // wire 形状与历史一致（导出与桌面出口靠它）
        assert_eq!(
            serde_json::to_value(&omitted).unwrap(),
            serde_json::json!({"omitted": true, "size": CAPTURE_BODY_LIMIT + 1})
        );
    }

    /// 一条结构完整的记录（字段名照记录契约；正文故意给一大段）。
    fn capture_record(id: u64, status: Option<u16>, error: bool) -> Record {
        let response = status.map(|status| RecordResponse {
            status: Some(status),
            headers: vec![
                (
                    "Content-Type".to_owned(),
                    "application/json; charset=utf-8".to_owned(),
                ),
                ("X-Trace".to_owned(), "abc".to_owned()),
            ],
            body: Some(RecordBody::Present {
                encoding: "utf8".to_owned(),
                size: 4096,
                content: "x".repeat(4096),
            }),
        });
        Record {
            version: 2,
            session: 9,
            request_id: id,
            time: 1_700_000_000_000 + id,
            duration_ms: 3,
            request: RecordRequest {
                method: "POST".to_owned(),
                path: "/v1/orders".to_owned(),
                query: "page=2".to_owned(),
                authority: "api.example.com".to_owned(),
                headers: vec![("Accept".to_owned(), "*/*".to_owned())],
                body: Some(RecordBody::Present {
                    encoding: "utf8".to_owned(),
                    size: 128,
                    content: "y".repeat(128),
                }),
            },
            response,
            error: error.then(|| envboard_protocol::RecordError::Detail(
                envboard_protocol::RecordErrorDetail {
                    kind: "connect".to_owned(),
                    message: None,
                },
            )),
        }
    }

    #[test]
    fn rows_project_fields_without_bodies() {
        let buffer = buffer(0);
        buffer.push(11, Arc::new(capture_record(11, Some(201), false)), 100);
        let rows = buffer.rows(0, usize::MAX);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.request_id, 11);
        assert_eq!(row.method, "POST");
        assert_eq!(row.authority, "api.example.com");
        assert_eq!(row.path, "/v1/orders", "path 不含 query");
        assert_eq!(row.status, Some(201));
        assert!(!row.error);
        assert_eq!(row.request_size, 128);
        assert_eq!(row.response_size, 4096);
        assert_eq!(
            row.content_type.as_deref(),
            Some("application/json; charset=utf-8"),
            "content-type 取原值、大小写不敏感匹配"
        );

        // 失败记录：无响应 → status None、error true、响应大小 0。
        buffer.push(12, Arc::new(capture_record(12, None, true)), 100);
        let failed = &buffer.rows(11, usize::MAX)[0];
        assert_eq!(failed.status, None);
        assert!(failed.error);
        assert_eq!(failed.response_size, 0);
        assert_eq!(failed.content_type, None);
    }

    #[test]
    fn rows_respect_cursor_and_limit_like_since() {
        let buffer = buffer(0);
        for id in [3, 5, 8] {
            buffer.push(id, Arc::new(capture_record(id, Some(200), false)), 100);
        }
        let ids: Vec<u64> = buffer.rows(3, 10).iter().map(|r| r.request_id).collect();
        assert_eq!(ids, vec![5, 8], "严格大于游标，时间序");
        assert_eq!(buffer.rows(0, 2).len(), 2, "limit 生效");
        assert!(buffer.rows(8, 10).is_empty());
        assert_eq!(
            buffer.rows(0, usize::MAX).len(),
            buffer.since(0, usize::MAX).len(),
            "行数与整记录读一致"
        );
    }

    /// 缓冲只持有 `Arc`：读出来的与存进去的是同一个对象（不复制记录体）。
    #[test]
    fn reads_share_the_stored_record() {
        let buffer = buffer(0);
        let stored = Arc::new(rec(4));
        buffer.push(4, Arc::clone(&stored), 10);
        let got = buffer.get(4).expect("刚存的记录");
        assert!(Arc::ptr_eq(&got, &stored));
        let tailed = buffer.tail(1).pop().expect("尾部窗口");
        assert!(Arc::ptr_eq(&tailed, &stored));
    }
}

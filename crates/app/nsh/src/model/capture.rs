//! 抓包列表的视图语义：状态分段、资源类型分组、计数与过滤。
//!
//! ADR-8（EB-DESIGN-004）定的是「列即筛选」：筛选项来自纸面上的列（状态列、
//! 类型列），不另立一排 15 个 chip。所以判据也按列来算——这一层只回答
//! 「这一行属于哪一段/哪一组」，颜色与布局在 `pages`。
//!
//! 过滤**全在界面侧**：行已经是瘦投影（几十到一百多字节），35k 行一趟纯内存
//! 谓词是毫秒级；放到引擎里做会拉长缓冲锁的持有时间，拖慢写入。

use envboard_protocol::CaptureRow;
use envboard_ui::primitives::Tone;

// ---- 状态分段 --------------------------------------------------------------

/// 状态列的六段（对齐桌面版的 6 个过滤 chip）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum StatusFilter {
    #[default]
    All,
    /// 异常：以错误结束，或 ≥400（对齐桌面版 `err` 的判据）。
    Error,
    S2,
    S3,
    S4,
    S5,
}

impl StatusFilter {
    /// 面板里的展示顺序：全部→异常→2xx…5xx。
    pub const ALL: [StatusFilter; 6] = [
        StatusFilter::All,
        StatusFilter::Error,
        StatusFilter::S2,
        StatusFilter::S3,
        StatusFilter::S4,
        StatusFilter::S5,
    ];

    pub fn label(self) -> &'static str {
        match self {
            StatusFilter::All => "全部",
            StatusFilter::Error => "异常",
            StatusFilter::S2 => "2xx",
            StatusFilter::S3 => "3xx",
            StatusFilter::S4 => "4xx",
            StatusFilter::S5 => "5xx",
        }
    }

    /// 序号：计数数组的下标与它一一对应（少一处手写映射，就少一处漂移）。
    pub fn index(self) -> usize {
        match self {
            StatusFilter::All => 0,
            StatusFilter::Error => 1,
            StatusFilter::S2 => 2,
            StatusFilter::S3 => 3,
            StatusFilter::S4 => 4,
            StatusFilter::S5 => 5,
        }
    }

    /// 判据只有这一处：面板计数与行过滤共用它，所以数字与列表永远一致。
    ///
    /// `error` 短路排在前面：这条记录在状态列写的是 `ERR`，就不能再被 2xx 段收走，
    /// 否则「点 2xx 看到一行写着 ERR」。
    pub fn matches(self, row: &CaptureRow) -> bool {
        if row.error {
            return matches!(self, StatusFilter::All | StatusFilter::Error);
        }
        let status = row.status;
        match self {
            StatusFilter::All => true,
            StatusFilter::Error => status.is_some_and(|s| s >= 400),
            StatusFilter::S2 => status.is_some_and(|s| (200..300).contains(&s)),
            StatusFilter::S3 => status.is_some_and(|s| (300..400).contains(&s)),
            StatusFilter::S4 => status.is_some_and(|s| (400..500).contains(&s)),
            StatusFilter::S5 => status.is_some_and(|s| s >= 500),
        }
    }
}

// ---- 资源类型分组 ----------------------------------------------------------

/// 类型列的九组（对齐桌面版的 9 个过滤 chip）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ResFilter {
    #[default]
    All,
    /// Fetch/XHR：JSON / XML / 纯文本这类「接口数据」，也是 SPA 的主路径。
    Fetch,
    Doc,
    Css,
    Js,
    Img,
    Media,
    Font,
    Other,
}

impl ResFilter {
    pub const ALL: [ResFilter; 9] = [
        ResFilter::All,
        ResFilter::Fetch,
        ResFilter::Doc,
        ResFilter::Css,
        ResFilter::Js,
        ResFilter::Img,
        ResFilter::Media,
        ResFilter::Font,
        ResFilter::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ResFilter::All => "全部类型",
            ResFilter::Fetch => "Fetch/XHR",
            ResFilter::Doc => "文档",
            ResFilter::Css => "CSS",
            ResFilter::Js => "JS",
            ResFilter::Img => "图片",
            ResFilter::Media => "媒体",
            ResFilter::Font => "字体",
            ResFilter::Other => "其他",
        }
    }

    pub fn index(self) -> usize {
        match self {
            ResFilter::All => 0,
            ResFilter::Fetch => 1,
            ResFilter::Doc => 2,
            ResFilter::Css => 3,
            ResFilter::Js => 4,
            ResFilter::Img => 5,
            ResFilter::Media => 6,
            ResFilter::Font => 7,
            ResFilter::Other => 8,
        }
    }

    pub fn matches(self, row: &CaptureRow) -> bool {
        self == ResFilter::All || resource_of(row) == self
    }
}

/// 资源类型判定：先看响应 content-type，缺了再看路径扩展名（与桌面版同口径）。
/// 抓包记录没有浏览器的 initiator type，Fetch/XHR 只能按「接口数据」近似。
pub fn resource_of(row: &CaptureRow) -> ResFilter {
    let ct = row.content_type.as_deref().unwrap_or("").to_ascii_lowercase();
    if ct.starts_with("text/html") || ct == "application/xhtml+xml" {
        return ResFilter::Doc;
    }
    if ct.contains("json") || ct.contains("xml") || ct == "text/plain" {
        return ResFilter::Fetch;
    }
    if ct.contains("javascript") || ct.contains("ecmascript") {
        return ResFilter::Js;
    }
    if ct.contains("text/css") {
        return ResFilter::Css;
    }
    if ct.starts_with("image/") {
        return ResFilter::Img;
    }
    if ct.starts_with("audio/") || ct.starts_with("video/") {
        return ResFilter::Media;
    }
    if ct.contains("font") || ct.contains("woff") {
        return ResFilter::Font;
    }
    // content-type 缺失或认不出：按扩展名兜底（如实心 favicon / 脚本 / 样式）。
    let path = row.path.to_ascii_lowercase();
    let ends = |suffixes: &[&str]| suffixes.iter().any(|s| path.ends_with(s));
    if ends(&[".css"]) {
        ResFilter::Css
    } else if ends(&[".js", ".mjs", ".cjs", ".jsx", ".ts"]) {
        ResFilter::Js
    } else if ends(&[".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".ico", ".bmp", ".avif"]) {
        ResFilter::Img
    } else if ends(&[".mp3", ".mp4", ".webm", ".ogg", ".wav", ".flac", ".m4a"]) {
        ResFilter::Media
    } else if ends(&[".woff", ".woff2", ".ttf", ".otf", ".eot"]) {
        ResFilter::Font
    } else if ends(&[".html", ".htm", ".xhtml"]) {
        ResFilter::Doc
    } else if ends(&[".json", ".xml"]) {
        ResFilter::Fetch
    } else {
        ResFilter::Other
    }
}

// ---- 合并（读侧止血） -------------------------------------------------------

/// 把一拍**尾部窗口**并入本地表：按 `request_id` 归并去重（同 id 取这一拍的那份）、
/// 丢掉已被引擎淘汰的头、超上限砍头。
///
/// 为什么必须按 id 归并而不是直接 append：记录是请求跑完才入队的，入队顺序 ≠ id 顺序。
/// 旧实现拿「最后一行 id」当游标，先完成的大 id 把游标顶高，后完成的小 id 就永远
/// 进不来（真机四并发只显示了一条）。改成尾窗口 + 按 id 归并后，顺序由 id 决定，
/// 迟到的记录会插到它该在的位置。
///
/// 两段都已是 id 升序，所以是一次线性归并：`O(表内行数 + 这一拍行数)`。
pub fn merge_rows(
    rows: &mut Vec<CaptureRow>,
    incoming: Vec<CaptureRow>,
    oldest: Option<u64>,
    limit: usize,
) {
    let cutoff = oldest.unwrap_or(0);
    // 引擎里已经没有的（比 oldest 更老）不该继续挂在表上。
    let kept = std::mem::take(rows);
    let mut left = kept.into_iter().filter(|row| row.request_id >= cutoff).peekable();
    let mut right = incoming
        .into_iter()
        .filter(|row| row.request_id >= cutoff)
        .peekable();
    let mut out: Vec<CaptureRow> = Vec::new();
    loop {
        match (left.peek(), right.peek()) {
            (Some(l), Some(r)) => match l.request_id.cmp(&r.request_id) {
                std::cmp::Ordering::Less => out.push(left.next().expect("peeked")),
                std::cmp::Ordering::Greater => out.push(right.next().expect("peeked")),
                std::cmp::Ordering::Equal => {
                    // 同一条被读了两遍：以这一拍为准（它是引擎缓冲里的当前值）。
                    left.next();
                    out.push(right.next().expect("peeked"));
                }
            },
            (Some(_), None) => out.push(left.next().expect("peeked")),
            (None, Some(_)) => out.push(right.next().expect("peeked")),
            (None, None) => break,
        }
    }
    if out.len() > limit {
        let drop_to = out.len() - limit;
        out.drain(..drop_to);
    }
    *rows = out;
}

// ---- 计数与过滤 ------------------------------------------------------------

/// 两维的计数。判据与过滤同源（`StatusFilter::matches` / `resource_of`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    status: [usize; 6],
    res: [usize; 9],
}

impl Counts {
    pub fn status(&self, filter: StatusFilter) -> usize {
        self.status[filter.index()]
    }

    pub fn res(&self, filter: ResFilter) -> usize {
        self.res[filter.index()]
    }

    /// 全部（两维各自的总数相同）；列表空态与单测用它，所以允许“页面没直接调”。
    #[allow(dead_code)]
    pub fn total(&self) -> usize {
        self.status[StatusFilter::All.index()]
    }
}

pub fn counts(rows: &[CaptureRow]) -> Counts {
    let mut counts = Counts::default();
    counts.status[StatusFilter::All.index()] = rows.len();
    counts.res[ResFilter::All.index()] = rows.len();
    for row in rows {
        for status in StatusFilter::ALL {
            if status != StatusFilter::All && status.matches(row) {
                counts.status[status.index()] += 1;
            }
        }
        counts.res[resource_of(row).index()] += 1;
    }
    counts
}

/// 过滤结果给**下标**而不是克隆行：一行的字符串是拥有的，35k 行克隆一次就是几 MB，
/// 换一次筛选就白拷一遍。虚拟列表拿下标回原表取行即可。
pub fn filtered_indexes(
    rows: &[CaptureRow],
    status: StatusFilter,
    res: ResFilter,
) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, row)| status.matches(row) && res.matches(row))
        .map(|(index, _)| index)
        .collect()
}

// ---- 单元格文案 ------------------------------------------------------------

/// 方法列：记录缺方法时给「—」，不留空白。
pub fn method_text(row: &CaptureRow) -> &str {
    if row.method.is_empty() { "—" } else { &row.method }
}

/// 状态列：错误记 `ERR`；还没响应给「—」；其余给状态码。
pub fn status_text(row: &CaptureRow) -> String {
    if row.error {
        return "ERR".to_owned();
    }
    match row.status {
        Some(status) => status.to_string(),
        None => "—".to_owned(),
    }
}

/// 状态列的语气：5xx/错误实红、4xx 警示、其余（2xx/3xx）正文墨。
/// 3xx 不弱化成三级墨——列表里它和 2xx 同样要能扫出来。
pub fn status_tone(row: &CaptureRow) -> Tone {
    if row.error {
        return Tone::Critical;
    }
    match row.status {
        Some(status) if status >= 500 => Tone::Critical,
        Some(status) if status >= 400 => Tone::Caution,
        _ => Tone::Neutral,
    }
}

/// 大小列：请求体 + 响应体（桌面版同口径）。
pub fn size_text(row: &CaptureRow) -> String {
    format_bytes(row.request_size.saturating_add(row.response_size))
}

/// 字节数的展示口径（与桌面版 `formatBytes` 一致）：0 → `0 B`，<1024 → `N B`，
/// 再往上各留一位小数。
pub fn format_bytes(size: u64) -> String {
    if size == 0 {
        "0 B".to_owned()
    } else if size < 1024 {
        format!("{size} B")
    } else if size < 1024 * 1024 {
        format!("{:.1} KB", size as f64 / 1024.0)
    } else {
        format!("{:.1} MB", size as f64 / 1024.0 / 1024.0)
    }
}

// ---- fixture（golden 与截图口径） -----------------------------------------

/// 一屏抓包样张：2xx/3xx/4xx/5xx、错误、无响应、各种类型都在场，
/// golden 才能把状态颜色与类型分组一次拍全。
pub fn fixture_rows(n: usize) -> Vec<CaptureRow> {
    let base = [
        (200u16, "/v1/orders", "application/json", 0u64, 512u64, false),
        (201, "/v1/orders", "application/json", 128, 640, false),
        (301, "/old", "text/html", 0, 0, false),
        (404, "/v1/users/9", "application/json", 0, 188, false),
        (500, "/v1/checkout", "text/plain", 32, 96, false),
        (204, "/assets/app.css", "text/css", 0, 0, false),
        (200, "/assets/app.js", "application/javascript", 0, 214_048, false),
        (200, "/assets/logo.png", "image/png", 0, 8_192, false),
        (200, "/fonts/inter.woff2", "font/woff2", 0, 65_536, false),
        (200, "/media/loop.mp4", "video/mp4", 0, 3_145_728, false),
    ];
    let mut rows = Vec::with_capacity(n);
    for index in 0..n {
        let (status, path, ct, req, resp, error) = base[index % base.len()];
        rows.push(CaptureRow {
            request_id: index as u64 + 1,
            time: 1_700_000_000_000 + index as u64 * 17,
            method: if index % 4 == 0 { "GET" } else { "POST" }.to_owned(),
            authority: if index % 3 == 0 {
                "api.staging.local".to_owned()
            } else {
                "api.example.com".to_owned()
            },
            scheme: if index % 5 == 0 {
                "http".to_owned()
            } else {
                "https".to_owned()
            },
            path: path.to_owned(),
            status: if error { None } else { Some(status) },
            error,
            request_size: req,
            response_size: resp,
            content_type: Some(ct.to_owned()),
        });
    }
    // 一条无响应的失败记录（连接失败）：状态列要显示 ERR。
    if n >= 1 {
        let last = rows.len() - 1;
        rows[last].status = None;
        rows[last].error = true;
        rows[last].path = "/v1/health".to_owned();
        rows[last].content_type = None;
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: Option<u16>, error: bool) -> CaptureRow {
        CaptureRow {
            request_id: 1,
            time: 0,
            method: "GET".to_owned(),
            authority: "example.com".to_owned(),
            scheme: "https".to_owned(),
            path: "/x".to_owned(),
            status,
            error,
            request_size: 0,
            response_size: 0,
            content_type: None,
        }
    }

    #[test]
    fn status_segments_have_exclusive_boundaries() {
        assert!(StatusFilter::S2.matches(&row(Some(200), false)));
        assert!(!StatusFilter::S2.matches(&row(Some(204), true)), "错误记录不算 2xx");
        assert!(StatusFilter::S3.matches(&row(Some(301), false)));
        assert!(StatusFilter::S4.matches(&row(Some(404), false)));
        assert!(StatusFilter::S5.matches(&row(Some(503), false)));
        // 边界：399 是 3xx、400 是 4xx、500 是 5xx
        assert!(!StatusFilter::S4.matches(&row(Some(399), false)));
        assert!(StatusFilter::S4.matches(&row(Some(400), false)));
        assert!(StatusFilter::S5.matches(&row(Some(500), false)));
        // 异常：≥400 或 error 标记（含无响应的连接失败）
        assert!(StatusFilter::Error.matches(&row(Some(404), false)));
        assert!(StatusFilter::Error.matches(&row(None, true)));
        assert!(!StatusFilter::Error.matches(&row(Some(200), false)));
        // 无响应的“进行中”记录只落在「全部」
        assert!(!StatusFilter::S2.matches(&row(None, false)));
        assert!(StatusFilter::All.matches(&row(None, false)));
    }

    #[test]
    fn resource_type_prefers_content_type_then_extension() {
        let mut r = row(Some(200), false);
        r.content_type = Some("application/json; charset=utf-8".to_owned());
        assert_eq!(resource_of(&r), ResFilter::Fetch);
        r.content_type = Some("text/css".to_owned());
        assert_eq!(resource_of(&r), ResFilter::Css);
        r.content_type = None;
        r.path = "/static/logo.svg".to_owned();
        assert_eq!(resource_of(&r), ResFilter::Img);
        r.path = "/static/font.woff2".to_owned();
        assert_eq!(resource_of(&r), ResFilter::Font);
        r.path = "/blob".to_owned();
        assert_eq!(resource_of(&r), ResFilter::Other);
    }

    #[test]
    fn counts_and_filter_agree_on_fixture() {
        let rows = fixture_rows(10);
        let c = counts(&rows);
        assert_eq!(c.total(), 10);
        // 每段计数都等于该段过滤出来的行数（同一判据的两处用法）
        for status in StatusFilter::ALL {
            let n = filtered_indexes(&rows, status, ResFilter::All).len();
            assert_eq!(c.status(status), n, "状态段 {} 计数与过滤不一致", status.label());
        }
        for res in ResFilter::ALL {
            let n = filtered_indexes(&rows, StatusFilter::All, res).len();
            assert_eq!(c.res(res), n, "类型组 {} 计数与过滤不一致", res.label());
        }
        // 两维 AND：2xx × Fetch 只剩 JSON 那两条
        let both = filtered_indexes(&rows, StatusFilter::S2, ResFilter::Fetch);
        assert_eq!(both.len(), 2);
    }

    #[test]
    fn cell_texts_say_the_truth() {
        assert_eq!(status_text(&row(None, true)), "ERR");
        assert_eq!(status_text(&row(None, false)), "—");
        assert_eq!(status_text(&row(Some(304), false)), "304");
        assert_eq!(status_tone(&row(Some(500), false)), Tone::Critical);
        assert_eq!(status_tone(&row(Some(404), false)), Tone::Caution);
        assert_eq!(status_tone(&row(Some(200), false)), Tone::Neutral);
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(3 * 1024 * 1024), "3.0 MB");
    }

    // ---- 归并（S2 的读侧规则） --------------------------------------------

    fn row_at(id: u64, path: &str) -> CaptureRow {
        CaptureRow {
            request_id: id,
            path: path.to_owned(),
            ..row(Some(200), false)
        }
    }

    fn ids(rows: &[CaptureRow]) -> Vec<u64> {
        rows.iter().map(|row| row.request_id).collect()
    }

    /// 迟到的小 id 必须插回它该在的位置，而不是被丢弃或追加到末尾。
    #[test]
    fn merge_inserts_late_small_ids_in_id_order() {
        let mut rows = vec![row_at(21, "/a"), row_at(23, "/c")];
        // 这一拍：22 是刚完成的（比 21 晚跑完、id 更大）；20 是慢请求迟到。
        merge_rows(&mut rows, vec![row_at(20, "/slow"), row_at(22, "/b")], None, 4000);
        assert_eq!(ids(&rows), vec![20, 21, 22, 23], "按 id 升序，五条一条不少");
    }

    /// 同一条被两拍都读到：保留这一拍的那份（表格内容随记录更新而更新）。
    #[test]
    fn merge_dedupes_the_same_request_id() {
        let mut rows = vec![row_at(7, "/old-path")];
        merge_rows(&mut rows, vec![row_at(7, "/new-path")], None, 4000);
        assert_eq!(rows.len(), 1, "同 id 不重复");
        assert_eq!(rows[0].path, "/new-path");
    }

    /// 引擎按字节淘汰了老记录：本地表也不该继续显示（否则点开是「记录已不在缓冲」）。
    #[test]
    fn merge_drops_rows_older_than_the_buffer_window() {
        let mut rows = vec![row_at(1, "/gone"), row_at(5, "/kept"), row_at(9, "/kept")];
        merge_rows(&mut rows, vec![row_at(9, "/kept"), row_at(12, "/new")], Some(5), 4000);
        assert_eq!(ids(&rows), vec![5, 9, 12], "比 oldest(5) 老的 1 丢掉");
        // 一整拍都比 oldest 老（缓冲刚被清空重填）：表也该空
        let mut rows = vec![row_at(1, "/gone")];
        merge_rows(&mut rows, vec![row_at(2, "/gone")], Some(10), 4000);
        assert!(rows.is_empty());
    }

    /// 表有上限：只留 id 最大的那一段，砍头不砍尾。
    #[test]
    fn merge_trims_the_head_to_the_row_limit() {
        let mut rows = vec![row_at(1, "/a"), row_at(2, "/b")];
        merge_rows(
            &mut rows,
            vec![row_at(3, "/c"), row_at(4, "/d"), row_at(5, "/e")],
            None,
            3,
        );
        assert_eq!(ids(&rows), vec![3, 4, 5], "上限 3 → 留最新三条");
    }
}

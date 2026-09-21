//! HTTP 面：静态资产 + REST API + SSE。
//!
//! 资产是 `frontend/`（Vite 工程）的构建产物，编译期 `include_dir!` 内嵌：
//! 运行时零 Node、零 CDN。**前端资产必须保持外置文件形态**（dist 的脚本与样式
//! 都走 `<link>` / `<script src>`）—— 内联注入会被我们自己下发的严格 CSP 拒绝，
//! 而那种故障在 curl 断言里看不出来（v1 踩过）。
//!
//! 推送流的帧形状由 `envboard-protocol` 统一供给（`spec/protocol.md` 是语言中立
//! 契约）：三条 SSE 流的每帧都带显式 `cursor`，帧名统一为
//! `snapshot / baseline / events / error`。

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use envboard_admin::AdminService;
use envboard_engine::{Error, ErrorCode};
use envboard_protocol::{
    DebugEvents, DebugSnapshot, ErrorBody, ErrorFrame, SnapshotFrame, TrajectoryWindow,
};
use futures_util::stream::Stream;
use include_dir::{Dir, include_dir};
use serde_json::{Value, json};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::{IntervalStream, ReceiverStream};

use crate::config::{CONTENT_SECURITY_POLICY, REQUEST_HEADER, TOKEN_HEADER, WebConfig};

/// 前端构建产物（内嵌源，提交入库；src↔dist 成对判定由 toolchain 门禁、
/// drift 校验由 `ci/verify.sh` 的 frontend 层负责）。
const DIST: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../frontend/dist");

#[derive(Clone)]
pub struct AppState {
    pub admin: Arc<AdminService>,
    pub config: WebConfig,
    /// 设置页「证书信息」的只读摘要（组合根从共享 CA 读出后转成 JSON）。
    pub ca: Option<Value>,
    /// 根证书 PEM 字节（下载端点的载荷）。
    pub ca_pem: Arc<Vec<u8>>,
}

/// 组合根交进来的共享 CA 资产：只有证书的公开面，私钥永不经过 web 层。
#[derive(Clone)]
pub struct CaAssets {
    pub info: Option<Value>,
    pub pem: Arc<Vec<u8>>,
}

/// 起 HTTP 服务并常驻（只做 HTTP 绑定与横幅；reconcile 与日志照看是宿主
/// 组合根的编排职责，在 `envboard-server` 里）。
pub async fn serve(admin: Arc<AdminService>, config: WebConfig, ca: CaAssets) -> Result<(), Error> {
    let state = AppState {
        admin,
        config: config.clone(),
        ca: ca.info,
        ca_pem: ca.pem,
    };
    let app = router(state);

    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|error| {
            Error::invalid_config(
                "web.listen",
                format!("cannot bind {}: {error}", config.listen),
            )
        })?;
    println!("envboard workbench: http://{}", config.listen);
    // 配了 token 就把可点击的完整地址打出来：浏览器无法携带自定义头，直接打开
    // 只能靠 `?token=`。监听通配地址（如 0.0.0.0）时用 127.0.0.1 展示 —— 通配地址
    // 本身不可点击，token 值不受影响。
    if let Some(token) = config.token.as_deref() {
        let display_host = if config.listen.ip().is_unspecified() {
            "127.0.0.1"
        } else {
            &config.listen.ip().to_string()
        };
        println!(
            "  dashboard: http://{display_host}:{}/?token={token}",
            config.listen.port()
        );
    } else {
        // 免鉴权只可能来自回环监听（非回环在 WebConfig::parse 就被拒了）——
        // 横幅如实说明当前档位，别让运维猜。
        println!("  auth: disabled (loopback bind; pass --token to enable)");
    }
    println!("  (assets are embedded; CSP has no 'unsafe-inline' — see spec/ and src/config.rs)");

    axum::serve(listener, app)
        .await
        .map_err(|error| Error::internal_error(format!("web server failed: {error}")))
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/assets/*file", get(serve_asset))
        .route("/api/status", get(api_status))
        // 设置页「证书」：只读摘要 + 下载 + 二维码（全部走统一门禁，token 档照常 401）。
        .route("/api/ca", get(api_ca))
        .route("/api/ca.pem", get(api_ca_pem))
        .route("/api/ca/qrcode.svg", get(api_ca_qrcode))
        .route("/api/environments", get(api_list).post(api_create))
        .route(
            "/api/environments/:name",
            get(api_get).patch(api_update).delete(api_remove),
        )
        .route("/api/environments/:name/start", post(api_start))
        .route("/api/environments/:name/stop", post(api_stop))
        .route("/api/environments/:name/restart", post(api_restart))
        .route("/api/environments/:name/reallocate", post(api_reallocate))
        .route("/api/environments/:name/logs", get(api_logs))
        .route("/api/environments/:name/trajectory", get(api_trajectory))
        .route("/api/environments/:name/captures", get(api_captures))
        .route(
            "/api/environments/:name/captures/export",
            get(api_captures_export),
        )
        .route(
            "/api/environments/:name/captures/:request_id",
            get(api_capture),
        )
        .route("/api/debug", get(api_debug).post(api_debug_start))
        .route("/api/debug/stop", post(api_debug_stop))
        .route("/api/debug/stream", get(api_debug_stream))
        .route("/api/har", get(api_har_list).post(api_har_import))
        .route("/api/har/:id", get(api_har_get).delete(api_har_delete))
        .route(
            "/api/environments/:name/capture/clear",
            post(api_capture_clear),
        )
        .route(
            "/api/environments/:name/trajectory/stream",
            get(api_trajectory_stream),
        )
        .route("/api/rules", get(api_rules_list).post(api_rules_import))
        .route("/api/proxies", get(api_proxy_list).post(api_proxy_put))
        .route(
            "/api/proxies/:name",
            get(api_proxy_get).delete(api_proxy_delete),
        )
        // 故障注入旋钮（live 断言组 9 的面）：核心不支持注入时如实 400。
        .route("/api/_fault", post(api_fault))
        .route(
            "/api/rules/:name",
            get(api_rules_read).delete(api_rules_delete),
        )
        .route("/api/compare", get(api_compare))
        .route("/api/history", get(api_history))
        .route("/api/events", get(api_events))
        // API 的 404 也要是 JSON：前端按 {error:{code,message}} 解析，
        // 让它面对 axum 的纯文本 404 只能报"响应不是 JSON"，排查体验很差。
        .fallback(api_not_found)
        .layer(axum::middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

// --------------------------------------------------------------------------- #
// 安全中间件：Host 校验 + token + 变更类路由的自定义头
// --------------------------------------------------------------------------- #

async fn guard(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let headers = request.headers();

    // ① Host 头必须是配置的监听地址（防 DNS rebinding）
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !state
        .config
        .allowed_hosts()
        .iter()
        .any(|allowed| allowed == host)
    {
        return failure(
            StatusCode::FORBIDDEN,
            ErrorCode::InvalidConfig,
            format!(
                "Host header {host:?} is not one of the configured listen addresses {:?} \
                 (DNS rebinding protection)",
                state.config.allowed_hosts()
            ),
        );
    }

    // ② 配了 token 就必须带对：header 优先，未带时接受 `?token=`（SSE 的
    // EventSource 带不了自定义头，浏览器直接打开工作台也只能靠 URL 携带）。
    //
    // **静态资产豁免**：`/assets/*` 是编译期内嵌的构建产物（hashed JS/CSS），
    // 不含任何数据（数据只从 API 出）。浏览器解析 `<link>`/`<script>` 时带不了
    // header、也不会把页面 URL 上的 `?token=` 复制到子资源请求上 —— 豁免它们，
    // 否则开了 token 工作台必然白屏。API 与页面本体（`/`）不豁免。
    let path = request.uri().path();
    let public_asset = matches!(
        *request.method(),
        axum::http::Method::GET | axum::http::Method::HEAD
    ) && path.starts_with("/assets/");
    if let Some(expected) = state.config.token.as_deref()
        && !public_asset
    {
        let provided = headers
            .get(TOKEN_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(std::borrow::Cow::Borrowed)
            .or_else(|| query_token(request.uri().query()));
        let matched = provided.is_some_and(|provided| constant_time_eq(&provided, expected));
        if !matched {
            return failure(
                StatusCode::UNAUTHORIZED,
                ErrorCode::InvalidConfig,
                format!("missing or wrong {TOKEN_HEADER} header or ?token= parameter"),
            );
        }
    }

    // ③ 变更类路由必须带自定义头 —— 跨站简单请求带不了它，这就挡住了 CSRF
    let mutating = !matches!(
        *request.method(),
        axum::http::Method::GET | axum::http::Method::HEAD
    );
    if mutating {
        let present = headers
            .get(REQUEST_HEADER)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == "1");
        if !present {
            return failure(
                StatusCode::FORBIDDEN,
                ErrorCode::InvalidConfig,
                format!(
                    "mutating requests must carry `{REQUEST_HEADER}: 1` — a cross-site form or \
                     simple request cannot set custom headers, which is what blocks CSRF here"
                ),
            );
        }
    }

    next.run(request).await
}

/// 从 URL query 里取 `token` 参数（只认裸键，无 URL 编码解析 —— token 由我们生成，
/// 是 URL 安全字符集；带编码的值按字面比较自然不匹配，宁可拒绝）。
fn query_token(query: Option<&str>) -> Option<std::borrow::Cow<'_, str>> {
    let query = query?;
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        if parts.next() == Some("token") {
            return parts.next().map(std::borrow::Cow::Borrowed);
        }
    }
    None
}

/// 常量时间比较：长度不同立即失败（长度本就不是秘密），等长时逐字节 XOR 累加，
/// 不因首个不同字节提前返回 —— 避免逐字节计时侧信道。
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

async fn api_not_found(uri: axum::http::Uri) -> Response {
    failure(
        StatusCode::NOT_FOUND,
        ErrorCode::NotFound,
        format!(
            "no route for {} (see /api/status for the API surface)",
            uri.path()
        ),
    )
}

fn failure(status: StatusCode, code: ErrorCode, message: String) -> Response {
    (
        status,
        Json(json!({"error": {"code": code.as_str(), "message": message}})),
    )
        .into_response()
}

fn error_response(error: Error) -> Response {
    let status =
        StatusCode::from_u16(error.code.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = ErrorBody {
        code: error.code.as_str().to_string(),
        message: error.message,
        field: error.field,
    };
    (status, Json(json!({"error": body}))).into_response()
}

// --------------------------------------------------------------------------- #
// 静态资产
// --------------------------------------------------------------------------- #

fn asset(body: &'static [u8], content_type: &'static str, immutable: bool) -> Response {
    let mut response = (StatusCode::OK, body).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    if immutable {
        // hashed 文件名 = 内容即版本：可永久缓存（重新构建即换名，无失效问题）。
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    }
    response
}

/// 入口页：不缓存（它引用 hashed 资产，自身必须常新）。
async fn index() -> Response {
    match DIST.get_file("index.html") {
        Some(file) => {
            let mut response = asset(file.contents(), "text/html; charset=utf-8", false);
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        // 编译期内嵌：缺文件 = 构建被破坏，响亮失败。
        None => failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::InternalError,
            "embedded frontend dist is missing index.html".to_string(),
        ),
    }
}

/// hashed 资产（`/assets/<name>`）：文件名带内容哈希 → immutable 缓存。
async fn serve_asset(Path(file): Path<String>) -> Response {
    // 路径段白名单（防遍历；include_dir 查的也是编译期固定的树）。
    let safe = !file.is_empty()
        && file
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    let content_type = if file.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if file.ends_with(".js") {
        "text/javascript; charset=utf-8"
    } else {
        "application/octet-stream"
    };
    if !safe {
        return failure(
            StatusCode::NOT_FOUND,
            ErrorCode::NotFound,
            "asset not found".to_string(),
        );
    }
    match DIST.get_file(format!("assets/{file}").as_str()) {
        Some(asset_file) => asset(asset_file.contents(), content_type, true),
        None => failure(
            StatusCode::NOT_FOUND,
            ErrorCode::NotFound,
            "asset not found".to_string(),
        ),
    }
}

// --------------------------------------------------------------------------- #
// API
// --------------------------------------------------------------------------- #

async fn api_status(State(state): State<AppState>) -> Response {
    let admin = &state.admin;
    let core = admin.core_info();
    let capabilities = admin.capabilities();
    match admin.list() {
        Ok(views) => Json(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "core": {"name": core.name, "version": core.version},
            "capabilities": {
                "listen": capabilities.listen,
                "dynamic_certs": capabilities.dynamic_certs,
                "rewrite_upstream": capabilities.rewrite_upstream,
                "per_domain_insecure": capabilities.per_domain_insecure,
                "shared_ca": capabilities.shared_ca,
                "http1_only": capabilities.http1_only,
            },
            "config": {
                "state_dir": admin.config().state_dir.display().to_string(),
                "port_range": format!("{}-{}", admin.config().port_range.0, admin.config().port_range.1),
            },
            "environments": views.len(),
            "running": views.iter().filter(|view| view.health == "running").count(),
            "events_dropped": admin.events_dropped(),
        }))
        .into_response(),
        Err(error) => error_response(error),
    }
}

/// GET /api/environments/:name/trajectory?limit=N —— 请求轨迹尾部（拉取式）。
/// 响应体是轨迹窗口帧（`spec/protocol.md`）：`cursor` = 窗口末端字节偏移，
/// 前端把它原样传回 SSE 的 `?cursor=` 即从断点续传。
async fn api_trajectory(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let limit = params
        .get("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(200)
        .clamp(1, 2000);
    match state.admin.trajectory(&name, limit) {
        Ok((cursor, events)) => Json(TrajectoryWindow { cursor, events }).into_response(),
        Err(error) => error_response(error),
    }
}

/// GET /api/environments/:name/captures?limit=N —— 抓包会话尾部（易失）。
async fn api_captures(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let limit = params
        .get("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(100)
        .clamp(1, 2000);
    match state.admin.captures(&name, limit) {
        Ok(Some(view)) => Json(view).into_response(),
        Ok(None) => not_found_response(&name),
        Err(error) => error_response(error),
    }
}

/// GET /api/environments/:name/captures/:request_id —— 单条抓包详情。
async fn api_capture(
    State(state): State<AppState>,
    Path((name, request_id)): Path<(String, String)>,
) -> Response {
    let Ok(request_id) = request_id.parse::<u64>() else {
        return error_response(Error::invalid_config(
            "request_id",
            format!("request_id must be an integer, got {request_id:?}"),
        ));
    };
    // 全缓冲查找（曾经的实现是 tail(1) 再比对 —— 只有最新一条查得到，旧的必 404）。
    match state.admin.capture_detail(&name, request_id) {
        Ok(Some(record)) => Json(record).into_response(),
        Ok(None) => not_found_response(&name),
        Err(error) => error_response(error),
    }
}

/// POST /api/environments/:name/capture/clear —— 手动清空会话（会话延续）。
async fn api_capture_clear(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    match state.admin.clear_capture(&name) {
        Ok(true) => Json(json!({"ok": true})).into_response(),
        Ok(false) => not_found_response(&name),
        Err(error) => error_response(error),
    }
}

/// GET /api/environments/:name/captures/export?format=har|jsonl —— 导出当前会话。
async fn api_captures_export(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let format = params.get("format").map(String::as_str).unwrap_or("har");
    let Some(view) = state.admin.captures(&name, usize::MAX).unwrap_or(None) else {
        return not_found_response(&name);
    };
    match format {
        "jsonl" => {
            let mut body = envboard_events::header_line();
            body.push('\n');
            for record in &view.records {
                body.push_str(&record.to_string());
                body.push('\n');
            }
            attachment(
                body,
                &format!("{name}-session-{}.jsonl", view.session.id),
                "application/x-ndjson",
            )
        }
        "har" => {
            let har = har_from_records(&name, &view.records);
            match serde_json::to_string_pretty(&har) {
                Ok(text) => attachment(
                    text,
                    &format!("{name}-session-{}.har", view.session.id),
                    "application/json",
                ),
                Err(error) => error_response(Error::new(
                    envboard_engine::ErrorCode::InternalError,
                    format!("har serialization failed: {error}"),
                )),
            }
        }
        other => error_response(Error::invalid_config(
            "format",
            format!("format must be har or jsonl, got {other:?}"),
        )),
    }
}

fn not_found_response(name: &str) -> Response {
    error_response(Error::new(
        envboard_engine::ErrorCode::NotFound,
        format!(
            "no live capture session for environment {name:?} (session lives with the running instance)"
        ),
    ))
}

fn attachment(body: String, filename: &str, content_type: &str) -> Response {
    (
        StatusCode::OK,
        [(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\u{22}{filename}\u{22}"),
        )],
        [(header::CONTENT_TYPE, content_type)],
        body,
    )
        .into_response()
}

/// HAR 1.2 形状（对齐 mitmproxy 的 savehar 输出）：只含我们记录到的字段。
fn har_from_records(env: &str, records: &[Value]) -> Value {
    let entries: Vec<Value> = records
        .iter()
        .filter_map(|record| {
            let request = record.get("request")?;
            let response = record.get("response")?;
            let body_to_har = |body: &Value| -> Value {
                let size = body.get("size").and_then(Value::as_u64).unwrap_or(0);
                if body.get("omitted").is_some() {
                    json!({"size": size, "mimeType": null, "text": null})
                } else {
                    let text = body.get("content").and_then(Value::as_str).unwrap_or("");
                    json!({"size": size, "mimeType": null, "text": text})
                }
            };
            Some(json!({
                "startedDateTime": record.get("time")
                    .and_then(Value::as_u64)
                    .map(|ms| format!("{ms}"))
                    .unwrap_or_default(),
                "time": 0,
                "_envboard": {
                    "session": record.get("session").cloned().unwrap_or(Value::Null),
                    "request_id": record.get("request_id").cloned().unwrap_or(Value::Null),
                },
                "request": {
                    "method": request.get("method").cloned().unwrap_or(Value::Null),
                    "url": format!("http://{}{}",
                        request.get("authority").and_then(Value::as_str).unwrap_or(""),
                        request.get("path").and_then(Value::as_str).unwrap_or("")),
                    "httpVersion": "HTTP/1.1",
                    "headers": headers_to_har(request.get("headers")),
                    "queryString": [],
                    "headersSize": -1,
                    "bodySize": request.get("body").and_then(|b| b.get("size")).cloned().unwrap_or(json!(-1)),
                    "postData": body_to_har(request.get("body").unwrap_or(&Value::Null)),
                },
                "response": {
                    "status": response.get("status").cloned().unwrap_or(Value::Null),
                    "statusText": "",
                    "httpVersion": "HTTP/1.1",
                    "headers": headers_to_har(response.get("headers")),
                    "content": body_to_har(response.get("body").unwrap_or(&Value::Null)),
                    "headersSize": -1,
                    "bodySize": response.get("body").and_then(|b| b.get("size")).cloned().unwrap_or(json!(-1)),
                    "redirectURL": "",
                },
                "cache": {},
                "timings": {"send": 0, "wait": -1, "receive": 0},
            }))
        })
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

fn headers_to_har(value: Option<&Value>) -> Value {
    let Some(pairs) = value.and_then(Value::as_array) else {
        return Value::Array(vec![]);
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

/// GET /api/environments/:name/trajectory/stream —— SSE 实时轨迹。
/// 连接即发 `baseline`（尾部窗口 + 窗口末端字节偏移游标），此后按 500ms 轮询
/// 文件增量发 `events`；断线重连由前端带 `?cursor=` 从断点续传。
/// 帧形状 = `TrajectoryWindow`（`spec/protocol.md`）。
async fn api_trajectory_stream(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let admin = state.admin.clone();
    let name = name.clone();
    let cursor = params
        .get("cursor")
        .and_then(|value| value.parse::<u64>().ok());
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);
    tokio::spawn(async move {
        let encode = |event: &str, value: &serde_json::Value| {
            Ok(Event::default().event(event).data(value.to_string()))
        };
        // 基线：cursor 提供则从断点续传，否则发尾部窗口 + 窗口末端游标。
        let mut offset = match cursor {
            Some(offset) => offset,
            None => {
                let (window_cursor, events) = admin.trajectory(&name, 200).unwrap_or_default();
                let payload = serde_json::to_value(&TrajectoryWindow {
                    cursor: window_cursor,
                    events,
                })
                .unwrap_or(Value::Null);
                if tx.send(encode("baseline", &payload)).await.is_err() {
                    return;
                }
                window_cursor
            }
        };
        let mut ticker = tokio::time::interval(Duration::from_millis(500));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match admin.trajectory_since(&name, offset) {
                Ok((new_offset, events)) if !events.is_empty() => {
                    offset = new_offset;
                    let payload = serde_json::to_value(&TrajectoryWindow {
                        cursor: offset,
                        events,
                    })
                    .unwrap_or(Value::Null);
                    if tx.send(encode("events", &payload)).await.is_err() {
                        return;
                    }
                }
                Ok((new_offset, _)) => offset = new_offset,
                Err(error) => {
                    let payload = serde_json::to_value(&ErrorFrame {
                        ok: false,
                        error: ErrorBody {
                            code: error.code.as_str().to_string(),
                            message: error.message,
                            field: error.field,
                        },
                    })
                    .unwrap_or(Value::Null);
                    let _ = tx.send(encode("error", &payload)).await;
                    return;
                }
            }
        }
    });
    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

/// POST /api/debug {"env": "..."} —— 开启/切换调试会话（工作区级单例；
/// 原目标环境抓包停止并清空）。
async fn api_debug_start(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    let Some(env) = body.get("env").and_then(Value::as_str) else {
        return error_response(Error::invalid_config("env", "env is required"));
    };
    match state.admin.start_debug(&json!({"env": env})) {
        Ok(view) => Json(view).into_response(),
        Err(error) => error_response(error),
    }
}

/// POST /api/debug/stop —— 停止并清空调试会话。
async fn api_debug_stop(State(state): State<AppState>) -> Response {
    match state.admin.stop_debug() {
        Ok(stopped) => Json(json!({"ok": stopped})).into_response(),
        Err(error) => error_response(error),
    }
}

/// GET /api/debug —— 当前调试会话视图。
async fn api_debug(State(state): State<AppState>) -> Response {
    match state.admin.debug_view() {
        Some(view) => Json(view).into_response(),
        None => Json(json!({"env": null})).into_response(),
    }
}

/// GET /api/debug/stream —— 调试会话的抓包实时推送（SSE）。
///
/// 帧形状 = `DebugSnapshot` / `DebugEvents`（`spec/protocol.md`）：连接即发
/// `snapshot`（形状 = `GET /api/debug` 的 DebugView，整幅替换，游标 = 会话内
/// 最新 request_id）；此后每 500ms 轮询内存缓冲，有增量发 `events` 帧。
/// 游标（request_id）由流自己维护 —— 淘汰只会移除游标之前的记录，增量无缺口，
/// 所以断线重连（EventSource 自动）只需重新收一次 snapshot。
/// 会话换代（实例重启 / clear，即 session.id 或 generation 变化）→ 重发
/// `snapshot`；目标消失（停止/删除）→ `snapshot` 帧 `{cursor:0, env:null, capture:null}`。
async fn api_debug_stream(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let admin = state.admin.clone();
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);
    tokio::spawn(async move {
        let encode = |event: &str, value: &serde_json::Value| {
            Ok(Event::default().event(event).data(value.to_string()))
        };
        let mut cursor = 0u64;
        let mut session_key: Option<(u64, u64)> = None;
        let mut last_counts: Option<(u64, u64)> = None;
        // 契约「连接即发 snapshot」：首帧无条件发出（无目标时 env:null），
        // 之后 idle→idle 不重发、会话出现/换代才再发 snapshot。
        let mut announced = false;
        let mut ticker = tokio::time::interval(Duration::from_millis(500));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let Some((env, delta)) = admin.debug_delta(cursor, 500) else {
                if !announced || session_key.is_some() {
                    session_key = None;
                    cursor = 0;
                    last_counts = None;
                    announced = true;
                    let payload = serde_json::to_value(&DebugSnapshot {
                        cursor: 0,
                        env: None,
                        capture: None,
                    })
                    .unwrap_or(Value::Null);
                    if tx.send(encode("snapshot", &payload)).await.is_err() {
                        return;
                    }
                }
                continue;
            };
            announced = true;
            let key = (delta.session.id, delta.session.generation);
            if session_key != Some(key) {
                // 新会话/换代：snapshot 用尾部窗口（与拉取端点同形），
                // 游标推进到窗口内最新一条；本 tick 的 delta 记录被窗口覆盖，丢弃。
                let Some(view) = admin.captures(&env, 500).ok().flatten() else {
                    continue; // 目标实例刚好停了：下个 tick 走 idle 分支
                };
                cursor = view
                    .records
                    .last()
                    .and_then(|record| record.get("request_id"))
                    .and_then(Value::as_u64)
                    .unwrap_or(cursor);
                session_key = Some(key);
                last_counts = Some((view.captured, view.dropped));
                // 与 GET /api/debug 的 DebugView 同形（snapshot 帧 = 整幅替换）。
                let payload = serde_json::to_value(&DebugSnapshot {
                    cursor,
                    env: Some(env),
                    capture: Some(view),
                })
                .unwrap_or(Value::Null);
                if tx.send(encode("snapshot", &payload)).await.is_err() {
                    return;
                }
            } else {
                let counts = (delta.captured, delta.dropped);
                if delta.records.is_empty() && last_counts == Some(counts) {
                    continue;
                }
                if let Some(id) = delta
                    .records
                    .last()
                    .and_then(|record| record.get("request_id"))
                    .and_then(Value::as_u64)
                {
                    cursor = id;
                }
                last_counts = Some(counts);
                let payload = serde_json::to_value(&DebugEvents {
                    cursor,
                    records: delta.records,
                    captured: counts.0,
                    dropped: counts.1,
                })
                .unwrap_or(Value::Null);
                if tx.send(encode("events", &payload)).await.is_err() {
                    return;
                }
            }
        }
    });
    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

/// POST /api/har/import —— 导入 HAR 会话（body = HAR 1.2 JSON；name 走查询参数）。
async fn api_har_import(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    Json(body): Json<Value>,
) -> Response {
    let name = params
        .get("name")
        .cloned()
        .unwrap_or_else(|| "imported.har".to_string());
    match state.admin.har_import(&name, &body) {
        Ok(view) => Json(view).into_response(),
        Err(error) => error_response(error),
    }
}

/// GET /api/har —— 导入会话列表。
async fn api_har_list(State(state): State<AppState>) -> Response {
    Json(json!({ "sessions": state.admin.har_list() })).into_response()
}

/// GET /api/har/:id?limit=N —— 导入会话条目窗口。
async fn api_har_get(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let Ok(id) = id.parse::<u64>() else {
        return not_found_response(&id);
    };
    let limit = params
        .get("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(200)
        .clamp(1, 5000);
    match state.admin.har_get(id, limit) {
        Some(view) => Json(view).into_response(),
        None => not_found_response(&id.to_string()),
    }
}

/// DELETE /api/har/:id —— 删除导入会话。
async fn api_har_delete(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Ok(id) = id.parse::<u64>() else {
        return not_found_response(&id);
    };
    Json(json!({"ok": state.admin.har_delete(id)})).into_response()
}

/// GET /api/history?name=<env>&limit=N —— 控制面审计事件（只读、拉取式）。
/// 事件是"为什么变成这样"的证据面；权威状态仍由 /api/environments 回答。
async fn api_history(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let name = params.get("name").map(String::as_str);
    let limit = params
        .get("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(100)
        .clamp(1, 1000);
    match state.admin.history(name, limit) {
        Ok(events) => Json(json!({ "events": events })).into_response(),
        Err(error) => error_response(error),
    }
}

/// 设置页「证书信息」：组合根在启动时读出的只读摘要，原样透出。
/// CA 是进程常量（运行期不轮换），所以不走 manager、也不做逐次重读。
async fn api_ca(State(state): State<AppState>) -> Response {
    match &state.ca {
        Some(info) => Json(info.clone()).into_response(),
        None => failure(
            StatusCode::NOT_FOUND,
            ErrorCode::NotFound,
            "shared CA certificate is not readable".to_string(),
        ),
    }
}

/// 下载根证书：mitmproxy 兼容形态的 -ca-cert.pem 内容（只含证书，不含私钥）。
async fn api_ca_pem(State(state): State<AppState>) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-pem-file"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=envboard-root-ca.pem"),
    );
    (headers, state.ca_pem.to_vec()).into_response()
}

/// 根证书下载地址的二维码（SVG）。内容是浏览器传入的下载 URL —— 服务端不猜
/// 谁在访问（手机要扫到的是它能直连的那个地址，只有浏览器知道），只做长度
/// 限制防滥用。前端直接放进 <img>（CSP 的 img-src 允许 'self'）。
async fn api_ca_qrcode(
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let data = params.get("data").cloned().unwrap_or_default();
    if data.is_empty() || data.len() > 512 {
        return failure(
            StatusCode::BAD_REQUEST,
            ErrorCode::InvalidConfig,
            "qrcode data must be 1..=512 bytes".to_string(),
        );
    }
    let code =
        qrcode::QrCode::with_error_correction_level(data.as_bytes(), qrcode::types::EcLevel::M);
    let Ok(code) = code else {
        return failure(
            StatusCode::BAD_REQUEST,
            ErrorCode::InvalidConfig,
            "cannot encode qrcode".to_string(),
        );
    };
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .dark_color(qrcode::render::svg::Color("#141a24"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build();
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("image/svg+xml"),
        )],
        svg,
    )
        .into_response()
}

async fn api_list(State(state): State<AppState>) -> Response {
    match state.admin.list() {
        Ok(views) => Json(views).into_response(),
        Err(error) => error_response(error),
    }
}

async fn api_get(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    match state.admin.get(&name) {
        Ok(view) => Json(view).into_response(),
        Err(error) => error_response(error),
    }
}

async fn api_create(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    match state.admin.create(&body) {
        Ok(view) => (StatusCode::CREATED, Json(view)).into_response(),
        Err(error) => error_response(error),
    }
}

async fn api_update(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    match state.admin.update(&name, &body) {
        Ok(view) => Json(view).into_response(),
        Err(error) => error_response(error),
    }
}

async fn api_remove(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    match state.admin.remove(&name) {
        Ok(()) => Json(json!({"removed": name})).into_response(),
        Err(error) => error_response(error),
    }
}

async fn api_start(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    match state.admin.start(&name).await {
        Ok(view) => Json(view).into_response(),
        Err(error) => error_response(error),
    }
}

async fn api_stop(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    match state.admin.stop(&name).await {
        Ok(view) => Json(view).into_response(),
        Err(error) => error_response(error),
    }
}

async fn api_restart(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    match state.admin.restart(&name).await {
        Ok(view) => Json(view).into_response(),
        Err(error) => error_response(error),
    }
}

async fn api_reallocate(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    match state.admin.reallocate_port(&name) {
        Ok(view) => Json(view).into_response(),
        Err(error) => error_response(error),
    }
}

#[derive(serde::Deserialize)]
struct LogsQuery {
    #[serde(default = "default_lines")]
    lines: usize,
}

fn default_lines() -> usize {
    200
}

/// POST /api/_fault {"env": "...", "reason": "..."} —— 把在跑实例置为 failed
/// （等价于"引擎线程死了"的可观察形态）。只有实现了注入旋钮的核心会成功；
/// 真引擎与 fake 都实现它，语义是"模拟线程死亡"，不是新造状态机分支。
async fn api_fault(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    let env = body.get("env").and_then(Value::as_str).unwrap_or_default();
    let reason = body
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("fault injection");
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        state.admin.inject_fault(env, reason)
    })) {
        Ok(true) => Json(serde_json::json!({"injected": true, "env": env})).into_response(),
        Ok(false) => (
            axum::http::StatusCode::BAD_REQUEST,
            format!(
                "core does not support fault injection for {env:?} (not running, or unsupported)"
            ),
        )
            .into_response(),
        Err(_) => (
            axum::http::StatusCode::BAD_REQUEST,
            "fault injection failed".to_string(),
        )
            .into_response(),
    }
}

async fn api_logs(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<LogsQuery>,
) -> Response {
    match state.admin.logs_tail(&name, query.lines) {
        Ok(lines) => Json(json!({"env": name, "lines": lines})).into_response(),
        Err(error) => error_response(error),
    }
}

async fn api_rules_list(State(state): State<AppState>) -> Response {
    match state.admin.rules_list() {
        Ok(names) => Json(json!({"rules": names})).into_response(),
        Err(error) => error_response(error),
    }
}

#[derive(serde::Deserialize)]
struct RulesImport {
    name: String,
    #[serde(default)]
    text: String,
}

async fn api_rules_import(
    State(state): State<AppState>,
    Json(body): Json<RulesImport>,
) -> Response {
    // 导入挂在**集合**上（`POST /api/rules`），不用 `/api/rules/import` ——
    // `import` 是合法资源名，会与 `{name}` 路由撞车（v1 踩过 405）。
    match state.admin.import_rules(&body.name, &body.text) {
        Ok(path) => (
            StatusCode::CREATED,
            Json(json!({"name": body.name, "path": path.display().to_string()})),
        )
            .into_response(),
        Err(error) => error_response(error),
    }
}

async fn api_rules_read(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    match state.admin.rules_read(&name) {
        Ok(text) => Json(json!({"name": name, "text": text})).into_response(),
        Err(error) => error_response(error),
    }
}

async fn api_rules_delete(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    match state.admin.rules_delete(&name) {
        Ok(()) => Json(json!({"removed": name})).into_response(),
        Err(error) => error_response(error),
    }
}

async fn api_proxy_list(State(state): State<AppState>) -> Response {
    match state.admin.proxy_list() {
        Ok(views) => Json(views).into_response(),
        Err(error) => error_response(error),
    }
}

async fn api_proxy_put(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    match state.admin.proxy_put(&body) {
        Ok(view) => (StatusCode::CREATED, Json(view)).into_response(),
        Err(error) => error_response(error),
    }
}

async fn api_proxy_get(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    match state.admin.proxy_get(&name) {
        Ok(view) => Json(view).into_response(),
        Err(error) => error_response(error),
    }
}

async fn api_proxy_delete(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    match state.admin.proxy_delete(&name) {
        Ok(()) => Json(json!({"removed": name})).into_response(),
        Err(error) => error_response(error),
    }
}

#[derive(serde::Deserialize)]
struct CompareQuery {
    host: String,
}

async fn api_compare(State(state): State<AppState>, Query(query): Query<CompareQuery>) -> Response {
    match state.admin.compare(&query.host) {
        Ok(value) => Json(value).into_response(),
        Err(error) => error_response(error),
    }
}

/// SSE：把"当前全部环境"作为事件推给前端。
///
/// 刻意用"每秒推一次快照"而不是事件溯源：快照不需要前端维护增量状态，也不会因为
/// 丢一条事件就长期显示错误（单机工具的规模下，简单比精巧更可靠）。
/// 帧 = `SnapshotFrame`：`cursor` 是控制面账本代次（每次控制面写入 +1），
/// 作为快照版本号 —— 健康变化不经过控制面写入，同代次两帧内容仍可能不同，
/// 前端不得据此跳过渲染（`spec/protocol.md`）。
async fn api_events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let interval = tokio::time::interval(Duration::from_millis(1_000));
    let stream = IntervalStream::new(interval).map(move |_| {
        let cursor = state.admin.state_generation();
        let payload = match state.admin.list() {
            Ok(views) => {
                let environments = views
                    .iter()
                    .map(|view| serde_json::to_value(view).unwrap_or(Value::Null))
                    .collect();
                SnapshotFrame {
                    ok: true,
                    cursor,
                    environments,
                    error: None,
                }
            }
            Err(error) => SnapshotFrame {
                ok: false,
                cursor,
                environments: Vec::new(),
                error: Some(ErrorBody {
                    code: error.code.as_str().to_string(),
                    message: error.message,
                    field: error.field,
                }),
            },
        };
        Ok(Event::default()
            .event("snapshot")
            .data(serde_json::to_string(&payload).unwrap_or_default()))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_token_reads_the_bare_token_key() {
        assert_eq!(query_token(Some("token=abc")), Some("abc".into()));
        assert_eq!(query_token(Some("a=1&token=abc&b=2")), Some("abc".into()));
        // 没有值 / 只有键名 / 完全没有 token，都视为未携带
        assert_eq!(query_token(Some("token=")), Some("".into()));
        assert_eq!(query_token(Some("a=1")), None);
        assert_eq!(query_token(None), None);
        // 前缀撞名不算（xxxtoken 不是 token）
        assert_eq!(query_token(Some("xxxtoken=abc")), None);
    }

    #[test]
    fn constant_time_eq_is_an_exact_string_compare() {
        assert!(constant_time_eq("secret", "secret"));
        assert!(!constant_time_eq("secret", "secreT"));
        assert!(!constant_time_eq("secret", "secrets"));
        assert!(!constant_time_eq("", "a"));
        assert!(constant_time_eq("", ""));
    }
}

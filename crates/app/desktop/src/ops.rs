//! 命令语义的**纯函数核**：每个函数只认门面（`AdminService`）与组合根交付的
//! CA 操作面（`CaControl`），不认 Tauri —— 命令级契约测试直接打这里，不需要 webview。
//!
//! 响应形状与退场的 web 传输面（`crates/app/web/src/api.rs`，git 历史可查）逐键一致：
//! 前端 `src/api/types.ts` 是 wire 形状的镜像，形状漂移就是契约漂移。
//! 错误一律走门面的统一信封 `{code, message, field?}`（`envboard-engine::Error`）。

use envboard_admin::AdminService;
use envboard_engine::{Error, ErrorCode};
use envboard_host::{CaControl, StorePresence};
use serde_json::{Value, json};

/// 命令核的统一返回：成功 = wire 形状的 JSON，失败 = 门面错误信封。
pub type OpsResult = Result<Value, Error>;

fn value<T: serde::Serialize>(item: T) -> OpsResult {
    serde_json::to_value(item).map_err(|error| Error::internal_error(error.to_string()))
}

fn not_found(message: String) -> Error {
    Error::new(ErrorCode::NotFound, message)
}

/// CA 不可用（测试注入假引擎，没有物化 CA）时的统一错误。
fn ca_unavailable() -> Error {
    not_found("shared CA is not materialized in this host".to_string())
}

fn no_live_session(name: &str) -> Error {
    not_found(format!(
        "no live capture session for environment {name:?} (session lives with the running instance)"
    ))
}

/* ---------------- status ---------------- */

pub fn status(admin: &AdminService) -> OpsResult {
    let core = admin.core_info();
    let capabilities = admin.capabilities();
    let views = admin.list()?;
    let config = admin.config();
    Ok(json!({
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
            "state_dir": config.state_dir.display().to_string(),
            "port_range": format!("{}-{}", config.port_range.0, config.port_range.1),
        },
        "environments": views.len(),
        "running": views.iter().filter(|view| view.health == "running").count(),
        "events_dropped": admin.events_dropped(),
    }))
}

/* ---------------- CA 操作面 ---------------- */

pub fn ca_overview(ca: Option<&CaControl>) -> OpsResult {
    let control = ca.ok_or_else(ca_unavailable)?;
    let presence = |p: StorePresence| match p {
        StorePresence::Installed => "installed",
        StorePresence::Missing => "missing",
        StorePresence::Unknown => "unknown",
    };
    let trust = control.trust();
    Ok(json!({
        "info": control.info(),
        "fingerprint": control.fingerprint(),
        "cert_path": control.cert_path().display().to_string(),
        "trust": {
            "current_user": presence(trust.current_user),
            "machine": presence(trust.machine),
            "installed": trust.is_installed(),
            "detail": trust.detail,
        },
    }))
}

/// 把当前 CA 直接安装进**当前用户** ROOT 存储（免管理员、幂等）。
///
/// 不再走系统证书导入向导：位置固定为当前用户，用户不需要在向导里做选择。
pub fn ca_install_cert(ca: Option<&CaControl>) -> OpsResult {
    let control = ca.ok_or_else(ca_unavailable)?;
    control.install_trust()?;
    Ok(json!({"ok": true}))
}

/// 卸载证书：只处理**当前用户** ROOT 存储（免管理员、幂等）。
///
/// 计算机存储里的旧证书不在本版本的处理范围 —— 仍会探测并提示，
/// 但不拉起提权助手，也不假装已经清空。
pub fn ca_uninstall_cert(ca: Option<&CaControl>) -> OpsResult {
    let control = ca.ok_or_else(ca_unavailable)?;
    let trust = control.trust();
    let removed = if trust.current_user == StorePresence::Installed {
        control.uninstall_trust()?
    } else {
        false
    };
    let warning = (trust.machine == StorePresence::Installed).then(|| {
        "计算机存储里仍有一份旧证书；本版本只管理当前用户存储，需要时请在系统证书管理器中手动移除"
            .to_string()
    });
    Ok(json!({
        "ok": true,
        "removed": removed,
        "warning": warning,
    }))
}

/* ---------------- 环境生命周期 ---------------- */

pub fn env_list(admin: &AdminService) -> OpsResult {
    value(admin.list()?)
}

pub fn env_get(admin: &AdminService, name: &str) -> OpsResult {
    value(admin.get(name)?)
}

pub fn env_create(admin: &AdminService, body: &Value) -> OpsResult {
    value(admin.create(body)?)
}

pub fn env_patch(admin: &AdminService, name: &str, patch: &Value) -> OpsResult {
    value(admin.update(name, patch)?)
}

pub fn env_delete(admin: &AdminService, name: &str) -> OpsResult {
    admin.remove(name)?;
    Ok(json!({"removed": name}))
}

pub async fn env_start(admin: &AdminService, name: &str) -> OpsResult {
    value(admin.start(name).await?)
}

pub async fn env_stop(admin: &AdminService, name: &str) -> OpsResult {
    value(admin.stop(name).await?)
}

pub async fn env_restart(admin: &AdminService, name: &str) -> OpsResult {
    value(admin.restart(name).await?)
}

pub fn env_reallocate(admin: &AdminService, name: &str) -> OpsResult {
    value(admin.reallocate_port(name)?)
}

pub fn env_logs(admin: &AdminService, name: &str, lines: usize) -> OpsResult {
    let lines = admin.logs_tail(name, lines)?;
    Ok(json!({"env": name, "lines": lines}))
}

pub fn env_captures(admin: &AdminService, name: &str, limit: usize) -> OpsResult {
    let limit = limit.clamp(1, 2000);
    match admin.captures(name, limit)? {
        Some(view) => value(view),
        None => Err(no_live_session(name)),
    }
}

pub fn env_capture_detail(admin: &AdminService, name: &str, request_id: u64) -> OpsResult {
    // 全缓冲查找（不是 tail(1) 再比对 —— 只有最新一条查得到是旧实现的 bug）。
    match admin.capture_detail(name, request_id)? {
        Some(record) => Ok(record),
        None => Err(no_live_session(name)),
    }
}

pub fn env_capture_clear(admin: &AdminService, name: &str) -> OpsResult {
    match admin.clear_capture(name)? {
        true => Ok(json!({"ok": true})),
        false => Err(no_live_session(name)),
    }
}

/// 导出当前会话：回 `{filename, text}`，前端落盘。HAR 1.2 形状对齐 mitmproxy savehar。
pub fn env_captures_export(admin: &AdminService, name: &str, format: &str) -> OpsResult {
    let view = admin
        .captures(name, usize::MAX)?
        .ok_or_else(|| no_live_session(name))?;
    match format {
        "jsonl" => {
            let mut body = envboard_events::header_line();
            body.push('\n');
            for record in &view.records {
                body.push_str(&record.to_string());
                body.push('\n');
            }
            Ok(json!({
                "filename": format!("{name}-session-{}.jsonl", view.session.id),
                "text": body,
            }))
        }
        "har" => {
            let har = har_from_records(name, &view.records);
            let text = serde_json::to_string_pretty(&har).map_err(|error| {
                Error::internal_error(format!("har serialization failed: {error}"))
            })?;
            Ok(json!({
                "filename": format!("{name}-session-{}.har", view.session.id),
                "text": text,
            }))
        }
        other => Err(Error::invalid_config(
            "format",
            format!("format must be har or jsonl, got {other:?}"),
        )),
    }
}

/// HAR 1.2 形状（自 web 传输面原样移植）：只含我们记录到的字段。
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

/* ---------------- 调试（工作区级单例） ---------------- */

pub fn debug_get(admin: &AdminService) -> OpsResult {
    match admin.debug_view() {
        Some(view) => value(view),
        None => Ok(json!({"env": null})),
    }
}

pub fn debug_start(admin: &AdminService, env: &str) -> OpsResult {
    value(admin.start_debug(&json!({"env": env}))?)
}

pub fn debug_stop(admin: &AdminService) -> OpsResult {
    Ok(json!({"ok": admin.stop_debug()?}))
}

/* ---------------- HAR 导入会话 ---------------- */

pub fn har_list(admin: &AdminService) -> OpsResult {
    Ok(json!({"sessions": admin.har_list()}))
}

pub fn har_import(admin: &AdminService, name: &str, body: &Value) -> OpsResult {
    value(admin.har_import(name, body)?)
}

pub fn har_get(admin: &AdminService, id: u64, limit: usize) -> OpsResult {
    let limit = limit.clamp(1, 5000);
    admin
        .har_get(id, limit)
        .ok_or_else(|| not_found(format!("no imported HAR session {id}")))
}

pub fn har_delete(admin: &AdminService, id: u64) -> OpsResult {
    Ok(json!({"ok": admin.har_delete(id)}))
}

/* ---------------- 规则库 ---------------- */

pub fn rules_list(admin: &AdminService) -> OpsResult {
    Ok(json!({"rules": admin.rules_list()?}))
}

pub fn rules_import(admin: &AdminService, name: &str, text: &str) -> OpsResult {
    let path = admin.import_rules(name, text)?;
    Ok(json!({"name": name, "path": path.display().to_string()}))
}

pub fn rules_get(admin: &AdminService, name: &str) -> OpsResult {
    let text = admin.rules_read(name)?;
    Ok(json!({"name": name, "text": text}))
}

pub fn rules_delete(admin: &AdminService, name: &str) -> OpsResult {
    admin.rules_delete(name)?;
    Ok(json!({"removed": name}))
}

/* ---------------- 上游代理 ---------------- */

pub fn proxies_list(admin: &AdminService) -> OpsResult {
    value(admin.proxy_list()?)
}

pub fn proxies_put(admin: &AdminService, body: &Value) -> OpsResult {
    value(admin.proxy_put(body)?)
}

pub fn proxies_delete(admin: &AdminService, name: &str) -> OpsResult {
    admin.proxy_delete(name)?;
    Ok(json!({"removed": name}))
}

/* ---------------- 测试探针 ---------------- */

/// 故障注入（验收用）：把在跑实例置为 failed。核心不支持时如实报错。
pub fn inject_fault(admin: &AdminService, env: &str, reason: &str) -> OpsResult {
    let injected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        admin.inject_fault(env, reason)
    }))
    .unwrap_or(false);
    if injected {
        Ok(json!({"injected": true, "env": env}))
    } else {
        Err(Error::invalid_config(
            "env",
            format!(
                "core does not support fault injection for {env:?} (not running, or unsupported)"
            ),
        ))
    }
}

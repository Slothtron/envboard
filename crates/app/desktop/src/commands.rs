//! Tauri 命令面：每个命令都是「取状态 → 打 ops 纯函数核 → 翻译错误」三行薄适配，
//! 零业务逻辑（语义与测试都在 `ops.rs`）。
//!
//! 错误信封 `{code, message, field?}` 原样过 IPC —— 前端 `ApiError` 与两层错误
//! 文案（`src/errors.ts`）不认识传输，只认这个形状。

use serde_json::Value;
use tauri::State;

use crate::Shell;
use crate::ops;

/// 门面错误 → IPC 可序列化信封（`envboard_engine::Error` 本身不实现 Serialize）。
#[derive(Debug, serde::Serialize)]
pub struct CmdError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

impl From<envboard_engine::Error> for CmdError {
    fn from(error: envboard_engine::Error) -> Self {
        Self {
            code: error.code.as_str().to_string(),
            message: error.message,
            field: error.field,
        }
    }
}

type CmdResult = Result<Value, CmdError>;

/* ---------------- status / CA ---------------- */

#[tauri::command]
pub fn status(shell: State<'_, Shell>) -> CmdResult {
    ops::status(&shell.admin).map_err(Into::into)
}

/* ---------------- UI 偏好（ui.json，ADR-012） ---------------- */

/// 主题偏好：`light / dark / system`，未写过返回 null（前端用自己的默认）。
#[tauri::command]
pub fn ui_prefs_get(shell: State<'_, Shell>) -> Option<String> {
    shell
        .ui_prefs
        .theme()
        .map(|theme| theme.as_str().to_string())
}

/// 写主题偏好：壳侧原子写进 state-dir 的 ui.json（不依赖 WebView localStorage）。
#[tauri::command]
pub fn ui_prefs_set(shell: State<'_, Shell>, theme: String) -> Result<(), CmdError> {
    let choice = crate::prefs::ThemeChoice::parse(&theme).ok_or_else(|| {
        envboard_engine::Error::invalid_config("theme", format!("unknown theme {theme:?}"))
    })?;
    shell.ui_prefs.set_theme(choice).map_err(Into::into)
}

#[tauri::command]
pub fn ca_overview(shell: State<'_, Shell>) -> CmdResult {
    ops::ca_overview(shell.ca.as_ref()).map_err(Into::into)
}

/// 安装到当前用户 ROOT 存储（crypt32 写入是阻塞调用）→ spawn_blocking，别占主线程。
#[tauri::command]
pub async fn ca_install_cert(shell: State<'_, Shell>) -> CmdResult {
    let ca = shell.ca.clone();
    tauri::async_runtime::spawn_blocking(move || ops::ca_install_cert(ca.as_ref()))
        .await
        .map_err(join_error)?
        .map_err(Into::into)
}

#[tauri::command]
pub async fn ca_uninstall_cert(shell: State<'_, Shell>) -> CmdResult {
    let ca = shell.ca.clone();
    tauri::async_runtime::spawn_blocking(move || ops::ca_uninstall_cert(ca.as_ref()))
        .await
        .map_err(join_error)?
        .map_err(Into::into)
}

fn join_error(error: impl std::fmt::Display) -> CmdError {
    envboard_engine::Error::new(
        envboard_engine::ErrorCode::InternalError,
        format!("trust store task failed: {error}"),
    )
    .into()
}

/* ---------------- 环境生命周期 ---------------- */

#[tauri::command]
pub fn env_list(shell: State<'_, Shell>) -> CmdResult {
    ops::env_list(&shell.admin).map_err(Into::into)
}

#[tauri::command]
pub fn env_get(shell: State<'_, Shell>, name: String) -> CmdResult {
    ops::env_get(&shell.admin, &name).map_err(Into::into)
}

#[tauri::command]
pub fn env_create(shell: State<'_, Shell>, body: Value) -> CmdResult {
    ops::env_create(&shell.admin, &body).map_err(Into::into)
}

#[tauri::command]
pub fn env_patch(shell: State<'_, Shell>, name: String, patch: Value) -> CmdResult {
    ops::env_patch(&shell.admin, &name, &patch).map_err(Into::into)
}

#[tauri::command]
pub fn env_delete(shell: State<'_, Shell>, name: String) -> CmdResult {
    ops::env_delete(&shell.admin, &name).map_err(Into::into)
}

#[tauri::command]
pub async fn env_start(shell: State<'_, Shell>, name: String) -> CmdResult {
    ops::env_start(&shell.admin, &name)
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn env_stop(shell: State<'_, Shell>, name: String) -> CmdResult {
    ops::env_stop(&shell.admin, &name).await.map_err(Into::into)
}

#[tauri::command]
pub async fn env_restart(shell: State<'_, Shell>, name: String) -> CmdResult {
    ops::env_restart(&shell.admin, &name)
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub fn env_reallocate(shell: State<'_, Shell>, name: String) -> CmdResult {
    ops::env_reallocate(&shell.admin, &name).map_err(Into::into)
}

#[tauri::command]
pub fn env_logs(shell: State<'_, Shell>, name: String, lines: Option<usize>) -> CmdResult {
    ops::env_logs(&shell.admin, &name, lines.unwrap_or(200)).map_err(Into::into)
}

#[tauri::command]
pub fn env_captures(shell: State<'_, Shell>, name: String, limit: Option<usize>) -> CmdResult {
    ops::env_captures(&shell.admin, &name, limit.unwrap_or(100)).map_err(Into::into)
}

#[tauri::command]
pub fn env_capture_detail(shell: State<'_, Shell>, name: String, request_id: u64) -> CmdResult {
    ops::env_capture_detail(&shell.admin, &name, request_id).map_err(Into::into)
}

#[tauri::command]
pub fn env_capture_clear(shell: State<'_, Shell>, name: String) -> CmdResult {
    ops::env_capture_clear(&shell.admin, &name).map_err(Into::into)
}

#[tauri::command]
pub fn env_captures_export(
    shell: State<'_, Shell>,
    name: String,
    format: Option<String>,
) -> CmdResult {
    ops::env_captures_export(&shell.admin, &name, format.as_deref().unwrap_or("har"))
        .map_err(Into::into)
}

/* ---------------- 调试 ---------------- */

#[tauri::command]
pub fn debug_get(shell: State<'_, Shell>) -> CmdResult {
    ops::debug_get(&shell.admin).map_err(Into::into)
}

#[tauri::command]
pub fn debug_start(shell: State<'_, Shell>, env: String) -> CmdResult {
    ops::debug_start(&shell.admin, &env).map_err(Into::into)
}

#[tauri::command]
pub fn debug_stop(shell: State<'_, Shell>) -> CmdResult {
    ops::debug_stop(&shell.admin).map_err(Into::into)
}

/* ---------------- HAR ---------------- */

#[tauri::command]
pub fn har_list(shell: State<'_, Shell>) -> CmdResult {
    ops::har_list(&shell.admin).map_err(Into::into)
}

#[tauri::command]
pub fn har_import(shell: State<'_, Shell>, name: Option<String>, body: Value) -> CmdResult {
    ops::har_import(
        &shell.admin,
        &name.unwrap_or_else(|| "imported.har".to_string()),
        &body,
    )
    .map_err(Into::into)
}

#[tauri::command]
pub fn har_get(shell: State<'_, Shell>, id: u64, limit: Option<usize>) -> CmdResult {
    ops::har_get(&shell.admin, id, limit.unwrap_or(200)).map_err(Into::into)
}

#[tauri::command]
pub fn har_delete(shell: State<'_, Shell>, id: u64) -> CmdResult {
    ops::har_delete(&shell.admin, id).map_err(Into::into)
}

/* ---------------- 规则库 / 上游代理 ---------------- */

#[tauri::command]
pub fn rules_list(shell: State<'_, Shell>) -> CmdResult {
    ops::rules_list(&shell.admin).map_err(Into::into)
}

#[tauri::command]
pub fn rules_import(shell: State<'_, Shell>, name: String, text: String) -> CmdResult {
    ops::rules_import(&shell.admin, &name, &text).map_err(Into::into)
}

#[tauri::command]
pub fn rules_get(shell: State<'_, Shell>, name: String) -> CmdResult {
    ops::rules_get(&shell.admin, &name).map_err(Into::into)
}

#[tauri::command]
pub fn rules_delete(shell: State<'_, Shell>, name: String) -> CmdResult {
    ops::rules_delete(&shell.admin, &name).map_err(Into::into)
}

#[tauri::command]
pub fn proxies_list(shell: State<'_, Shell>) -> CmdResult {
    ops::proxies_list(&shell.admin).map_err(Into::into)
}

#[tauri::command]
pub fn proxies_put(shell: State<'_, Shell>, body: Value) -> CmdResult {
    ops::proxies_put(&shell.admin, &body).map_err(Into::into)
}

#[tauri::command]
pub fn proxies_delete(shell: State<'_, Shell>, name: String) -> CmdResult {
    ops::proxies_delete(&shell.admin, &name).map_err(Into::into)
}

/* ---------------- 测试探针 ---------------- */

#[tauri::command]
pub fn inject_fault(shell: State<'_, Shell>, env: String, reason: Option<String>) -> CmdResult {
    ops::inject_fault(
        &shell.admin,
        &env,
        &reason.unwrap_or_else(|| "fault injection".to_string()),
    )
    .map_err(Into::into)
}

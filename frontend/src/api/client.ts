/**
 * Admin API 客户端 —— Tauri IPC 传输（ADR-006：web 控制台退场，桌面为唯一形态）。
 *
 * 命令语义的权威是 `crates/app/desktop/src/ops.rs`（纯函数核）+ `envboard-protocol`
 * crate 文档；wire 形状镜像在 `./types.ts`。视图层只认 `api` 对象的签名，
 * 传输从 HTTP 换成 IPC 对它不可见。
 *
 * 安全模型：token/CSRF/Host 守卫随 HTTP 面退场；IPC 面由 Tauri capabilities
 * 圈禁（只对 main 窗开放）。错误信封 `{code,message,field?}` 原样过 IPC。
 */

import { invoke } from "@tauri-apps/api/core";

import {
  ApiError,
  type CaOverview,
  type CaptureRecord,
  type CaptureView,
  type DebugView,
  type EnvCreateReq,
  type EnvPatchReq,
  type EnvView,
  type ErrorBody,
  type ProxyPutReq,
  type ProxyView,
  type RuleSummary,
  type StatusInfo,
} from "./types";

/* ---------------- 核心 invoke ---------------- */

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    // 命令核回的是信封 {code,message,field?}；非信封异常（IPC 本身坏了）兜底 unknown。
    const body = error as Partial<ErrorBody> | null;
    if (body && typeof body.code === "string" && typeof body.message === "string") {
      throw new ApiError(0, body as ErrorBody);
    }
    throw new ApiError(0, { code: "unknown", message: String(error) });
  }
}

/** 文本落盘（HAR / JSONL 导出）：blob + a[download]，WebView2 原生支持。 */
export function downloadText(filename: string, text: string, mime: string): void {
  const url = URL.createObjectURL(new Blob([text], { type: mime }));
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 1_000);
}

/* ---------------- 命令面 ---------------- */

export const api = {
  status: () => call<StatusInfo>("status"),

  /* ---------------- UI 偏好（ui.json，ADR-012） ---------------- */

  /** 主题偏好；未写过 = null（前端用自己的默认）。 */
  uiPrefsGet: () => call<string | null>("ui_prefs_get"),
  /** 写主题偏好：壳侧落盘 state-dir 的 ui.json，跨关窗/重建存活。 */
  uiPrefsSet: (theme: string) => call<void>("ui_prefs_set", { theme }),

  /* ---------------- CA 操作面（证书页） ---------------- */

  ca: () => call<CaOverview>("ca_overview"),
  /** 直接安装到当前用户 ROOT 存储（免管理员、幂等）。 */
  caInstallCert: () => call<{ ok: boolean }>("ca_install_cert"),
  /** 卸载：只处理当前用户 ROOT 存储；计算机存储里的旧证书只提示、不代删。 */
  caUninstallCert: () =>
    call<{ ok: boolean; removed: boolean; warning?: string | null }>("ca_uninstall_cert"),

  envList: () => call<EnvView[]>("env_list"),
  envGet: (name: string) => call<EnvView>("env_get", { name }),
  envCreate: (req: EnvCreateReq) => call<EnvView>("env_create", { body: req }),
  envPatch: (name: string, patch: EnvPatchReq) => call<EnvView>("env_patch", { name, patch }),
  envDelete: (name: string) => call<{ removed: string }>("env_delete", { name }),

  envStart: (name: string) => call<EnvView>("env_start", { name }),
  envStop: (name: string) => call<EnvView>("env_stop", { name }),
  envRestart: (name: string) => call<EnvView>("env_restart", { name }),
  envReallocate: (name: string) => call<EnvView>("env_reallocate", { name }),

  envLogs: (name: string, lines = 200) =>
    call<{ env: string; lines: string[] }>("env_logs", { name, lines }),
  envCaptures: (name: string, limit = 100) =>
    call<CaptureView>("env_captures", { name, limit }),
  envCaptureDetail: (name: string, requestId: number) =>
    call<CaptureRecord>("env_capture_detail", { name, requestId }),
  envCaptureClear: (name: string) => call<{ ok: boolean }>("env_capture_clear", { name }),
  /** 导出当前会话：回 {filename, text}，由 downloadText 落盘。 */
  envCapturesExport: (name: string, format: "har" | "jsonl" = "har") =>
    call<{ filename: string; text: string }>("env_captures_export", { name, format }),

  /* ---------------- 抓包子页（工作区级单例） ---------------- */

  debugGet: () => call<DebugView>("debug_get"),
  debugStart: (env: string) => call<DebugView>("debug_start", { env }),
  debugStop: () => call<{ ok: boolean }>("debug_stop"),

  /* HAR 导入前端入口已退场（ADR-011）：后端 har_* 命令与 ops 保留。 */

  /* ---------------- 规则库 ---------------- */

  rulesList: () => call<{ rules: string[] }>("rules_list"),
  rulesGet: (name: string) => call<{ name: string; text: string }>("rules_get", { name }),
  rulesImport: (req: { name: string; text: string }) =>
    call<RuleSummary>("rules_import", { name: req.name, text: req.text }),
  rulesDelete: (name: string) => call<{ removed: string }>("rules_delete", { name }),

  /* ---------------- 上游代理 ---------------- */

  proxiesList: () => call<ProxyView[]>("proxies_list"),
  proxiesPut: (req: ProxyPutReq) => call<ProxyView>("proxies_put", { body: req }),
  proxiesDelete: (name: string) => call<{ removed: string }>("proxies_delete", { name }),

};
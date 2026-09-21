/**
 * Admin API 客户端 —— 端点与形状的权威是 `spec/protocol.md` + `crates/web/src/api.rs`。
 *
 * 安全模型对齐（crates/web/src/config.rs）：
 * - 变更类请求（非 GET/HEAD）必须带 `x-envboard-request: 1`（防 CSRF）；
 * - token：头 `x-envboard-token` 优先；入口链接 `/?token=…` 由这里读出并随每个请求带上。
 */

import {
  ApiError,
  type CaptureRecord,
  type CompareResult,
  type ControlEvent,
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
  type TrajectoryWindow,
} from "./types";

/* ---------------- token ---------------- */

let token: string | null = new URLSearchParams(window.location.search).get("token");

/** 当前 token 值（SSE 拼 query 用）。 */
export function tokenValue(): string | null {
  return token;
}

/** SSE 只能走 query 参数（EventSource 不支持自定义头）。 */
export function tokenQuery(): string {
  return token ? `?token=${encodeURIComponent(token)}` : "";
}

/** 给直链（下载/二维码）补 token。 */
export function withToken(path: string): string {
  const sep = path.includes("?") ? "&" : "?";
  return token ? `${path}${sep}token=${encodeURIComponent(token)}` : path;
}

/* ---------------- 核心 fetch ---------------- */

async function request<T>(
  method: "GET" | "POST" | "PATCH" | "DELETE",
  path: string,
  body?: unknown,
): Promise<T> {
  const headers: Record<string, string> = {};
  if (method !== "GET") headers["x-envboard-request"] = "1";
  if (token) headers["x-envboard-token"] = token;
  if (body !== undefined) headers["content-type"] = "application/json";

  const response = await fetch(path, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
  });

  if (!response.ok) {
    // 错误信封 {error:{code,message,field}}；非 JSON 的异常响应兜底为 unknown。
    let error: ErrorBody;
    try {
      error = (await response.json()).error;
    } catch {
      error = { code: "unknown", message: `${method} ${path} → HTTP ${response.status}` };
    }
    throw new ApiError(response.status, error);
  }
  return (await response.json()) as T;
}

/* ---------------- 环境 ---------------- */

export const api = {
  status: () => request<StatusInfo>("GET", "/api/status"),
  ca: () => request<Record<string, unknown>>("GET", "/api/ca"),

  envList: () => request<EnvView[]>("GET", "/api/environments"),
  envGet: (name: string) => request<EnvView>("GET", `/api/environments/${encodeURIComponent(name)}`),
  envCreate: (req: EnvCreateReq) => request<EnvView>("POST", "/api/environments", req),
  envPatch: (name: string, patch: EnvPatchReq) =>
    request<EnvView>("PATCH", `/api/environments/${encodeURIComponent(name)}`, patch),
  envDelete: (name: string) =>
    request<{ removed: string }>("DELETE", `/api/environments/${encodeURIComponent(name)}`),

  envStart: (name: string) =>
    request<EnvView>("POST", `/api/environments/${encodeURIComponent(name)}/start`),
  envStop: (name: string) =>
    request<EnvView>("POST", `/api/environments/${encodeURIComponent(name)}/stop`),
  envRestart: (name: string) =>
    request<EnvView>("POST", `/api/environments/${encodeURIComponent(name)}/restart`),
  envReallocate: (name: string) =>
    request<EnvView>("POST", `/api/environments/${encodeURIComponent(name)}/reallocate`),

  envLogs: (name: string, lines = 200) =>
    request<{ env: string; lines: string[] }>(
      "GET",
      `/api/environments/${encodeURIComponent(name)}/logs?lines=${lines}`,
    ),
  envTrajectory: (name: string, limit = 200) =>
    request<TrajectoryWindow>(
      "GET",
      `/api/environments/${encodeURIComponent(name)}/trajectory?limit=${limit}`,
    ),
  envCaptures: (name: string, limit = 100) =>
    request<CaptureView>(
      "GET",
      `/api/environments/${encodeURIComponent(name)}/captures?limit=${limit}`,
    ),
  envCaptureDetail: (name: string, requestId: number) =>
    request<CaptureRecord>(
      "GET",
      `/api/environments/${encodeURIComponent(name)}/captures/${requestId}`,
    ),
  envCaptureClear: (name: string) =>
    request<{ ok: boolean }>("POST", `/api/environments/${encodeURIComponent(name)}/capture/clear`),

  /* ---------------- 调试（工作区级单例） ---------------- */

  debugGet: () => request<DebugView>("GET", "/api/debug"),
  debugStart: (env: string) => request<DebugView>("POST", "/api/debug", { env }),
  debugStop: () => request<{ ok: boolean }>("POST", "/api/debug/stop"),

  /* ---------------- HAR ---------------- */

  harList: () => request<{ sessions: unknown[] }>("GET", "/api/har"),
  harImport: (har: unknown, name?: string) =>
    request<CaptureView>(
      "POST",
      `/api/har${name ? `?name=${encodeURIComponent(name)}` : ""}`,
      har,
    ),
  harGet: (id: number, limit = 200) =>
    request<CaptureView>("GET", `/api/har/${id}?limit=${limit}`),
  harDelete: (id: number) => request<{ ok: boolean }>("DELETE", `/api/har/${id}`),

  /* ---------------- 规则库 ---------------- */

  rulesList: () => request<{ rules: string[] }>("GET", "/api/rules"),
  rulesGet: (name: string) =>
    request<{ name: string; text: string }>("GET", `/api/rules/${encodeURIComponent(name)}`),
  rulesImport: (req: { name: string; text: string }) =>
    request<RuleSummary>("POST", "/api/rules", req),
  rulesDelete: (name: string) =>
    request<{ removed: string }>("DELETE", `/api/rules/${encodeURIComponent(name)}`),

  /* ---------------- 上游代理 ---------------- */

  proxiesList: () => request<ProxyView[]>("GET", "/api/proxies"),
  proxiesPut: (req: ProxyPutReq) => request<ProxyView>("POST", "/api/proxies", req),
  proxiesDelete: (name: string) =>
    request<{ removed: string }>("DELETE", `/api/proxies/${encodeURIComponent(name)}`),

  /* ---------------- 对比与活动 ---------------- */

  compare: (host: string) =>
    request<CompareResult>("GET", `/api/compare?host=${encodeURIComponent(host)}`),
  history: (opts: { name?: string; limit?: number } = {}) => {
    const params = new URLSearchParams();
    if (opts.name) params.set("name", opts.name);
    params.set("limit", String(opts.limit ?? 100));
    return request<{ events: ControlEvent[] }>("GET", `/api/history?${params.toString()}`);
  },
};

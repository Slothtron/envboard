/**
 * Admin API 的 TypeScript 镜像 —— 唯一权威是 `crates/protocol`（DTO）与
 * `crates/web/src/api.rs`（端点）。本文件逐键对齐其 wire 形状（snake_case），
 * 改后端契约必须同步这里（spec/protocol.md 的「唯一词汇」纪律）。
 */

/* ---------------- 错误信封 ---------------- */

export interface ErrorBody {
  code: string;
  message: string;
  field?: string;
}

export class ApiError extends Error {
  readonly code: string;
  readonly field?: string;
  readonly status: number;

  constructor(status: number, body: ErrorBody) {
    super(body.message);
    this.code = body.code;
    this.field = body.field;
    this.status = status;
  }
}

/* ---------------- 视图 DTO ---------------- */

export interface Endpoint {
  host: string;
  port: number;
}

/** 健康字面量（starting/running/failed/stopped/port_conflict/unhealthy）。 */
export type Health = string;
export type Desired = "running" | "stopped";

export interface EnvView {
  name: string;
  listen: Endpoint;
  rules: string | null;
  upstream: string | null;
  insecure_hosts: string[];
  description: string;
  desired: Desired;
  health: Health;
  health_reason: string | null;
  rules_count: number;
  rules_missing: boolean;
  proxy_command: string;
  proxy_auth_enabled: boolean;
  capture: boolean;
}

export interface ProxyView {
  name: string;
  host: string;
  port: number;
  has_auth: boolean;
  references: string[];
}

/* ---------------- 抓包 ---------------- */

export type BodyEncoding =
  | { encoding: "utf8" | "base64"; size: number; content: string }
  | { omitted: true; size: number };

export interface CaptureRecord {
  version: number;
  session: number;
  request_id: number;
  time: number;
  request: {
    method: string;
    path: string;
    authority: string;
    headers: Array<[string, string]>;
    body: BodyEncoding | null;
  };
  response: {
    status: number;
    headers: Array<[string, string]>;
    body: BodyEncoding | null;
  } | null;
  error: unknown;
}

export interface SessionInfo {
  id: number;
  started_at: number;
  generation: number;
}

export interface CaptureView {
  session: SessionInfo;
  captured: number;
  dropped: number;
  records: CaptureRecord[];
}

export interface DebugView {
  env: string | null;
  capture: CaptureView | null;
}

/* ---------------- 推送流帧 ---------------- */

export interface SnapshotFrame {
  ok: boolean;
  cursor: number;
  environments: EnvView[];
  error?: ErrorBody;
}

export interface DebugSnapshot {
  cursor: number;
  env: string | null;
  capture: CaptureView | null;
}

export interface DebugEvents {
  cursor: number;
  records: CaptureRecord[];
  captured: number;
  dropped: number;
}

export interface TrajectoryWindow {
  cursor: number;
  events: Array<{ seq: number; time: number; type: string; data: Record<string, unknown> }>;
}

/* ---------------- 控制面审计事件（spec/events.md 信封） ---------------- */

export interface ControlEvent {
  seq: number;
  time: number;
  type: string;
  data: Record<string, unknown>;
}

/* ---------------- 其余响应 ---------------- */

export interface StatusInfo {
  version: string;
  core: { name: string; version: string };
  capabilities: Record<string, unknown>;
  config: { state_dir: string; port_range: [number, number] | { start: number; end: number } };
  environments: number;
  running: number;
  events_dropped: number;
}

export interface CompareRow {
  env: string;
  port: number;
  rules: string | null;
  ip: string | null;
  covered: boolean;
}

export interface CompareResult {
  host: string;
  environments: CompareRow[];
}

export interface RuleSummary {
  name: string;
  path: string;
}

/* ---------------- 请求体（裸对象） ---------------- */

export interface EnvCreateReq {
  name: string;
  listen?: Partial<Endpoint>;
  rules?: string | null;
  upstream?: string | null;
  description?: string;
  insecure_hosts?: string[];
  capture?: boolean;
  proxy_user?: string | null;
  proxy_password?: string | null;
}

export type EnvPatchReq = Partial<Omit<EnvCreateReq, "name">>;

export interface ProxyPutReq {
  name: string;
  host: string;
  port?: number;
  user?: string | null;
  password?: string | null;
}

export interface RulesImportReq {
  name: string;
  text: string;
}

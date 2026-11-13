/**
 * Admin API 的 TypeScript 镜像 —— 唯一权威是 `crates/core/protocol`（DTO）与
 * `crates/app/web/src/api.rs`（端点）。本文件逐键对齐其 wire 形状（snake_case），
 * 改后端契约必须同步这里（envboard-protocol crate 文档 的「唯一词汇」纪律）。
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
  /** MITM 白名单（空 = 全部解密；名单外的 CONNECT 直通）。 */
  mitm_hosts: string[];
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

/* ---------------- 其余响应 ---------------- */

export interface StatusInfo {
  version: string;
  core: { name: string; version: string };
  capabilities: Record<string, unknown>;
  config: {
    state_dir: string;
    // /api/status 实际回显字符串 "16000-16999"；兼容历史 tuple/object 形态。
    port_range: string | [number, number] | { start: number; end: number };
  };
  environments: number;
  running: number;
  events_dropped: number;
}

export interface RuleSummary {
  name: string;
  path: string;
}

/* ---------------- CA 操作面（GET /api/ca*，对齐 crates/service/host 的 CaControl） ---------------- */

/** 证书摘要（引擎 ca::CertInfo 的 wire 形状；结构异常时整个 info 为 null）。 */
export interface CertInfo {
  version?: number;
  serial_hex?: string;
  not_before?: string;
  not_after?: string;
  sig_alg?: string;
  pubkey_alg?: string;
  pubkey_curve?: string | null;
  pubkey_bits?: number | null;
  common_name?: string | null;
  organization?: string | null;
}

/** 系统信任库探测（只读；installed = 任一存储装了）。 */
export interface TrustInfo {
  current_user: "installed" | "missing" | "unknown";
  machine: "installed" | "missing" | "unknown";
  installed: boolean;
  detail: string | null;
}

export interface CaOverview {
  info: CertInfo | null;
  fingerprint: string;
  cert_path: string;
  trust: TrustInfo;
}

/* ---------------- 请求体（裸对象） ---------------- */

export interface EnvCreateReq {
  name: string;
  listen?: Partial<Endpoint>;
  rules?: string | null;
  upstream?: string | null;
  description?: string;
  insecure_hosts?: string[];
  /** MITM 白名单（空/缺省 = 全部解密）。与 insecure_hosts 同款：整体替换。 */
  mitm_hosts?: string[];
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

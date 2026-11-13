/**
 * mock 传输的种子数据（dev-tools，不进发布产物）。
 *
 * 形状严格对齐 `../src/api/types`；语义参照 `crates/app/desktop/tests/contract.rs`。
 * 开局就是一个可信的工作台：四个环境（两跑两停）、两套规则、一个带鉴权的上游、
 * 一份已导入的 HAR、若干抓包记录——主流程（建/改/删/启停/抓包）开箱可验。
 */

import type { CaOverview, CaptureRecord, EnvView } from "../src/api/types";

export const PORT_RANGE: [number, number] = [16000, 16999];

/** 环境内部态 = EnvView 的超集（凭据只进不出：view 里只有布尔）。 */
export interface EnvState {
  name: string;
  host: string;
  port: number;
  rules: string | null;
  upstream: string | null;
  insecure_hosts: string[];
  mitm_hosts: string[];
  description: string;
  desired: "running" | "stopped";
  health: string;
  health_reason: string | null;
  capture: boolean;
  proxyUser: string | null;
  proxyPassword: string | null;
  logs: string[];
  records: CaptureRecord[];
  sessionId: number;
  sessionStartedAt: number;
  generation: number;
  captured: number;
  dropped: number;
  nextRequestId: number;
}

export interface ProxyState {
  name: string;
  host: string;
  port: number;
  user: string | null;
  password: string | null;
}

export interface HarSession {
  id: number;
  name: string;
  importedAt: number;
  records: CaptureRecord[];
}

const now = Date.now();

function record(
  sessionId: number,
  requestId: number,
  method: string,
  authority: string,
  path: string,
  status: number,
  ageMs: number,
): CaptureRecord {
  const reqBody =
    method === "POST"
      ? { encoding: "utf8" as const, size: 27, content: '{"probe":"envboard-mock"}' }
      : null;
  return {
    version: 1,
    session: sessionId,
    request_id: requestId,
    time: now - ageMs,
    request: {
      method,
      path,
      authority,
      headers: [
        ["host", authority],
        ["user-agent", "envboard-mock/0.2.0"],
        ["accept", "application/json"],
      ],
      body: reqBody,
    },
    response: {
      status,
      headers: [
        ["content-type", "application/json"],
        ["x-mock", "envboard-dev-tools"],
      ],
      body: {
        encoding: "utf8",
        size: 19,
        content: `{"status":${status},"ok":true}`,
      },
    },
    error: null,
  };
}

export function seedEnvs(): EnvState[] {
  const base = {
    host: "127.0.0.1",
    insecure_hosts: [] as string[],
    mitm_hosts: [] as string[],
    proxyUser: null,
    proxyPassword: null,
    dropped: 0,
    generation: 1,
  };
  return [
    {
      ...base,
      name: "dev-api",
      port: 16000,
      rules: "dev-overrides",
      upstream: "corp-proxy",
      description: "开发环境 API 网关（热应用演示）",
      desired: "running",
      health: "running",
      health_reason: null,
      capture: true,
      insecure_hosts: ["staging.internal.example.com"],
      mitm_hosts: ["api.example.com", "cdn.example.com"],
      logs: [
        "INFO  engine: listening on 127.0.0.1:16000",
        "INFO  rules: dev-overrides compiled (4 entries)",
        "INFO  upstream: corp-proxy bound (CONNECT tunnel)",
        "INFO  capture: session 1 started",
      ],
      records: [
        record(1, 1, "GET", "api.example.com", "/v1/users", 200, 52_000),
        record(1, 2, "POST", "api.example.com", "/v1/orders", 201, 41_000),
        record(1, 3, "GET", "cdn.example.com", "/assets/app.js", 304, 30_000),
        record(1, 4, "GET", "api.example.com", "/v1/missing", 404, 18_000),
        record(1, 5, "GET", "api.example.com", "/v1/health", 200, 6_000),
      ],
      sessionId: 1,
      sessionStartedAt: now - 120_000,
      captured: 5,
      nextRequestId: 6,
    },
    {
      ...base,
      name: "staging",
      port: 16001,
      rules: "staging-hosts",
      upstream: null,
      description: "预发环境（明文直连）",
      desired: "running",
      health: "running",
      health_reason: null,
      capture: false,
      logs: ["INFO  engine: listening on 127.0.0.1:16001", "INFO  rules: staging-hosts compiled (2 entries)"],
      records: [],
      sessionId: 2,
      sessionStartedAt: now - 90_000,
      captured: 0,
      nextRequestId: 1,
    },
    {
      ...base,
      name: "prod-readonly",
      port: 16002,
      rules: null,
      upstream: "corp-proxy",
      description: "生产只读对照（停机态）",
      desired: "stopped",
      health: "stopped",
      health_reason: null,
      capture: false,
      logs: ["INFO  engine: stopped by user"],
      records: [],
      sessionId: 3,
      sessionStartedAt: now - 60_000,
      captured: 0,
      nextRequestId: 1,
    },
    {
      ...base,
      name: "sandbox",
      port: 16003,
      rules: "ghost-rules",
      upstream: null,
      description: "规则缺失演示（rules_missing）",
      desired: "stopped",
      health: "stopped",
      health_reason: null,
      capture: false,
      logs: ["WARN  rules: ghost-rules not found in ledger"],
      records: [],
      sessionId: 4,
      sessionStartedAt: now - 30_000,
      captured: 0,
      nextRequestId: 1,
    },
  ];
}

export function seedRules(): Map<string, string> {
  return new Map([
    [
      "dev-overrides",
      [
        "# 开发环境 hosts 覆盖（只改建连目标）",
        "api.example.com 127.0.0.1:9001",
        "cdn.example.com 127.0.0.1:9002",
        "staging.internal.example.com 10.0.0.8",
        "legacy.example.com 10.0.0.9:8080",
      ].join("\n"),
    ],
    [
      "staging-hosts",
      ["# 预发覆盖", "api.example.com 10.1.0.5", "cdn.example.com 10.1.0.6"].join("\n"),
    ],
  ]);
}

export function seedProxies(): Map<string, ProxyState> {
  return new Map([
    [
      "corp-proxy",
      {
        name: "corp-proxy",
        host: "proxy.corp.example.com",
        port: 8080,
        user: "svc-envboard",
        password: "mock-secret-never-echoed",
      },
    ],
  ]);
}

export function seedCa(): CaOverview {
  return {
    info: {
      version: 3,
      serial_hex: "01A2B3C4D5E6F708",
      not_before: "2026-11-13T00:00:00Z",
      not_after: "2036-11-10T00:00:00Z",
      sig_alg: "ECDSA-SHA256",
      pubkey_alg: "EC",
      pubkey_curve: "P-256",
      pubkey_bits: null,
      common_name: "envboard CA",
      organization: "envboard",
    },
    fingerprint: "9f2c41aa77b0d3e56c18de90a4f5b2c7d8e0f1a3b5c7d9e1f3a5b7c9d1e3f5a7",
    cert_path: "(mock) state-dir/shared/confdir/envboard-ca-cert.pem",
    trust: {
      current_user: "missing",
      machine: "missing",
      installed: false,
      detail: "mock 信任库：安装/卸载只翻转状态，不碰真系统存储",
    },
  };
}

export function seedHar(): HarSession[] {
  return [
    {
      id: 1,
      name: "baseline-import",
      importedAt: now - 3_600_000,
      records: [
        record(900, 1, "GET", "api.example.com", "/v1/baseline", 200, 3_600_000),
        record(900, 2, "GET", "cdn.example.com", "/assets/base.css", 200, 3_599_000),
      ],
    },
  ];
}

/** 模拟流量发生器用的域名/路径池（抓包流增量帧）。 */
export const TRAFFIC_POOL: Array<{ method: string; authority: string; path: string; status: number }> = [
  { method: "GET", authority: "api.example.com", path: "/v1/users", status: 200 },
  { method: "GET", authority: "api.example.com", path: "/v1/health", status: 200 },
  { method: "POST", authority: "api.example.com", path: "/v1/events", status: 201 },
  { method: "GET", authority: "cdn.example.com", path: "/assets/app.js", status: 304 },
  { method: "GET", authority: "api.example.com", path: "/v1/orders?page=2", status: 200 },
  { method: "GET", authority: "api.example.com", path: "/v1/missing", status: 404 },
];

/** EnvView 投影之外的便利：种子环境名列表（status 用）。 */
export function envNames(envs: EnvState[]): string[] {
  return envs.map((e) => e.name);
}

export type { EnvView };

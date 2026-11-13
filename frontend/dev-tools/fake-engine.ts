/**
 * mock 假引擎（dev-tools，不进发布产物）。
 *
 * 命令语义的权威是 `crates/app/desktop/src/ops.rs` + `tests/contract.rs`；
 * 这里按同一套词汇做内存实现：错误信封 `{code,message,field?}` 裸抛出
 * （client.ts 的 call() 原样识别）、凭据只进不出、端口冲突不换端口、
 * rules_delete 让绑定环境转 rules_missing、写操作后立即推快照帧。
 *
 * 与真身的刻意差异：无真实网络/端口/信任库——一切状态在内存，刷新即重置
 * （主题除外，落 localStorage 方便走查跨页签）。
 */

import type {
  CaOverview,
  CaptureRecord,
  CaptureView,
  DebugView,
  EnvCreateReq,
  EnvPatchReq,
  EnvView,
  ProxyPutReq,
  ProxyView,
  RuleSummary,
  StatusInfo,
} from "../src/api/types";
import {
  PORT_RANGE,
  TRAFFIC_POOL,
  seedCa,
  seedEnvs,
  seedHar,
  seedProxies,
  seedRules,
  type EnvState,
  type HarSession,
  type ProxyState,
} from "./fixtures";

/** 命令面全清单（parity 门禁拿它与 lib.rs 的 generate_handler! 比对）。 */
export const MOCK_COMMANDS: readonly string[] = [
  "status",
  "ui_prefs_get",
  "ui_prefs_set",
  "ca_overview",
  "ca_install_cert",
  "ca_uninstall_cert",
  "env_list",
  "env_get",
  "env_create",
  "env_patch",
  "env_delete",
  "env_start",
  "env_stop",
  "env_restart",
  "env_reallocate",
  "env_logs",
  "env_captures",
  "env_capture_detail",
  "env_capture_clear",
  "env_captures_export",
  "debug_get",
  "debug_start",
  "debug_stop",
  "har_list",
  "har_import",
  "har_get",
  "har_delete",
  "rules_list",
  "rules_import",
  "rules_get",
  "rules_delete",
  "proxies_list",
  "proxies_put",
  "proxies_delete",
  "inject_fault",
  "watch_environments",
  "watch_debug",
  "unwatch",
];

export interface ChannelLike {
  onmessage: ((message: never) => void) | null;
}

type Args = Record<string, unknown>;

/* ---------------- 状态 ---------------- */

let envs: EnvState[] = seedEnvs();
const rules: Map<string, string> = seedRules();
const proxies: Map<string, ProxyState> = seedProxies();
let ca: CaOverview = seedCa();
const harSessions: HarSession[] = seedHar();
let nextHarId = 2;
let debugEnv: string | null = null;
let trafficCursor = 0;

let envCursor = 0;
let debugCursor = 0;
let nextStreamId = 1;

interface StreamReg {
  id: number;
  kind: "env" | "debug";
  channel: ChannelLike;
  timer: number;
}
const streams: Map<number, StreamReg> = new Map();

/* ---------------- 工具 ---------------- */

function fail(code: string, message: string, field?: string): never {
  throw field === undefined ? { code, message } : { code, message, field };
}

function str(args: Args, key: string): string {
  const value = args[key];
  if (typeof value !== "string" || value.length === 0) {
    fail("invalid_config", `missing or invalid '${key}'`, key);
  }
  return value;
}

function findEnv(name: string): EnvState {
  const env = envs.find((e) => e.name === name);
  if (!env) fail("not_found", `no environment named '${name}'`, "name");
  return env;
}

function countRules(text: string): number {
  return text
    .split("\n")
    .filter((line) => line.trim().length > 0 && !line.trim().startsWith("#")).length;
}

function view(env: EnvState): EnvView {
  const bound = env.rules !== null ? rules.get(env.rules) : undefined;
  return {
    name: env.name,
    listen: { host: env.host, port: env.port },
    rules: env.rules,
    upstream: env.upstream,
    insecure_hosts: [...env.insecure_hosts],
    mitm_hosts: [...env.mitm_hosts],
    description: env.description,
    desired: env.desired,
    health: env.health,
    health_reason: env.health_reason,
    rules_count: bound === undefined ? 0 : countRules(bound),
    rules_missing: env.rules !== null && bound === undefined,
    proxy_command: `export https_proxy=http://127.0.0.1:${env.port}`,
    proxy_auth_enabled: env.proxyUser !== null && env.proxyUser.length > 0,
    capture: env.capture,
  };
}

function proxyView(p: ProxyState): ProxyView {
  return {
    name: p.name,
    host: p.host,
    port: p.port,
    has_auth: p.user !== null && p.user.length > 0,
    references: envs.filter((e) => e.upstream === p.name).map((e) => e.name),
  };
}

function usedPorts(except?: string): Set<number> {
  return new Set(envs.filter((e) => e.name !== except).map((e) => e.port));
}

function allocatePort(except?: string): number {
  const used = usedPorts(except);
  for (let port = PORT_RANGE[0]; port <= PORT_RANGE[1]; port += 1) {
    if (!used.has(port)) return port;
  }
  fail("port_conflict", `no free port in ${PORT_RANGE[0]}-${PORT_RANGE[1]}`, "listen.port");
}

function captureView(env: EnvState, limit = 100): CaptureView {
  return {
    session: { id: env.sessionId, started_at: env.sessionStartedAt, generation: env.generation },
    captured: env.captured,
    dropped: env.dropped,
    records: structuredClone(env.records.slice(-limit)),
  };
}

function debugView(): DebugView {
  if (debugEnv === null) return { env: null, capture: null };
  const env = envs.find((e) => e.name === debugEnv);
  return { env: debugEnv, capture: env ? captureView(env) : null };
}

/* ---------------- 推送 ---------------- */

function emit(channel: ChannelLike, event: string, data: unknown): void {
  channel.onmessage?.({ event, data } as never);
}

// 推送一律走异步宏任务：真 IPC 的 Channel 帧绝不会在命令调用栈内同步送达
// （Rust 侧是独立轮询任务）。同步推帧会让「命令响应 + 快照帧」两次集合重建
// 撞进同一个 commit 窗口，react-aria 按索引重排 key 时抛 "Cannot change the
// id of an item" 白屏——mock 必须复现真传输的时序边界，而不是比它更急。
function notifyEnv(): void {
  window.setTimeout(() => {
    envCursor += 1;
    const frame = { ok: true, cursor: envCursor, environments: envs.map(view) };
    for (const reg of streams.values()) {
      if (reg.kind === "env") emit(reg.channel, "snapshot", frame);
    }
  }, 0);
}

function notifyDebugSnapshot(): void {
  window.setTimeout(() => {
    debugCursor += 1;
    const frame = { cursor: debugCursor, ...debugView() };
    for (const reg of streams.values()) {
      if (reg.kind === "debug") emit(reg.channel, "snapshot", frame);
    }
  }, 0);
}

function makeRecord(env: EnvState): CaptureRecord {
  const spec = TRAFFIC_POOL[trafficCursor % TRAFFIC_POOL.length];
  trafficCursor += 1;
  const requestId = env.nextRequestId;
  env.nextRequestId += 1;
  return {
    version: 1,
    session: env.sessionId,
    request_id: requestId,
    time: Date.now(),
    request: {
      method: spec.method,
      path: spec.path,
      authority: spec.authority,
      headers: [
        ["host", spec.authority],
        ["user-agent", "envboard-mock/0.2.0"],
      ],
      body:
        spec.method === "POST"
          ? { encoding: "utf8", size: 27, content: '{"probe":"envboard-mock"}' }
          : null,
    },
    response: {
      status: spec.status,
      headers: [["content-type", "application/json"]],
      body: { encoding: "utf8", size: 19, content: `{"status":${spec.status},"ok":true}` },
    },
    error: null,
  };
}

function debugTick(): void {
  if (debugEnv === null) return;
  const env = envs.find((e) => e.name === debugEnv);
  if (!env || !env.capture) return;
  const record = makeRecord(env);
  env.records.push(record);
  env.captured += 1;
  if (env.records.length > 500) {
    env.records.shift();
    env.dropped += 1;
  }
  debugCursor += 1;
  const frame = {
    cursor: debugCursor,
    records: [record],
    captured: env.captured,
    dropped: env.dropped,
  };
  for (const reg of streams.values()) {
    if (reg.kind === "debug") emit(reg.channel, "events", frame);
  }
}

/** 登记一条流（watch_environments / watch_debug），返回流 id。 */
export function registerStream(command: string, channel: ChannelLike): number {
  const id = nextStreamId;
  nextStreamId += 1;
  const kind = command === "watch_environments" ? "env" : "debug";
  const timer = window.setInterval(
    () => {
      if (kind === "env") notifyEnv();
      else debugTick();
    },
    kind === "env" ? 1000 : 700,
  );
  streams.set(id, { id, kind, channel, timer });
  if (kind === "env") notifyEnv();
  else notifyDebugSnapshot();
  return id;
}

/* ---------------- 命令分发 ---------------- */

export function dispatch(command: string, args: Args): unknown {
  switch (command) {
    case "status": {
      const info: StatusInfo = {
        version: "0.2.0",
        core: { name: "envboard-engine", version: "0.2.0" },
        capabilities: { capture: true, mitm: true, upstream: true, rules: true, mock: true },
        config: { state_dir: "(mock) in-memory", port_range: `${PORT_RANGE[0]}-${PORT_RANGE[1]}` },
        environments: envs.length,
        running: envs.filter((e) => e.health === "running").length,
        events_dropped: 0,
      };
      return info;
    }

    case "ui_prefs_get":
      return window.localStorage.getItem("envboard-mock-theme");
    case "ui_prefs_set":
      window.localStorage.setItem("envboard-mock-theme", str(args, "theme"));
      return null;

    case "ca_overview":
      return structuredClone(ca);
    case "ca_install_cert":
      ca = {
        ...ca,
        trust: { ...ca.trust, current_user: "installed", installed: true, detail: "mock：已装入当前用户 ROOT（未碰真存储）" },
      };
      return { ok: true };
    case "ca_uninstall_cert": {
      const removed = ca.trust.current_user === "installed";
      ca = {
        ...ca,
        trust: { ...ca.trust, current_user: "missing", installed: false, detail: "mock：已从当前用户 ROOT 移除" },
      };
      return { ok: true, removed, warning: null };
    }

    case "env_list":
      return envs.map(view);
    case "env_get":
      return view(findEnv(str(args, "name")));
    case "env_create": {
      const body = (args.body ?? {}) as EnvCreateReq;
      if (typeof body.name !== "string" || body.name.trim().length === 0) {
        fail("invalid_config", "environment name must not be empty", "name");
      }
      if (envs.some((e) => e.name === body.name)) {
        fail("conflict", `environment '${body.name}' already exists`, "name");
      }
      let port: number;
      if (body.listen?.port !== undefined) {
        port = body.listen.port;
        if (usedPorts().has(port)) {
          fail("port_conflict", `port ${port} is already allocated`, "listen.port");
        }
      } else {
        port = allocatePort();
      }
      const env: EnvState = {
        name: body.name,
        host: body.listen?.host ?? "127.0.0.1",
        port,
        rules: body.rules ?? null,
        upstream: body.upstream ?? null,
        insecure_hosts: body.insecure_hosts ?? [],
        mitm_hosts: body.mitm_hosts ?? [],
        description: body.description ?? "",
        desired: "stopped",
        health: "stopped",
        health_reason: null,
        capture: body.capture ?? false,
        proxyUser: body.proxy_user ?? null,
        proxyPassword: body.proxy_password ?? null,
        logs: [`INFO  created (mock): port ${port}`],
        records: [],
        sessionId: Math.max(0, ...envs.map((e) => e.sessionId)) + 1,
        sessionStartedAt: Date.now(),
        generation: 1,
        captured: 0,
        dropped: 0,
        nextRequestId: 1,
      };
      envs.push(env);
      notifyEnv();
      return view(env);
    }
    case "env_patch": {
      const env = findEnv(str(args, "name"));
      const patch = (args.patch ?? {}) as EnvPatchReq;
      const stopFields = (["listen", "proxy_user", "proxy_password"] as const).filter(
        (key) => patch[key] !== undefined,
      );
      if (env.desired === "running" && stopFields.length > 0) {
        fail(
          "conflict",
          `fields require a stopped environment: ${stopFields.join(", ")}`,
          stopFields[0],
        );
      }
      if (patch.listen?.port !== undefined && usedPorts(env.name).has(patch.listen.port)) {
        fail("port_conflict", `port ${patch.listen.port} is already allocated`, "listen.port");
      }
      if (patch.listen?.host !== undefined) env.host = patch.listen.host;
      if (patch.listen?.port !== undefined) env.port = patch.listen.port;
      if (patch.rules !== undefined) env.rules = patch.rules;
      if (patch.upstream !== undefined) env.upstream = patch.upstream;
      if (patch.description !== undefined) env.description = patch.description;
      if (patch.insecure_hosts !== undefined) env.insecure_hosts = [...patch.insecure_hosts];
      if (patch.mitm_hosts !== undefined) env.mitm_hosts = [...patch.mitm_hosts];
      if (patch.capture !== undefined) env.capture = patch.capture;
      if (patch.proxy_user !== undefined) env.proxyUser = patch.proxy_user;
      if (patch.proxy_password !== undefined) env.proxyPassword = patch.proxy_password;
      env.logs.push(`INFO  patched (mock): ${Object.keys(patch).join(", ") || "no-op"}`);
      notifyEnv();
      return view(env);
    }
    case "env_delete": {
      const name = str(args, "name");
      findEnv(name);
      envs = envs.filter((e) => e.name !== name);
      if (debugEnv === name) debugEnv = null;
      notifyEnv();
      notifyDebugSnapshot();
      return { removed: name };
    }
    case "env_start": {
      const env = findEnv(str(args, "name"));
      env.desired = "running";
      const clash = envs.find(
        (e) => e.name !== env.name && e.port === env.port && e.health === "running",
      );
      if (clash) {
        env.health = "port_conflict";
        env.health_reason = `port ${env.port} held by running '${clash.name}'`;
      } else {
        env.health = "running";
        env.health_reason = null;
      }
      env.logs.push(`INFO  start (mock): ${env.health}`);
      notifyEnv();
      return view(env);
    }
    case "env_stop": {
      const env = findEnv(str(args, "name"));
      env.desired = "stopped";
      env.health = "stopped";
      env.health_reason = null;
      env.logs.push("INFO  stop (mock)");
      notifyEnv();
      return view(env);
    }
    case "env_restart": {
      const env = findEnv(str(args, "name"));
      env.desired = "running";
      env.health = "running";
      env.health_reason = null;
      env.generation += 1;
      env.logs.push("INFO  restart (mock): recovered");
      notifyEnv();
      return view(env);
    }
    case "env_reallocate": {
      const env = findEnv(str(args, "name"));
      env.port = allocatePort(env.name);
      env.logs.push(`INFO  reallocated (mock): port ${env.port}`);
      notifyEnv();
      return view(env);
    }
    case "env_logs": {
      const env = findEnv(str(args, "name"));
      const lines = typeof args.lines === "number" ? args.lines : 200;
      return { env: env.name, lines: env.logs.slice(-lines) };
    }
    case "env_captures": {
      const env = findEnv(str(args, "name"));
      const limit = typeof args.limit === "number" ? args.limit : 100;
      return captureView(env, limit);
    }
    case "env_capture_detail": {
      const env = findEnv(str(args, "name"));
      const record = env.records.find((r) => r.request_id === args.requestId);
      if (!record) fail("not_found", `no capture record ${String(args.requestId)}`, "request_id");
      return structuredClone(record);
    }
    case "env_capture_clear": {
      const env = findEnv(str(args, "name"));
      env.records = [];
      env.captured = 0;
      env.dropped = 0;
      env.generation += 1;
      notifyDebugSnapshot();
      return { ok: true };
    }
    case "env_captures_export": {
      const env = findEnv(str(args, "name"));
      const format = args.format === "jsonl" ? "jsonl" : "har";
      if (format === "jsonl") {
        return {
          filename: `${env.name}-${env.sessionId}.jsonl`,
          text: env.records.map((r) => JSON.stringify(r)).join("\n"),
        };
      }
      const entries = env.records.map((r) => ({
        startedDateTime: new Date(r.time).toISOString(),
        time: 1,
        request: {
          method: r.request.method,
          url: `https://${r.request.authority}${r.request.path}`,
          httpVersion: "HTTP/2.0",
          headers: r.request.headers.map(([name, value]) => ({ name, value })),
          queryString: [],
          headersSize: -1,
          bodySize: -1,
        },
        response: {
          status: r.response?.status ?? 0,
          statusText: "",
          httpVersion: "HTTP/2.0",
          headers: (r.response?.headers ?? []).map(([name, value]) => ({ name, value })),
          content: { size: 19, mimeType: "application/json", text: "{\"ok\":true}" },
          redirectURL: "",
          headersSize: -1,
          bodySize: -1,
        },
        cache: {},
        timings: { send: 0, wait: 1, receive: 0 },
      }));
      return {
        filename: `${env.name}-${env.sessionId}.har`,
        text: JSON.stringify(
          { log: { version: "1.2", creator: { name: "envboard-mock", version: "0.2.0" }, entries } },
          null,
          2,
        ),
      };
    }

    case "debug_get":
      return debugView();
    case "debug_start": {
      const name = str(args, "env");
      if (debugEnv !== null) {
        fail("conflict", `debug session already active on '${debugEnv}'`, "env");
      }
      const env = findEnv(name);
      env.capture = true;
      env.generation += 1;
      debugEnv = name;
      notifyEnv();
      notifyDebugSnapshot();
      return debugView();
    }
    case "debug_stop":
      debugEnv = null;
      notifyDebugSnapshot();
      return { ok: true };

    case "har_list":
      return {
        sessions: harSessions.map((s) => ({
          id: s.id,
          name: s.name,
          imported_at: s.importedAt,
          captured: s.records.length,
        })),
      };
    case "har_import": {
      const body = (args.body ?? {}) as { log?: { entries?: unknown[] } };
      const id = nextHarId;
      nextHarId += 1;
      const session: HarSession = {
        id,
        name: typeof args.name === "string" && args.name.length > 0 ? args.name : `import-${id}`,
        importedAt: Date.now(),
        records: [],
      };
      const entries = body.log?.entries;
      if (Array.isArray(entries)) {
        session.records = entries.slice(0, 500).map((entry, index) => {
          const e = entry as { request?: { method?: string; url?: string }; response?: { status?: number } };
          let authority = "imported.example.com";
          let path = "/";
          try {
            const url = new URL(e.request?.url ?? "https://imported.example.com/");
            authority = url.host;
            path = url.pathname + url.search;
          } catch {
            /* 形状宽容：坏 URL 不挡导入 */
          }
          return {
            version: 1,
            session: 900 + id,
            request_id: index + 1,
            time: session.importedAt,
            request: {
              method: e.request?.method ?? "GET",
              path,
              authority,
              headers: [["host", authority]],
              body: null,
            },
            response: { status: e.response?.status ?? 200, headers: [], body: null },
            error: null,
          };
        });
      }
      harSessions.push(session);
      return {
        session: { id: 900 + id, started_at: session.importedAt, generation: 1 },
        captured: session.records.length,
        dropped: 0,
        records: structuredClone(session.records),
      };
    }
    case "har_get": {
      const session = harSessions.find((s) => s.id === args.id);
      if (!session) fail("not_found", `no imported HAR session ${String(args.id)}`, "id");
      const limit = typeof args.limit === "number" ? Math.min(Math.max(args.limit, 1), 5000) : 100;
      return {
        session: { id: 900 + session.id, started_at: session.importedAt, generation: 1 },
        captured: session.records.length,
        dropped: 0,
        records: structuredClone(session.records.slice(0, limit)),
      };
    }
    case "har_delete": {
      const before = harSessions.length;
      const index = harSessions.findIndex((s) => s.id === args.id);
      if (index >= 0) harSessions.splice(index, 1);
      return { ok: harSessions.length < before };
    }

    case "rules_list":
      return { rules: [...rules.keys()].sort() };
    case "rules_get": {
      const name = str(args, "name");
      const text = rules.get(name);
      if (text === undefined) fail("not_found", `no rule set named '${name}'`, "name");
      return { name, text };
    }
    case "rules_import": {
      const name = str(args, "name");
      const text = str(args, "text");
      rules.set(name, text);
      const summary: RuleSummary = { name, path: `(mock) state-dir/rules/${name}.rules` };
      notifyEnv();
      return summary;
    }
    case "rules_delete": {
      const name = str(args, "name");
      if (!rules.delete(name)) fail("not_found", `no rule set named '${name}'`, "name");
      notifyEnv();
      return { removed: name };
    }

    case "proxies_list":
      return [...proxies.values()].map(proxyView);
    case "proxies_put": {
      const body = (args.body ?? {}) as ProxyPutReq;
      if (typeof body.name !== "string" || body.name.trim().length === 0) {
        fail("invalid_config", "proxy name must not be empty", "name");
      }
      if (typeof body.host !== "string" || body.host.trim().length === 0) {
        fail("invalid_config", "proxy host must not be empty", "host");
      }
      const existing = proxies.get(body.name);
      const state: ProxyState = {
        name: body.name,
        host: body.host,
        port: body.port ?? existing?.port ?? 8080,
        user: body.user !== undefined ? body.user : (existing?.user ?? null),
        password: body.password !== undefined ? body.password : (existing?.password ?? null),
      };
      proxies.set(body.name, state);
      notifyEnv();
      return proxyView(state);
    }
    case "proxies_delete": {
      const name = str(args, "name");
      const state = proxies.get(name);
      if (!state) fail("not_found", `no proxy named '${name}'`, "name");
      const refs = proxyView(state).references;
      if (refs.length > 0) {
        fail("conflict", `proxy '${name}' is referenced by: ${refs.join(", ")}`, "name");
      }
      proxies.delete(name);
      return { removed: name };
    }

    case "inject_fault": {
      const env = findEnv(str(args, "env"));
      env.health = "failed";
      env.health_reason =
        typeof args.reason === "string" && args.reason.length > 0
          ? args.reason
          : "injected fault (mock)";
      env.logs.push(`ERROR fault injected (mock): ${env.health_reason}`);
      notifyEnv();
      return view(env);
    }

    case "unwatch": {
      const reg = streams.get(args.id as number);
      if (reg) {
        window.clearInterval(reg.timer);
        streams.delete(reg.id);
      }
      return null;
    }

    default:
      fail("unknown", `mock engine has no command '${command}'`);
  }
}

/**
 * envboard 工作台 · 展示层词汇与派生 helper。
 *
 * 业务数据的唯一来源是 Admin API（src/api/）；本文件只放**展示规则**：
 * 视图路由键、健康字面量的中文与色调、字节格式化、活动事件的域归类与文案。
 * （设计规范 D-10：展示层不持有业务数据。）
 */

import type { EnvView } from "./api/types";

/* ---------------- 视图路由 ---------------- */

/**
 * 两视图：证书独立成页；规则库 + 上游代理并入环境页内页签；
 * 抓包日志是环境子页（不在 ViewKey 里，由 EnvironmentsView 内部切换）；
 * 对比/活动/调试页已退场（ADR-007/011）。
 * 加视图必须 data.ts / SideBar NAV / Workbench 分支三处同改（artifact 测试钉死）。
 */
export type ViewKey = "environments" | "cert";

/** 环境页内的子页签（不是路由视图）。 */
export type EnvSection = "instances" | "rules" | "proxies";

/* ---------------- 健康态 ---------------- */

/** 健康字面量（envboard-protocol crate 文档：starting/running/failed/stopped/port_conflict/unhealthy）。 */
export const HEALTH_LABEL: Record<string, string> = {
  starting: "启动中",
  running: "运行中",
  failed: "失败",
  stopped: "已停止",
  port_conflict: "端口冲突",
  unhealthy: "不健康",
};

export type HealthTone = "success" | "warning" | "danger" | "muted";

export function healthTone(health: string): HealthTone {
  if (health === "running") return "success";
  if (health === "starting") return "warning";
  if (health === "failed" || health === "port_conflict") return "danger";
  return "muted";
}

export function healthLabel(health: string): string {
  return HEALTH_LABEL[health] ?? health;
}

/** 期望与实际不一致（含 failed/unhealthy/port_conflict）= 需处理。 */
export function envNeedsAttention(env: EnvView): boolean {
  if (env.health === "failed" || env.health === "unhealthy" || env.health === "port_conflict") {
    return true;
  }
  return env.health !== env.desired;
}

/* ---------------- 派生文本 ---------------- */

export function listenAddress(env: EnvView): string {
  return `${env.listen.host}:${env.listen.port}`;
}

/** 绑定 0.0.0.0 = 局域网可达（概览「对外服务」）。 */
export function isPublicListen(env: EnvView): boolean {
  return env.listen.host !== "127.0.0.1" && env.listen.host !== "::1";
}

/** 端口区间展示：/api/status 回显字符串；兼容 tuple/object 历史形态。 */
export function formatPortRange(
  p: string | [number, number] | { start: number; end: number } | undefined,
): string {
  if (!p) return "…";
  if (typeof p === "string") return p;
  return Array.isArray(p) ? `${p[0]}-${p[1]}` : `${p.start}-${p.end}`;
}

export function formatBytes(size: number): string {
  if (size === 0) return "0 B";
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  return `${(size / 1024 / 1024).toFixed(1)} MB`;
}

/* 活动页已退场（ADR-007）：事件着色/文案 helper 随垂直 slice 一并删除；
   后端审计账本（events.jsonl）与 facade history 保留。 */

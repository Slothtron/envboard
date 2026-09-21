/**
 * envboard 工作台 · 展示层词汇与派生 helper。
 *
 * 业务数据的唯一来源是 Admin API（src/api/）；本文件只放**展示规则**：
 * 视图路由键、健康字面量的中文与色调、字节格式化、活动事件的域归类与文案。
 * （设计规范 D-10：展示层不持有业务数据。）
 */

import type { ControlEvent, EnvView } from "./api/types";

/* ---------------- 视图路由 ---------------- */

export type ViewKey =
  | "environments"
  | "rules"
  | "proxies"
  | "debug"
  | "compare"
  | "activity"
  | "settings";

/* ---------------- 健康态 ---------------- */

/** 健康字面量（spec/protocol.md：starting/running/failed/stopped/port_conflict/unhealthy）。 */
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

export function formatBytes(size: number): string {
  if (size === 0) return "0 B";
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  return `${(size / 1024 / 1024).toFixed(1)} MB`;
}

/* ---------------- 活动事件（控制面审计，spec/events.md 词汇表） ---------------- */

export type ActivityDomain = "env" | "rules" | "engine" | "instance" | "other";

/** 事件按域归类（活动视图的筛选 chips 用，单一入口）。未知类型归「其他」。 */
export function activityDomain(type: string): ActivityDomain {
  if (type.startsWith("environment/")) return "env";
  if (type.startsWith("rules/")) return "rules";
  if (type.startsWith("engine/")) return "engine";
  if (type.startsWith("instance/")) return "instance";
  return "other";
}

/**
 * 事件的语义着色只表达后果：
 * rejected / deleted = danger，stopped = warning，其余 default（不用色彩装饰正常事件）。
 */
export function activityTone(ev: ControlEvent): "default" | "warning" | "danger" {
  if (
    ev.type === "engine/rejected" ||
    ev.type === "environment/deleted" ||
    ev.type === "rules/deleted"
  ) {
    return "danger";
  }
  if (ev.type === "instance/stopped") return "warning";
  return "default";
}

/** 从 data 取该事件的对象名（环境名 / 规则集名）；没有则 null。 */
export function activityTarget(ev: ControlEvent): string | null {
  const name =
    (ev.data.name as string | undefined) ?? (ev.data.rules_name as string | undefined);
  return name ?? null;
}

/** 事件的人类文案（派生而非存储字段）。 */
export function activityLabel(ev: ControlEvent): string {
  switch (ev.type) {
    case "environment/created":
      return "创建环境";
    case "environment/updated":
      return "更新环境";
    case "environment/deleted":
      return "删除环境";
    case "rules/imported":
      return "导入规则集";
    case "rules/deleted":
      return "删除规则集";
    case "engine/applied":
      return "热装配生效";
    case "engine/rejected":
      return "配置被整套拒绝（旧快照继续服务）";
    case "instance/started":
      return "启动实例";
    case "instance/stopped":
      return "停止实例";
    case "instance/reconciled":
      return "对账收敛";
    case "custom":
      return String(ev.data.kind ?? "custom");
    default:
      return ev.type;
  }
}

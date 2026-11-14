import type { ReactNode } from "react";
import { Alert, Button, Chip, Separator, Skeleton, Spinner, Surface } from "@heroui/react";

import type { EnvView } from "./api/types";
import { healthLabel, healthTone } from "./data";
import { IconAlert, IconRefresh, IconTray } from "./icons";

/* 页内分段控件的 tab 样式（宽度随文字、选中实心，区别于主导航下划线）。
   用 data- 工具类保证生效（不依赖被 HeroUI 工具类压过的自定义层）。 */
export const SEG_TAB =
  "rounded px-3 py-1 text-sm text-muted data-[selected=true]:bg-surface data-[selected=true]:font-semibold data-[selected=true]:text-foreground";

/* 唯一状态组件（方向A ⑬）：点 + 字 + 语义色，列表/抽屉/顶栏/状态栏同源。 */
const STATUS_LED: Record<string, string> = {
  success: "ph-led--ok",
  warning: "ph-led--warn",
  danger: "ph-led--err",
  muted: "ph-led--idle",
};

export function StatusBadge({ health, className }: { health: EnvView["health"]; className?: string }) {
  const led = STATUS_LED[healthTone(health)] ?? "ph-led--idle";
  return (
    <span className={`inline-flex items-center gap-1.5 whitespace-nowrap ${className ?? ""}`}>
      <span aria-hidden="true" className={`ph-led ${led}`} />
      <span className="text-sm text-foreground">{healthLabel(health)}</span>
    </span>
  );
}

/* ------------------------------------------------------------------ */
/* 分区标题（替代原 HTML 的 .section-title：分组先于平铺）              */
/* ------------------------------------------------------------------ */

export function SectionTitle({ children, trailing }: { children: ReactNode; trailing?: ReactNode }) {
  return (
    <div className="wb-section flex items-center gap-2 pb-2">
      <h3 className="text-sm font-semibold uppercase tracking-wide text-muted">{children}</h3>
      <Separator className="flex-1" />
      {trailing}
    </div>
  );
}

/* ------------------------------------------------------------------ */
/* 键值块（概览分组信息）                                              */
/* ------------------------------------------------------------------ */

export type KvTone = "default" | "warning" | "danger" | "muted";

const KV_TONE: Record<KvTone, string> = {
  default: "text-foreground",
  warning: "text-warning",
  danger: "text-danger",
  muted: "text-muted",
};

export function KvGrid({ children }: { children: ReactNode }) {
  return <div className="wb-kv-grid">{children}</div>;
}

export function Kv({
  k,
  v,
  tone = "default",
  alert = false,
}: {
  k: string;
  v: string;
  tone?: KvTone;
  alert?: boolean;
}) {
  return (
    <Surface
      className={`flex min-w-0 flex-col gap-1 p-3 ${alert ? "border border-danger/30 bg-danger-soft" : ""}`}
      variant={alert ? "default" : "secondary"}
    >
      <span className="text-sm text-muted">{k}</span>
      <span className={`truncate font-mono text-base ${KV_TONE[tone]}`} title={v}>
        {v}
      </span>
    </Surface>
  );
}

/* ------------------------------------------------------------------ */
/* 四态：loading / empty / error / disabled                            */
/* ------------------------------------------------------------------ */

export function TableSkeleton({ rows = 5 }: { rows?: number }) {
  return (
    <div className="flex flex-col gap-3 p-4">
      <Skeleton className="h-6 w-full rounded" />
      {Array.from({ length: rows }).map((_, i) => (
        <Skeleton key={i} className="h-10 w-full rounded" />
      ))}
    </div>
  );
}

export function EmptyState({
  icon,
  title,
  hint,
  action,
}: {
  icon?: ReactNode;
  title: string;
  hint: string;
  action?: ReactNode;
}) {
  return (
    <div className="flex flex-col items-center justify-center gap-3 p-6 text-center">
      <span className="grid size-11 place-items-center rounded-lg bg-surface-secondary text-muted">
        {icon ?? <IconTray className="size-5" />}
      </span>
      <p className="text-base font-semibold text-foreground">{title}</p>
      <p className="max-w-md text-sm leading-relaxed text-muted">{hint}</p>
      {action}
    </div>
  );
}

export function ErrorState({
  title,
  description,
  onRetry,
  isRetrying = false,
}: {
  title: string;
  description: string;
  onRetry?: () => void;
  isRetrying?: boolean;
}) {
  return (
    <Alert className="m-4" status="danger">
      <Alert.Indicator />
      <Alert.Content>
        <Alert.Title>{title}</Alert.Title>
        <Alert.Description>{description}</Alert.Description>
      </Alert.Content>
      {onRetry ? (
        <Button size="sm" variant="secondary" isPending={isRetrying} onPress={onRetry}>
          {isRetrying ? null : <IconRefresh className="size-4" />}
          重试
        </Button>
      ) : null}
    </Alert>
  );
}

/** 瞬时阻塞：按钮侧用 isPending，区域级用 Spinner */
export function BlockingSpinner({ label = "加载中" }: { label?: string }) {
  return (
    <div className="flex items-center justify-center gap-2 p-6 text-sm text-muted">
      <Spinner size="sm" />
      {label}
    </div>
  );
}

/** 页内反馈区（替代 Toast —— Toast 不在冻结清单内） */
export function FeedbackAlert({
  status,
  title,
  detail,
  onClose,
}: {
  status: "success" | "warning" | "danger";
  title: string;
  /** 错误原文层（[code] message · field）：不翻译、不截断（envboard-engine::api::error 模块文档）。 */
  detail?: string;
  onClose: () => void;
}) {
  return (
    <Alert status={status}>
      <Alert.Indicator />
      <Alert.Content>
        <Alert.Title>{title}</Alert.Title>
        {detail ? <Alert.Description>{detail}</Alert.Description> : null}
      </Alert.Content>
      <Button size="sm" variant="ghost" onPress={onClose}>
        关闭
      </Button>
    </Alert>
  );
}

/* ------------------------------------------------------------------ */
/* 筛选按钮：激活态用 secondary，避免占用该屏唯一的主操作配额            */
/* ------------------------------------------------------------------ */

export function FilterButton({
  isActive,
  count,
  onPress,
  children,
}: {
  isActive: boolean;
  count?: number;
  onPress: () => void;
  children: ReactNode;
}) {
  return (
    <Button
      aria-pressed={isActive}
      size="sm"
      variant={isActive ? "secondary" : "ghost"}
      onPress={onPress}
    >
      {children}
      {typeof count === "number" ? (
        <span className="font-mono text-sm opacity-75">{count}</span>
      ) : null}
    </Button>
  );
}

/* ------------------------------------------------------------------ */
/* 抓包：方法徽标与状态码着色（语义色贯穿列表）                          */
/* ------------------------------------------------------------------ */

const METHOD_COLOR: Record<string, "default" | "success" | "accent" | "warning" | "danger"> = {
  GET: "default",
  POST: "default",
  PUT: "warning",
  DELETE: "danger",
};

export function MethodChip({ method }: { method: string }) {
  return (
    /* tertiary（描边）变体：与状态码的 soft 填充 Chip 拉开层级，避免同色误读 */
    <Chip color={METHOD_COLOR[method] ?? "default"} size="sm" variant="tertiary">
      <Chip.Label>{method}</Chip.Label>
    </Chip>
  );
}

export function statusTone(status: number): string {
  // 0 = ERR/缺响应：保留错误信号，不静默变灰。
  if (status === 0 || status >= 500) return "font-mono font-semibold text-danger";
  if (status >= 400) return "font-mono font-semibold text-warning";
  if (status >= 300) return "font-mono font-semibold text-muted";
  // 1xx/2xx：正常回执用正文色，绿色只留给健康态。
  return "font-mono font-semibold text-foreground";
}

/* ------------------------------------------------------------------ */
/* 灰条提示（未保存改动 / 安全告警）                                    */
/* ------------------------------------------------------------------ */

export function InlineNotice({
  status,
  children,
  action,
}: {
  status: "success" | "warning" | "danger" | "accent";
  children: ReactNode;
  action?: ReactNode;
}) {
  return (
    <Alert className="mb-4" status={status}>
      <Alert.Indicator>
        <IconAlert className="size-4" />
      </Alert.Indicator>
      <Alert.Content>
        <Alert.Description>{children}</Alert.Description>
      </Alert.Content>
      {action}
    </Alert>
  );
}

/** 单行说明文本 */
export function Hint({ children }: { children: ReactNode }) {
  return <p className="text-sm leading-relaxed text-muted">{children}</p>;
}

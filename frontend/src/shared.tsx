import type { ReactNode } from "react";
import { Alert, Button, Chip, Separator, Skeleton, Spinner, Surface } from "@heroui/react";

import { IconAlert, IconRefresh, IconTray } from "./icons";

/* ------------------------------------------------------------------ */
/* 分区标题（替代原 HTML 的 .section-title：分组先于平铺）              */
/* ------------------------------------------------------------------ */

export function SectionTitle({ children, trailing }: { children: ReactNode; trailing?: ReactNode }) {
  return (
    <div className="flex items-center gap-2 pb-2">
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
  onClose,
}: {
  status: "success" | "warning" | "danger";
  title: string;
  onClose: () => void;
}) {
  return (
    <Alert status={status}>
      <Alert.Indicator />
      <Alert.Content>
        <Alert.Title>{title}</Alert.Title>
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

const METHOD_COLOR: Record<string, "success" | "accent" | "warning" | "danger"> = {
  GET: "success",
  POST: "accent",
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
  if (status >= 500) return "font-mono font-semibold text-danger";
  if (status >= 400) return "font-mono font-semibold text-warning";
  if (status >= 300) return "font-mono font-semibold text-accent";
  return "font-mono font-semibold text-success";
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

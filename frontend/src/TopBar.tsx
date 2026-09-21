import { Button, Chip, Separator, Tooltip } from "@heroui/react";

import { IconRefresh } from "./icons";
import { FilterButton } from "./shared";

export type MetricKey = "all" | "running" | "issues" | "stopped";

const DOT_TONE: Record<MetricKey, string> = {
  all: "bg-foreground",
  running: "bg-success",
  issues: "bg-warning",
  stopped: "bg-muted",
};

const METRICS: Array<{ key: MetricKey; label: string }> = [
  { key: "all", label: "全部" },
  { key: "running", label: "运行中" },
  { key: "issues", label: "需处理" },
  { key: "stopped", label: "已停止" },
];

interface TopBarProps {
  counts: Record<MetricKey, number>;
  metric: MetricKey;
  /** 引擎 core 版本（GET /api/status）；加载中为「…」 */
  coreVersion: string;
  onMetricChange: (key: MetricKey) => void;
  onRefresh: () => void;
  isRefreshing: boolean;
  /** 事件流连接态；false 时徽章转危险色 */
  streamConnected: boolean;
}

export function TopBar({
  counts,
  metric,
  coreVersion,
  onMetricChange,
  onRefresh,
  isRefreshing,
  streamConnected,
}: TopBarProps) {
  return (
    <header className="flex min-w-0 items-center gap-4 border-b border-border bg-surface px-5 py-3">
      {/* 品牌 */}
      <div className="flex flex-none items-center gap-3">
        <span
          aria-hidden="true"
          className="grid size-8 place-items-center rounded-lg bg-accent text-lg font-bold text-accent-foreground"
        >
          e
        </span>
        <span className="flex flex-col leading-tight">
          <span className="text-lg font-bold tracking-wide text-foreground">envboard</span>
          <span className="text-sm text-muted">环境代理工作台</span>
        </span>
      </div>

      <Separator orientation="vertical" className="h-6" />

      {/* 重构点 P2：顶栏健康总览取代首屏四张统计卡；点击即筛选列表，反馈就地 */}
      <div
        aria-label="全局概览（点击筛选列表）"
        className="flex flex-none items-center gap-2"
        role="group"
      >
        {METRICS.map((m) => (
          <FilterButton
            key={m.key}
            count={counts[m.key]}
            isActive={metric === m.key}
            onPress={() => onMetricChange(m.key)}
          >
            {m.key === "all" ? null : (
              <span aria-hidden="true" className={`size-2 rounded-full ${DOT_TONE[m.key]}`} />
            )}
            {m.label}
          </FilterButton>
        ))}
      </div>

      <div className="ml-auto flex min-w-0 items-center gap-2">
        {/* Tooltip.Trigger 要求可按压子元素；Chip 非交互，故版本徽章不套 Tooltip（避免 PressResponder 警告） */}
        <Chip color="success" size="sm" variant="soft">
          <span aria-hidden="true" className="size-2 rounded-full bg-success" />
          <Chip.Label>core {coreVersion}</Chip.Label>
        </Chip>
        <Chip color={streamConnected ? "success" : "danger"} size="sm" variant="soft">
          <span
            aria-hidden="true"
            className={`size-2 rounded-full ${streamConnected ? "bg-success" : "bg-danger"}`}
          />
          <Chip.Label>{streamConnected ? "事件流已连接" : "事件流已断开"}</Chip.Label>
        </Chip>

        <Tooltip delay={200}>
          <Tooltip.Trigger>
            <Button
              isIconOnly
              aria-label="刷新"
              size="sm"
              variant="ghost"
              isPending={isRefreshing}
              onPress={onRefresh}
            >
              <IconRefresh className="size-4" />
            </Button>
          </Tooltip.Trigger>
          <Tooltip.Content>刷新</Tooltip.Content>
        </Tooltip>
      </div>
    </header>
  );
}

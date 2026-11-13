import { useState } from "react";
import { Button, Chip, Tooltip } from "@heroui/react";

import { IconGear, IconLayers, IconRefresh } from "./icons";
import { FilterButton } from "./shared";
import { rememberTheme, storedChoice, type ThemeChoice } from "./theme";

export type MetricKey = "all" | "running" | "issues" | "stopped";

const LED_TONE: Record<MetricKey, string> = {
  all: "ph-led--idle",
  running: "ph-led--ok",
  issues: "ph-led--warn",
  stopped: "ph-led--idle",
};

const METRICS: Array<{ key: MetricKey; label: string }> = [
  { key: "all", label: "全部" },
  { key: "running", label: "运行中" },
  { key: "issues", label: "需处理" },
  { key: "stopped", label: "已停止" },
];

const THEME_LABEL: Record<ThemeChoice, string> = {
  dark: "深色",
  light: "浅色",
  system: "跟随",
};

interface TopBarProps {
  counts: Record<MetricKey, number>;
  metric: MetricKey;
  /** 引擎 core 版本（GET /api/status）；加载中为「…」 */
  coreVersion: string;
  onMetricChange: (key: MetricKey) => void;
  onRefresh: () => void;
  /** 右上角全局设置入口（模态，不含证书） */
  onOpenSettings: () => void;
  isRefreshing: boolean;
  /** 事件流连接态；false 时 LED 转危险色 */
  streamConnected: boolean;
}

export function TopBar({
  counts,
  metric,
  coreVersion,
  onMetricChange,
  onRefresh,
  onOpenSettings,
  isRefreshing,
  streamConnected,
}: TopBarProps) {
  const [choice, setChoice] = useState<ThemeChoice>(() => storedChoice());
  const cycleTheme = () => {
    const next: ThemeChoice = choice === "dark" ? "light" : choice === "light" ? "system" : "dark";
    rememberTheme(next);
    setChoice(next);
  };

  return (
    <header className="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-1 border-b border-border bg-surface px-6 py-2">
      {/* 品牌：磷光方砖 + appliance 代号 */}
      <div className="flex flex-none items-center gap-3">
        <span
          aria-hidden="true"
          className="grid size-8 place-items-center rounded bg-accent text-accent-foreground"
        >
          <IconLayers className="size-4" />
        </span>
        <span className="flex items-baseline gap-2 leading-tight">
          <span className="text-lg font-bold tracking-wide text-foreground">envboard</span>
          <span className="font-mono text-xs text-muted">[APPLIANCE]</span>
        </span>
      </div>

      {/* 顶栏健康总览：点击即筛选列表（单一过滤入口） */}
      <div
        aria-label="全局概览（点击筛选列表）"
        className="order-3 flex w-full flex-none flex-wrap items-center gap-1 sm:order-none sm:w-auto"
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
              <span aria-hidden="true" className={`ph-led ${LED_TONE[m.key]}`} />
            )}
            {m.label}
          </FilterButton>
        ))}
      </div>

      <div className="ml-auto flex min-w-0 items-center gap-2">
        <Chip className="hidden sm:inline-flex" color="success" size="sm" variant="soft">
          <span aria-hidden="true" className="ph-led ph-led--ok" />
          <Chip.Label>core {coreVersion}</Chip.Label>
        </Chip>
        <Chip color={streamConnected ? "success" : "danger"} size="sm" variant="soft">
          <span
            aria-hidden="true"
            className={`ph-led ${streamConnected ? "ph-led--ok ph-led--live" : "ph-led--err"}`}
          />
          <Chip.Label>{streamConnected ? "已连接" : "已断开"}</Chip.Label>
        </Chip>

        <Button aria-label="切换主题" size="sm" variant="ghost" onPress={cycleTheme}>
          <span className="font-mono text-xs">{THEME_LABEL[choice]}</span>
        </Button>

        <Tooltip delay={200}>
          <Tooltip.Trigger>
            <Button isIconOnly aria-label="打开设置" size="sm" variant="ghost" onPress={onOpenSettings}>
              <IconGear className="size-4" />
            </Button>
          </Tooltip.Trigger>
          <Tooltip.Content>设置</Tooltip.Content>
        </Tooltip>

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

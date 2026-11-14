import { useState } from "react";
import { Button, Chip, Tooltip } from "@heroui/react";

import { IconGear, IconLayers, IconRefresh } from "./icons";
import { SideBar } from "./SideBar";
import type { ViewKey } from "./data";
import { rememberTheme, storedChoice, type ThemeChoice } from "./theme";

export type MetricKey = "all" | "running" | "issues" | "stopped";

const THEME_LABEL: Record<ThemeChoice, string> = {
  dark: "深色",
  light: "浅色",
  system: "跟随",
};

interface TopBarProps {
  counts: Record<MetricKey, number>;
  coreVersion: string;
  onRefresh: () => void;
  onOpenSettings: () => void;
  isRefreshing: boolean;
  streamConnected: boolean;
  view: ViewKey;
  onViewChange: (view: ViewKey) => void;
  navCounts: Partial<Record<ViewKey, number>>;
}

/**
 * 方向A 单一顶栏：品牌 + 内联主导航（环境/证书）+ 不可点状态摘要 + 全局动作。
 * 筛选只保留在环境页工具栏一处（去重复控件）；顶栏摘要仅展示、不可点。
 */
export function TopBar({
  coreVersion,
  onRefresh,
  onOpenSettings,
  isRefreshing,
  streamConnected,
  view,
  onViewChange,
  navCounts,
}: TopBarProps) {
  const [choice, setChoice] = useState<ThemeChoice>(() => storedChoice());
  const cycleTheme = () => {
    const next: ThemeChoice = choice === "dark" ? "light" : choice === "light" ? "system" : "dark";
    rememberTheme(next);
    setChoice(next);
  };

  return (
    <header className="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-1 border-b border-border bg-surface px-4 py-2">
      <div className="flex flex-none items-center gap-2">
        {/* 品牌签名：单色 steel-blue mark（克制，不发光） */}
        <span
          aria-hidden="true"
          className="grid size-7 place-items-center rounded bg-accent text-accent-foreground"
        >
          <IconLayers className="size-4" />
        </span>
        <span className="text-base font-bold tracking-wide text-foreground">envboard</span>
      </div>

      <SideBar navCounts={navCounts} streamConnected={streamConnected} view={view} onViewChange={onViewChange} />

      <div className="ml-auto flex min-w-0 items-center gap-2">
        <Chip className="hidden sm:inline-flex" size="sm" variant="soft">
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
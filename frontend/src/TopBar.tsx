import { Button, Chip, Tooltip } from "@heroui/react";

import { IconGear, IconLayers } from "./icons";
import { SideBar } from "./SideBar";
import type { ViewKey } from "./data";
import type { ThemeChoice } from "./theme";

export type MetricKey = "all" | "running" | "issues" | "stopped";

const THEME_LABEL: Record<ThemeChoice, string> = {
  dark: "深色",
  light: "浅色",
  system: "跟随",
};

interface TopBarProps {
  coreVersion: string;
  onOpenSettings: () => void;
  streamConnected: boolean;
  view: ViewKey;
  onViewChange: (view: ViewKey) => void;
  navCounts: Partial<Record<ViewKey, number>>;
  /** 主题单一来源由 Workbench 持有（V-1）。 */
  theme: ThemeChoice;
  onThemeChange: (choice: ThemeChoice) => void;
}

/**
 * 方向A 单一顶栏：品牌 + 内联主导航（环境/证书）+ 不可点状态摘要 + 全局动作。
 * 筛选只保留在环境页工具栏一处（去重复控件）；顶栏摘要仅展示、不可点。
 */
export function TopBar({
  coreVersion,
  onOpenSettings,
  streamConnected,
  view,
  onViewChange,
  navCounts,
  theme: choice,
  onThemeChange,
}: TopBarProps) {
  const cycleTheme = () => {
    const next: ThemeChoice = choice === "dark" ? "light" : choice === "light" ? "system" : "dark";
    onThemeChange(next);
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

      <SideBar navCounts={navCounts} view={view} onViewChange={onViewChange} />

      <div className="ph-top-actions ml-auto flex min-w-0 items-center gap-2">
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

        <Tooltip delay={200}>
          <Tooltip.Trigger>
            <Button aria-label="切换主题" size="sm" variant="ghost" onPress={cycleTheme}>
              {/* 与 chip 标签同字体（sans）/字号/行盒：mono 的 ascent 不同会让基线错开 */}
              <span className="text-xs leading-none">{THEME_LABEL[choice]}</span>
            </Button>
          </Tooltip.Trigger>
          <Tooltip.Content>切换主题（当前：{THEME_LABEL[choice]}）</Tooltip.Content>
        </Tooltip>

        <Tooltip delay={200}>
          <Tooltip.Trigger>
            <Button isIconOnly aria-label="打开设置" size="sm" variant="ghost" onPress={onOpenSettings}>
              <IconGear className="size-4" />
            </Button>
          </Tooltip.Trigger>
          <Tooltip.Content>设置</Tooltip.Content>
        </Tooltip>

      </div>
    </header>
  );
}
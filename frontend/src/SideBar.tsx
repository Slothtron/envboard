import type { ComponentType, SVGProps } from "react";
import { Button } from "@heroui/react";

import type { ViewKey } from "./data";
import {
  IconBolt,
  IconCompare,
  IconGear,
  IconInfo,
  IconLayers,
  IconRules,
  IconServer,
} from "./icons";

type IconComponent = ComponentType<SVGProps<SVGSVGElement>>;

interface NavItem {
  key: ViewKey;
  label: string;
  Icon: IconComponent;
}

const NAV: NavItem[] = [
  { key: "environments", label: "环境", Icon: IconLayers },
  { key: "rules", label: "规则库", Icon: IconRules },
  { key: "proxies", label: "上游代理", Icon: IconServer },
  { key: "debug", label: "调试", Icon: IconBolt },
  { key: "compare", label: "跨环境对比", Icon: IconCompare },
  { key: "activity", label: "活动", Icon: IconInfo },
  { key: "settings", label: "设置", Icon: IconGear },
];

const GROUPS: Array<{ title: string; keys: ViewKey[] }> = [
  { title: "资源", keys: ["environments", "rules", "proxies"] },
  { title: "观测", keys: ["debug", "compare", "activity"] },
  { title: "系统", keys: ["settings"] },
];

interface SideBarProps {
  view: ViewKey;
  onViewChange: (view: ViewKey) => void;
  navCounts: Partial<Record<ViewKey, number>>;
  streamConnected: boolean;
}

/**
 * 重构点 P6：侧栏回归纯导航（带计数），不再兼任环境列表 / 搜索 / 状态条。
 * 暗色由 html.dark 统一切换，此处不写深色覆盖。
 */
export function SideBar({ view, onViewChange, navCounts, streamConnected }: SideBarProps) {
  return (
    <aside
      aria-label="工作台导航"
      className="flex min-h-0 flex-col border-r border-border bg-surface-secondary"
    >
      <nav aria-label="视图" className="flex flex-col gap-2 border-b border-border p-3">
        {GROUPS.map((group) => (
          <div key={group.title} className="flex flex-col gap-1">
            <span className="px-3 pb-1 pt-2 text-sm font-semibold uppercase tracking-wide text-muted">
              {group.title}
            </span>
            {group.keys.map((key) => {
              const item = NAV.find((n) => n.key === key)!;
              const { Icon } = item;
              const isActive = view === key;
              const count = navCounts[key];

              return (
                <Button
                  key={key}
                  aria-current={isActive ? "true" : undefined}
                  className="w-full justify-start"
                  size="sm"
                  variant={isActive ? "secondary" : "ghost"}
                  onPress={() => onViewChange(key)}
                >
                  <Icon className={`size-4 ${isActive ? "text-accent" : "text-muted"}`} />
                  <span className={isActive ? "font-semibold text-accent" : "font-medium"}>
                    {item.label}
                  </span>
                  {typeof count === "number" ? (
                    <span
                      className={`ml-auto font-mono text-sm ${isActive ? "text-accent" : "text-muted"}`}
                    >
                      {count}
                    </span>
                  ) : null}
                </Button>
              );
            })}
          </div>
        ))}
      </nav>

      {/* 连接态只在侧栏底部出现一次（重构点 P13：同一状态不重复堆叠） */}
      <div className="mt-auto flex items-center gap-2 border-t border-border p-3 text-sm text-muted">
        <span
          aria-hidden="true"
          className={`size-2 rounded-full ${streamConnected ? "bg-success" : "bg-danger"}`}
        />
        <span className="font-mono">SSE · 0 丢弃</span>
      </div>
    </aside>
  );
}

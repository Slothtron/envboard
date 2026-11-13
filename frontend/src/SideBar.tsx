import type { ComponentType, SVGProps } from "react";
import { Button } from "@heroui/react";

import type { ViewKey } from "./data";
import { IconLayers, IconShield } from "./icons";

type IconComponent = ComponentType<SVGProps<SVGSVGElement>>;

interface NavItem {
  key: ViewKey;
  label: string;
  code: string;
  Icon: IconComponent;
}

/**
 * 两视图导航（方向C 顶部横向 tab）：规则/代理并入环境页内页签；
 * 抓包日志是环境子页，证书独立成页（ADR-011）。
 */
const NAV: NavItem[] = [
  { key: "environments", label: "环境", code: "ENV", Icon: IconLayers },
  { key: "cert", label: "证书", code: "CA", Icon: IconShield },
];

interface SideBarProps {
  view: ViewKey;
  onViewChange: (view: ViewKey) => void;
  navCounts: Partial<Record<ViewKey, number>>;
  streamConnected: boolean;
}

/**
 * 方向C 外壳：顶部横向 tab 导航（保留 aria-label 与 NAV 键集合，
 * artifact 测试的「导航 ↔ 视图」接线不变）。暗色由 html.dark 统一切换。
 */
export function SideBar({ view, onViewChange, navCounts, streamConnected }: SideBarProps) {
  return (
    <nav aria-label="工作台导航" className="ph-nav" data-full-bleed>
      {NAV.map((item) => {
        const { Icon } = item;
        const isActive = view === item.key;
        const count = navCounts[item.key];
        return (
          <Button
            key={item.key}
            aria-current={isActive ? "true" : undefined}
            className={`ph-navitem ${isActive ? "ph-navitem--active text-accent" : "text-foreground"}`}
            size="sm"
            variant="ghost"
            onPress={() => onViewChange(item.key)}
          >
            <Icon className="size-4" />
            <span className={isActive ? "font-semibold" : "font-medium"}>{item.label}</span>
            <span className="ph-navcode">[{item.code}]</span>
            {typeof count === "number" ? <span className="ph-navbadge">{count}</span> : null}
          </Button>
        );
      })}
      <span className="ml-auto flex flex-none items-center gap-2 pl-3">
        <span
          aria-hidden="true"
          className={`ph-led ${streamConnected ? "ph-led--ok ph-led--live" : "ph-led--err"}`}
        />
        <span className="ph-navcode">IPC</span>
      </span>
    </nav>
  );
}

import type { ComponentType, SVGProps } from "react";
import { Button } from "@heroui/react";

import type { ViewKey } from "./data";
import { IconLayers, IconShield } from "./icons";

type IconComponent = ComponentType<SVGProps<SVGSVGElement>>;

interface NavItem {
  key: ViewKey;
  label: string;
  Icon: IconComponent;
}

/**
 * 主导航（方向A：并入顶栏的内联主 tab，仅 环境/证书 两视图）。
 * 规则/代理并入环境页内分段控件；抓包是环境子页；证书独立成页（ADR-011）。
 * 保留 NAV 键集合与 aria-label（artifact「导航 ↔ 视图」接线不变）。
 */
const NAV: NavItem[] = [
  { key: "environments", label: "环境", Icon: IconLayers },
  { key: "cert", label: "证书", Icon: IconShield },
];

interface SideBarProps {
  view: ViewKey;
  onViewChange: (view: ViewKey) => void;
  navCounts: Partial<Record<ViewKey, number>>;
}

export function SideBar({ view, onViewChange, navCounts }: SideBarProps) {
  return (
    <nav aria-label="工作台导航" className="ph-nav">
      {NAV.map((item) => {
        const { Icon } = item;
        const isActive = view === item.key;
        const count = navCounts[item.key];
        return (
          <Button
            key={item.key}
            aria-current={isActive ? "true" : undefined}
            className={`ph-navitem ${isActive ? "ph-navitem--active" : "text-muted"}`}
            size="sm"
            variant="ghost"
            onPress={() => onViewChange(item.key)}
          >
            <Icon className="size-4" />
            <span>{item.label}</span>
            {typeof count === "number" ? <span className="ph-navbadge">{count}</span> : null}
          </Button>
        );
      })}
    </nav>
  );
}
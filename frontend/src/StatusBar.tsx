import { useEffect, useState } from "react";

import type { EnvView, StatusInfo } from "./api/types";
import { formatPortRange } from "./data";
import { IconAlert } from "./icons";

interface StatusBarProps {
  statusInfo: StatusInfo | null;
  environments: EnvView[];
  streamConnected: boolean;
}

/** 装配警告：由环境健康推导（与门面语义一致），状态栏集中呈现。 */
function assemblyWarnings(envs: EnvView[]): string[] {
  const out: string[] = [];
  for (const e of envs) {
    if (e.rules_missing) out.push(`${e.name} 规则集缺失`);
    if (e.health === "port_conflict") out.push(`${e.name} 端口冲突 ${e.listen.port}`);
    if (e.health === "unhealthy") out.push(`${e.name} 不健康·旧快照在服务`);
    if (e.health === "failed") out.push(`${e.name} 启动失败`);
  }
  return out;
}

/** 方向C 外壳：底部 appliance 状态栏（state dir / 装配警告 / 端口区间 / 丢弃计数 / UTC）。 */
export function StatusBar({ statusInfo, environments, streamConnected }: StatusBarProps) {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const t = window.setInterval(() => setNow(new Date()), 1000);
    return () => window.clearInterval(t);
  }, []);

  const warnings = assemblyWarnings(environments);
  const utc = now.toISOString().slice(11, 19);

  return (
    <footer className="ph-statusbar" aria-label="工作台状态栏">
      <span className="flex flex-none items-center gap-2">
        <span aria-hidden="true" className={`ph-led ${streamConnected ? "ph-led--ok" : "ph-led--err"}`} />
        <b>state</b>
        <span className="truncate">{statusInfo?.config.state_dir ?? "…"}</span>
      </span>
      {warnings.map((w) => (
        <span key={w} className="ph-warnchip">
          <IconAlert className="size-3" />
          {w}
        </span>
      ))}
      <span className="ml-auto flex flex-none items-center gap-3">
        <span>
          端口区间 <b>{formatPortRange(statusInfo?.config.port_range)}</b>
        </span>
        <span>
          丢弃 <b>{statusInfo?.events_dropped ?? 0}</b>
        </span>
        <span>{utc} UTC</span>
      </span>
    </footer>
  );
}

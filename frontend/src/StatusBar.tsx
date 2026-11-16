import type { EnvView, StatusInfo } from "./api/types";
import { IconAlert } from "./icons";

interface StatusBarProps {
  statusInfo: StatusInfo | null;
  environments: EnvView[];
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

/**
 * 方向A 外壳：底部状态栏只保留「需要人处理的信息」——
 * 装配告警（无告警时一句「装配正常」）与事件丢弃（>0 才出现）。
 * 端口区间是后端的默认行为（用户可自行改端口），UTC 时钟无操作价值，均不展示。
 */
export function StatusBar({ statusInfo, environments }: StatusBarProps) {
  const warnings = assemblyWarnings(environments);
  const dropped = statusInfo?.events_dropped ?? 0;

  return (
    <footer className="ph-statusbar font-sans" aria-label="工作台状态栏">
      {warnings.length === 0 ? (
        <span className="flex-none">装配正常</span>
      ) : (
        warnings.map((w) => (
          <span key={w} className="ph-warnchip">
            <IconAlert className="size-3" />
            {w}
          </span>
        ))
      )}
      {dropped > 0 ? (
        <span className="ml-auto flex flex-none items-center gap-1">
          事件丢弃 <b className="font-mono">{dropped}</b>
        </span>
      ) : null}
    </footer>
  );
}

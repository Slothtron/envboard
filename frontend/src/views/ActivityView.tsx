import { useCallback, useEffect, useMemo, useState } from "react";
import { Button, Chip, SearchField, Separator, Surface } from "@heroui/react";

import { api } from "../api/client";
import type { ControlEvent } from "../api/types";
import {
  activityDomain,
  activityLabel,
  activityTarget,
  activityTone,
  type ActivityDomain,
} from "../data";
import { IconRefresh } from "../icons";
import { EmptyState, ErrorState, FilterButton, Hint, SectionTitle, TableSkeleton } from "../shared";

/** 事件信封的 `time` 是 epoch 毫秒；展示统一 `HH:MM:SS`（zh locale 不产「时分秒」汉字）。 */
const fmt = new Intl.DateTimeFormat("en-GB", {
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
  hour12: false,
});
const formatClock = (time: number) => fmt.format(time);
/** 分组标题：今天/昨天/具体日期（跨日账本不歧义）。 */
function formatDay(time: number): string {
  const startOfDay = (t: number) => new Date(t).setHours(0, 0, 0, 0);
  const diffDays = Math.round((startOfDay(Date.now()) - startOfDay(time)) / 86_400_000);
  if (diffDays === 0) return "今天";
  if (diffDays === 1) return "昨天";
  return new Date(time).toLocaleDateString("zh-CN");
}

const DOMAIN_LABEL: Record<ActivityDomain, string> = {
  env: "环境",
  rules: "规则",
  engine: "引擎",
  instance: "实例",
  other: "其他",
};

const DOT_TONE: Record<"default" | "warning" | "danger", string> = {
  default: "bg-muted",
  warning: "bg-warning",
  danger: "bg-danger",
};

/**
 * 活动视图（GET /api/history 控制面审计事件，spec/events.md 信封）。
 * 只读拉取式：无创建类主操作，工具条不出现 primary（设计规范 D-4 配额约束）。
 * 单一筛选入口：域 chips + 搜索框（D-8）。
 */
export function ActivityView() {
  const [domain, setDomain] = useState<ActivityDomain | null>(null);
  const [search, setSearch] = useState("");
  const [events, setEvents] = useState<ControlEvent[]>([]);
  const [status, setStatus] = useState<"loading" | "error" | "ready">("loading");
  const [isRefreshing, setIsRefreshing] = useState(false);

  const load = useCallback(() => {
    setIsRefreshing(true);
    api
      .history({ limit: 200 })
      .then((r) => {
        setEvents(r.events);
        setStatus("ready");
      })
      .catch(() => setStatus("error"))
      .finally(() => setIsRefreshing(false));
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const counts = useMemo(() => {
    const acc: Record<ActivityDomain, number> = { env: 0, rules: 0, engine: 0, instance: 0, other: 0 };
    for (const ev of events) acc[activityDomain(ev.type)] += 1;
    return acc;
  }, [events]);

  const visible = useMemo(() => {
    const q = search.trim().toLowerCase();
    return events.filter((ev) => {
      if (domain && activityDomain(ev.type) !== domain) return false;
      if (!q) return true;
      const target = activityTarget(ev) ?? "";
      return (
        ev.type.toLowerCase().includes(q) ||
        target.toLowerCase().includes(q) ||
        activityLabel(ev).toLowerCase().includes(q)
      );
    });
  }, [events, domain, search]);

  const filtered = domain !== null || search.trim().length > 0;

  /** 按日分组（事件按 seq 倒序，同日归一组）：跨日账本读起来不歧义。 */
  const groups = useMemo(() => {
    const out: Array<{ day: string; events: ControlEvent[] }> = [];
    for (const ev of visible) {
      const day = formatDay(ev.time);
      const last = out[out.length - 1];
      if (last && last.day === day) last.events.push(ev);
      else out.push({ day, events: [ev] });
    }
    return out;
  }, [visible]);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex flex-wrap items-center gap-2 border-b border-border bg-surface px-5 py-3">
        <div aria-label="事件域筛选" className="flex flex-wrap items-center gap-1" role="group">
          <FilterButton isActive={domain === null} onPress={() => setDomain(null)}>
            全部
            <span className="font-mono">{events.length}</span>
          </FilterButton>
          {(Object.keys(DOMAIN_LABEL) as ActivityDomain[]).map((key) => (
            <FilterButton
              key={key}
              count={counts[key]}
              isActive={domain === key}
              onPress={() => setDomain(domain === key ? null : key)}
            >
              {DOMAIN_LABEL[key]}
            </FilterButton>
          ))}
        </div>

        <SearchField aria-label="搜索事件" className="ml-auto flex-none" value={search} onChange={setSearch}>
          <SearchField.Group>
            <SearchField.SearchIcon />
            <SearchField.Input className="w-40 xl:w-56" placeholder="搜索类型 / 对象" />
            <SearchField.ClearButton />
          </SearchField.Group>
        </SearchField>

        <Button
          isPending={isRefreshing && status === "ready"}
          size="sm"
          variant="secondary"
          onPress={load}
        >
          {!isRefreshing ? <IconRefresh className="size-4" /> : null}
          刷新
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto p-5">
        {status === "loading" ? (
          <TableSkeleton rows={6} />
        ) : status === "error" ? (
          <ErrorState
            title="活动记录加载失败"
            description="无法读取审计事件账本。确认状态目录下的 events.jsonl 可读后重试。"
            isRetrying={isRefreshing}
            onRetry={load}
          />
        ) : visible.length === 0 ? (
          <EmptyState
            hint={
              filtered
                ? "当前筛选条件没有命中任何事件，清除筛选试试。"
                : "还没有任何控制面操作。创建环境或导入规则后会出现在这里。"
            }
            title={filtered ? "没有匹配的事件" : "暂无活动"}
            action={
              filtered ? (
                <Button
                  size="sm"
                  variant="secondary"
                  onPress={() => {
                    setDomain(null);
                    setSearch("");
                  }}
                >
                  清除筛选
                </Button>
              ) : undefined
            }
          />
        ) : (
          <div className="flex flex-col gap-4">
            <Hint>权威状态以环境列表为准；这里是「为什么变成这样」的证据面，只读。</Hint>
            {groups.map((group) => (
              <div key={group.day} className="flex flex-col gap-2">
                <SectionTitle>{group.day}</SectionTitle>
                <Surface className="flex flex-col p-0">
                  {group.events.map((ev, index) => (
                    <div key={ev.seq}>
                      {index > 0 ? <Separator /> : null}
                      <EventRow ev={ev} />
                    </div>
                  ))}
                </Surface>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

/** 一行事件：时间（mono）+ 序号（mono muted）+ 文案 + 对象 chip + 域标签。 */
function EventRow({ ev }: { ev: ControlEvent }) {
  const tone = activityTone(ev);
  const target = activityTarget(ev);
  return (
    <div className="flex flex-wrap items-center gap-3 px-4 py-3" data-activity-seq={ev.seq} data-activity-type={ev.type}>
      <span aria-hidden="true" className={`size-2 flex-none rounded-full ${DOT_TONE[tone]}`} />
      <span className="w-24 flex-none font-mono text-sm text-muted">{formatClock(ev.time)}</span>
      <span className="w-14 flex-none font-mono text-sm text-muted">#{ev.seq}</span>
      <span className="min-w-0 flex-1 truncate text-base text-foreground">{activityLabel(ev)}</span>
      {target ? (
        <Chip color="default" size="sm" variant="soft">
          <Chip.Label>{target}</Chip.Label>
        </Chip>
      ) : null}
      <span className="flex-none text-sm text-muted">{DOMAIN_LABEL[activityDomain(ev.type)]}</span>
      <span className="flex-none font-mono text-sm text-muted">{ev.type}</span>
    </div>
  );
}

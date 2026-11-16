import { useEffect, useMemo, useRef, useState } from "react";
import {
  Button,
  Chip,
  Dropdown,
  ListBox,
  SearchField,
  Select,
  Separator,
  Table,
  Tabs,
  Tooltip,
  type SortDescriptor,
} from "@heroui/react";

import type { EnvPatchReq, EnvView } from "../api/types";
import { envNeedsAttention, healthTone, listenAddress, type EnvSection } from "../data";
import { ProxiesView, RulesView } from "./CatalogViews";
import { IconPlus, IconStop, IconPlay } from "../icons";
import { EmptyState, ErrorState, FilterButton, SEG_TAB, StatusBadge, TableSkeleton } from "../shared";
import { CaptureView } from "./CaptureView";
import { EnvironmentDetail } from "./EnvironmentDetail";

export type LoadStatus = "loading" | "error" | "ready";
type MetricFilter = "all" | "running" | "issues" | "stopped";

const FILTER_LABEL: Record<MetricFilter, string> = {
  all: "全部",
  running: "运行中",
  stopped: "已停止",
  issues: "需处理",
};

export interface EnvironmentsViewProps {
  environments: EnvView[];
  status: LoadStatus;
  isRefreshing: boolean;
  onRetry: () => void;
  metricFilter: MetricFilter;
  onMetricFilterChange: (key: MetricFilter) => void;
  selected: string | null;
  onSelect: (name: string) => void;
  onToggle: (env: EnvView) => void;
  onRestart: (env: EnvView) => void;
  onDelete: (env: EnvView) => void;
  onNew: () => void;
  onCopied: () => void;
  onSaveConfig: (name: string, patch: EnvPatchReq) => Promise<void>;
  onFeedback: (status: "success" | "warning" | "danger", title: string, detail?: string) => void;
}

export function EnvironmentsView({
  environments,
  status,
  isRefreshing,
  onRetry,
  metricFilter,
  onMetricFilterChange,
  selected,
  onSelect,
  onToggle,
  onRestart,
  onDelete,
  onNew,
  onCopied,
  onSaveConfig,
  onFeedback,
}: EnvironmentsViewProps) {
  const [search, setSearch] = useState("");
  const [sort, setSort] = useState<SortDescriptor>({ column: "name", direction: "ascending" });
  const [detailOpen, setDetailOpen] = useState(false);
  const [section, setSection] = useState<EnvSection>("instances");
  const [captureEnv, setCaptureEnv] = useState<string | null>(null);
  const panelRef = useRef<HTMLElement>(null);
  const returnFocusRef = useRef<string | null>(null);
  // 抽屉打开时按列表实际可用宽度切窄列集（r3 §9.3）：ResizeObserver 直接量
  // 收敛后的容器宽（不依赖 window resize 事件），窄于 700 只留 环境/状态/操作。
  const listRef = useRef<HTMLDivElement>(null);
  const [listW, setListW] = useState(0);
  useEffect(() => {
    const el = listRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      for (const e of entries) setListW(e.contentRect.width);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [section]);
  const showRules = !detailOpen;
  const showListen = !detailOpen || listW >= 700;

  const counts = useMemo(() => {
    const running = environments.filter((e) => e.health === "running").length;
    const stopped = environments.filter((e) => e.health === "stopped").length;
    const issues = environments.filter(envNeedsAttention).length;
    return { all: environments.length, running, stopped, issues };
  }, [environments]);

  const visible = useMemo(() => {
    const q = search.trim().toLowerCase();
    let list = environments.filter((e) => {
      const matchMetric =
        metricFilter === "all" ||
        (metricFilter === "running" && e.health === "running") ||
        (metricFilter === "stopped" && e.health === "stopped") ||
        (metricFilter === "issues" && envNeedsAttention(e));
      if (!matchMetric) return false;
      if (!q) return true;
      return (
        e.name.toLowerCase().includes(q) ||
        String(e.listen.port).includes(q) ||
        (e.rules ?? "").toLowerCase().includes(q)
      );
    });
    const dir = sort.direction === "descending" ? -1 : 1;
    /* 状态排序与 data.ts healthTone 同源：危险 0 · 过渡 1 · 健康 2 · 停止 3（V-16） */
    const severityRank = (health: string): number => {
      const tone = healthTone(health);
      if (tone === "danger") return 0;
      if (tone === "warning") return 1;
      if (tone === "success") return 2;
      return 3;
    };
    list = [...list].sort((a, b) => {
      let cmp: number;
      if (sort.column === "port") cmp = a.listen.port - b.listen.port;
      else if (sort.column === "status") cmp = severityRank(a.health) - severityRank(b.health);
      else cmp = a.name.localeCompare(b.name);
      return cmp * dir || a.name.localeCompare(b.name);
    });
    return list;
  }, [environments, metricFilter, search, sort]);

  const active = environments.find((e) => e.name === selected) ?? null;
  const activeCapture = useMemo(
    () => (captureEnv ? (environments.find((e) => e.name === captureEnv) ?? null) : null),
    [environments, captureEnv],
  );

  useEffect(() => {
    if (!active) setDetailOpen(false);
  }, [active]);

  useEffect(() => {
    if (captureEnv && !activeCapture) setCaptureEnv(null);
  }, [captureEnv, activeCapture]);

  useEffect(() => {
    if (!detailOpen) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (document.querySelector('[role="dialog"], [role="alertdialog"]')) return;
      setDetailOpen(false);
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [detailOpen]);

  useEffect(() => {
    if (detailOpen) {
      panelRef.current?.focus();
    } else if (returnFocusRef.current) {
      const row = document.querySelector(
        `tr[data-env-row="${CSS.escape(returnFocusRef.current)}"]`,
      );
      (row as HTMLElement | null)?.focus();
      returnFocusRef.current = null;
    }
  }, [detailOpen]);

  function openDetail(env: EnvView) {
    returnFocusRef.current = env.name;
    onSelect(env.name);
    setDetailOpen(true);
  }

  const onListKeyDownCapture = (e: React.KeyboardEvent) => {
    if (e.key !== "Enter" && e.key !== " ") return;
    const target = e.target as HTMLElement;
    const row = target.closest?.("tr[data-env-row]") as HTMLElement | null;
    if (!row || target !== row) return;
    const env = environments.find((x) => x.name === row.dataset.envRow);
    if (!env) return;
    e.preventDefault();
    e.stopPropagation();
    openDetail(env);
  };

  const emptyState = (
    <EmptyState
      title="没有匹配的环境"
      hint={
        search || metricFilter !== "all"
          ? "当前搜索词或状态筛选排除了全部环境。清除筛选即可看到完整列表。"
          : "还没有任何环境。新建一个环境后，它会自动分配一个端口，之后可在配置里修改。"
      }
      action={
        search || metricFilter !== "all" ? (
          <Button
            size="sm"
            variant="secondary"
            onPress={() => {
              setSearch("");
              onMetricFilterChange("all");
            }}
          >
            清除筛选
          </Button>
        ) : (
          <Button size="sm" variant="secondary" onPress={onNew}>
            <IconPlus className="size-4" />
            新建环境
          </Button>
        )
      }
    />
  );

  if (activeCapture) {
    return (
      <CaptureView
        env={activeCapture}
        onBack={() => setCaptureEnv(null)}
        onFeedback={onFeedback}
      />
    );
  }

  return (
    <Tabs
      align="start"
      className="wb-seg flex min-h-0 flex-1 flex-col"
      selectedKey={section}
      onSelectionChange={(key) => {
        const next = key === "rules" || key === "proxies" ? key : "instances";
        setSection(next);
        if (next !== "instances") setDetailOpen(false);
      }}
    >
      <div className="bg-surface px-4 py-2">
        <div className="w-fit rounded border border-border bg-surface-secondary p-0.5">
          <Tabs.List aria-label="环境工作台">
            <Tabs.Tab className={SEG_TAB} id="instances">
              实例
              <Chip className="ml-2" size="sm" variant="soft">
                <Chip.Label>{environments.length}</Chip.Label>
              </Chip>
            </Tabs.Tab>
            <Tabs.Tab className={SEG_TAB} id="rules">规则集</Tabs.Tab>
            <Tabs.Tab className={SEG_TAB} id="proxies">上游代理</Tabs.Tab>
          </Tabs.List>
        </div>
      </div>

      <Tabs.Panel className="flex min-h-0 flex-1 flex-col" id="instances">
      <div className="flex min-h-0 flex-1 flex-col">
        <div className="flex flex-wrap items-center gap-2 border-b border-border bg-surface px-4 py-1.5">
          <SearchField
            aria-label="搜索环境、端口或规则"
            className="min-w-40 flex-none"
            value={search}
            onChange={setSearch}
          >
            <SearchField.Group>
              <SearchField.SearchIcon />
              <SearchField.Input className="w-40 xl:w-56" placeholder="搜索环境 / 端口 / 规则" />
              <SearchField.ClearButton aria-label="清除搜索" />
            </SearchField.Group>
          </SearchField>

          {/* ≥1024：常驻筛选 chips；<1024：收进下拉（断点策略） */}
          <div aria-label="按状态筛选" className="hidden flex-wrap items-center gap-2 min-[1024px]:flex" role="group">
            {(Object.keys(FILTER_LABEL) as MetricFilter[]).map((k) => (
              <FilterButton
                key={k}
                count={counts[k]}
                isActive={metricFilter === k}
                onPress={() => onMetricFilterChange(k)}
              >
                {FILTER_LABEL[k]}
              </FilterButton>
            ))}
          </div>
          <div className="min-[1024px]:hidden">
          <Select
            aria-label="按状态筛选"
            className="w-32"
            selectedKey={metricFilter}
            onSelectionChange={(k) => onMetricFilterChange(k as MetricFilter)}
          >
            <Select.Trigger>
              <Select.Value />
              <Select.Indicator />
            </Select.Trigger>
            <Select.Popover>
              <ListBox>
                {(Object.keys(FILTER_LABEL) as MetricFilter[]).map((k) => (
                  <ListBox.Item key={k} id={k} textValue={FILTER_LABEL[k]}>
                    {FILTER_LABEL[k]}（{counts[k]}）
                    <ListBox.ItemIndicator />
                  </ListBox.Item>
                ))}
              </ListBox>
            </Select.Popover>
          </Select>
          </div>

          <div className="ml-auto flex items-center gap-2">
            <Button size="sm" variant="primary" onPress={onNew}>
              <IconPlus className="size-4" />
              新建环境
            </Button>
          </div>
        </div>

        <div className="wb-content relative flex min-h-0 flex-1 overflow-hidden">
          <div
            className={`${detailOpen ? "wb-list-inset " : ""}flex min-h-0 min-w-0 flex-1 flex-col`}
            onKeyDownCapture={onListKeyDownCapture}
            ref={listRef}
          >
            {/* 面板铺满（r2 P0①）：底色/边界放在滚动容器上，空白落进面板内 */}
            <div className="min-h-0 flex-1 overflow-hidden bg-surface">
              {status === "loading" ? <TableSkeleton rows={4} /> : null}
              {status === "error" ? (
                <ErrorState
                  title="环境列表加载失败"
                  description="无法连接到 proxy core。确认服务已启动后重试。"
                  isRetrying={isRefreshing}
                  onRetry={onRetry}
                />
              ) : null}
              {status === "ready" ? (
                visible.length === 0 ? (
                  /* 空态垂直居中于铺满面板（r3 §9.4） */
                  <div className="grid h-full place-items-center">{emptyState}</div>
                ) : (
                <Table className="h-full min-h-0" variant="secondary">
                  <Table.ScrollContainer className="h-full min-h-0 overflow-auto" data-full-bleed>
                    <Table.Content
                      aria-label="环境列表"
                      className={`table-fixed ${detailOpen ? "min-w-0" : "min-w-160"}`}
                      selectedKeys={selected ? [selected] : []}
                      selectionMode="single"
                      sortDescriptor={sort}
                      onSortChange={setSort}
                      onSelectionChange={(keys) => {
                        const next = Array.from(keys as Set<string | number>)[0];
                        if (next != null) onSelect(String(next));
                      }}
                    >
                      <Table.Header className="sticky top-0 z-10 bg-surface-secondary">
                        {/* 语义列宽（r3 §9.3）：标识符/规则弹性，状态/监听/操作固定，
                            抽屉收敛后仍能在可用宽度内完整分配 */}
                        <Table.Column allowsSorting id="name" isRowHeader>
                          {({ sortDirection }) => (
                            <Table.SortableColumnHeader sortDirection={sortDirection}>
                              环境
                            </Table.SortableColumnHeader>
                          )}
                        </Table.Column>
                        <Table.Column allowsSorting className="w-[150px]" id="status">
                          {({ sortDirection }) => (
                            <Table.SortableColumnHeader sortDirection={sortDirection}>
                              状态
                            </Table.SortableColumnHeader>
                          )}
                        </Table.Column>
                        {showListen ? (
                          <Table.Column allowsSorting className="w-[190px]" id="port">
                            {({ sortDirection }) => (
                              <Table.SortableColumnHeader sortDirection={sortDirection}>
                                监听
                              </Table.SortableColumnHeader>
                            )}
                          </Table.Column>
                        ) : null}
                        {showRules ? (
                          <Table.Column className="w-[120px]" id="rules">
                            规则
                          </Table.Column>
                        ) : null}
                        <Table.Column className="w-[124px] text-end" id="actions">
                          操作
                        </Table.Column>
                      </Table.Header>
                      <Table.Body
                        items={visible.map((env) => ({ ...env, id: env.name }))}
                        renderEmptyState={() => emptyState}
                      >
                        {(env) => (
                          <EnvRow
                            key={env.name}
                            env={env}
                            showListen={showListen}
                            showRules={showRules}
                            onDelete={onDelete}
                            onOpenCapture={(e) => {
                              setDetailOpen(false);
                              setCaptureEnv(e.name);
                            }}
                            onRestart={onRestart}
                            onToggle={onToggle}
                            onViewDetail={openDetail}
                          />
                        )}
                      </Table.Body>
                    </Table.Content>
                  </Table.ScrollContainer>
                </Table>
                )
              ) : null}
            </div>
          </div>

          <aside
            aria-label="环境详情"
            aria-hidden={!detailOpen}
            className={`wb-detail-panel wb-detail-panel--env ${detailOpen ? "wb-detail-panel--open" : ""}`}
            data-env={active?.name}
            ref={panelRef}
            tabIndex={-1}
          >
            {detailOpen && active ? (
              <EnvironmentDetail
                env={active}
                onCopied={onCopied}
                onDelete={onDelete}
                onRestart={onRestart}
                onSave={onSaveConfig}
                onToggle={onToggle}
                onClose={() => setDetailOpen(false)}
              />
            ) : null}
          </aside>
        </div>
      </div>
      </Tabs.Panel>

      <Tabs.Panel className="flex min-h-0 flex-1 flex-col" id="rules">
        {section === "rules" ? (
          <RulesView environments={environments} onFeedback={onFeedback} />
        ) : null}
      </Tabs.Panel>

      <Tabs.Panel className="flex min-h-0 flex-1 flex-col" id="proxies">
        {section === "proxies" ? <ProxiesView onFeedback={onFeedback} /> : null}
      </Tabs.Panel>
    </Tabs>
  );
}

/* ------------------------------------------------------------------ */

function EnvRow({
  env,
  showListen,
  showRules,
  onToggle,
  onRestart,
  onDelete,
  onOpenCapture,
  onViewDetail,
}: {
  env: EnvView;
  showListen: boolean;
  showRules: boolean;
  onToggle: (env: EnvView) => void;
  onRestart: (env: EnvView) => void;
  onDelete: (env: EnvView) => void;
  onOpenCapture: (env: EnvView) => void;
  onViewDetail: (env: EnvView) => void;
}) {
  const isRunning = env.health === "running";

  return (
    <Table.Row
      className="cursor-pointer"
      data-env-health={env.health}
      data-env-row={env.name}
      id={env.name}
      onDoubleClick={() => onViewDetail(env)}
    >
      <Table.Cell>
        {/* 单行标识符；描述移入 title 与详情（行高密度） */}
        <span
          className="block truncate font-mono text-sm font-semibold text-foreground"
          title={env.name}
        >
          {env.name}
        </span>
      </Table.Cell>

      <Table.Cell>
        <StatusBadge health={env.health} />
        {envNeedsAttention(env) ? (
          <Chip className="ml-2" color="warning" size="sm" variant="soft">
            <Chip.Label>待处理</Chip.Label>
          </Chip>
        ) : null}
      </Table.Cell>

      {showListen ? (
        <Table.Cell>
          <span className="block truncate font-mono text-sm text-muted" title={listenAddress(env)}>
            {listenAddress(env)}
          </span>
        </Table.Cell>
      ) : null}

      {showRules ? (
        <Table.Cell>
          {env.rules === null ? (
            <span className="block truncate font-mono text-sm text-muted">（不覆盖）</span>
          ) : env.rules_missing ? (
            <span className="block truncate font-mono text-sm text-warning" title={`${env.rules}（缺失）`}>
              {env.rules}（缺失）
            </span>
          ) : (
            <span
              className="block truncate font-mono text-sm text-foreground"
              title={`${env.rules}（${env.rules_count}）`}
            >
              {env.rules}
              <span className="text-muted">（{env.rules_count}）</span>
            </span>
          )}
        </Table.Cell>
      ) : null}

      {/* 操作分级：常驻 1 主操作（启停）+ 1「更多」菜单；删除入菜单并确认 */}
      <Table.Cell>
        <div className="flex items-center justify-end gap-1">
          <Tooltip delay={200}>
            <Tooltip.Trigger>
              <Button
                isIconOnly
                aria-label={isRunning ? `停止环境 ${env.name}` : `启动环境 ${env.name}`}
                className={isRunning ? "btn-action--warn" : ""}
                size="sm"
                variant={isRunning ? "outline" : "secondary"}
                onPress={() => onToggle(env)}
              >
                {isRunning ? <IconStop className="size-4" /> : <IconPlay className="size-4" />}
              </Button>
            </Tooltip.Trigger>
            <Tooltip.Content>
              {isRunning ? "停止后会中断该环境的代理服务，可再次启动" : "启动该环境的代理服务"}
            </Tooltip.Content>
          </Tooltip>

          <Dropdown>
            <Dropdown.Trigger
              aria-label={`更多操作 ${env.name}`}
              className="h-6 px-3 text-sm text-muted"
            >
              更多
            </Dropdown.Trigger>
            <Dropdown.Popover placement="bottom end">
              <Dropdown.Menu
                aria-label={`环境 ${env.name} 操作`}
                onAction={(key) => {
                  if (key === "detail") onViewDetail(env);
                  else if (key === "capture") onOpenCapture(env);
                  else if (key === "restart") onRestart(env);
                  else if (key === "delete") onDelete(env);
                }}
              >
                <Dropdown.Item id="detail" textValue="查看详情">查看详情</Dropdown.Item>
                <Dropdown.Item id="capture" textValue="抓包日志">抓包日志</Dropdown.Item>
                <Dropdown.Item id="restart" textValue="重启" isDisabled={!isRunning}>
                  重启
                </Dropdown.Item>
                <Separator className="my-1" />
                <Dropdown.Item id="delete" textValue="删除" className="text-danger">
                  删除…
                </Dropdown.Item>
              </Dropdown.Menu>
            </Dropdown.Popover>
          </Dropdown>
        </div>
      </Table.Cell>
    </Table.Row>
  );
}
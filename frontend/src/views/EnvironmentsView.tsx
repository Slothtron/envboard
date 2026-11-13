import { useEffect, useMemo, useState } from "react";
import {
  Button,
  Chip,
  SearchField,
  Separator,
  Table,
  Tabs,
  Tooltip,
} from "@heroui/react";

import type { EnvPatchReq, EnvView } from "../api/types";
import { envNeedsAttention, healthLabel, healthTone, listenAddress, type EnvSection } from "../data";
import { ProxiesView, RulesView } from "./CatalogViews";
import {
  IconBolt,
  IconChevronDown,
  IconPlus,
  IconRestart,
  IconStop,
  IconPlay,
  IconTrash,
} from "../icons";
import { EmptyState, ErrorState, FilterButton, TableSkeleton } from "../shared";
import { CaptureView } from "./CaptureView";
import { EnvironmentDetail } from "./EnvironmentDetail";

export type LoadStatus = "loading" | "error" | "ready";
type SortKey = "name" | "port" | "status";
type MetricFilter = "all" | "running" | "issues" | "stopped";

const SORT_LABEL: Record<SortKey, string> = {
  name: "按名称",
  port: "按端口",
  status: "按状态",
};

const SORT_CYCLE: SortKey[] = ["name", "port", "status"];

const TONE_CHIP: Record<string, "success" | "warning" | "danger" | "default"> = {
  success: "success",
  warning: "warning",
  danger: "danger",
  muted: "default",
};

const TONE_DOT: Record<string, string> = {
  success: "bg-success",
  warning: "bg-warning",
  danger: "bg-danger",
  muted: "bg-muted",
};

export interface EnvironmentsViewProps {
  environments: EnvView[];
  status: LoadStatus;
  isRefreshing: boolean;
  onRetry: () => void;
  /** 顶栏指标筛选，与顶栏共享状态 */
  metricFilter: MetricFilter;
  onMetricFilterChange: (key: MetricFilter) => void;
  selected: string | null;
  onSelect: (name: string) => void;
  onToggle: (env: EnvView) => void;
  onRestart: (env: EnvView) => void;
  onDelete: (env: EnvView) => void;
  onNew: () => void;
  onCopied: () => void;
  onSaveConfig: (name: string, patch: EnvPatchReq) => void;
  /** 资源子页签（规则/代理）写操作反馈走全局反馈区 */
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
  const [sortKey, setSortKey] = useState<SortKey>("name");
  const [detailOpen, setDetailOpen] = useState(false);
  const [section, setSection] = useState<EnvSection>("instances");
  /* 抓包日志 = 环境子页：命中的环境由列表行入口写入（ADR-011）。 */
  const [captureEnv, setCaptureEnv] = useState<string | null>(null);

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
    list = [...list].sort((a, b) => {
      if (sortKey === "port") return a.listen.port - b.listen.port;
      if (sortKey === "status") return a.health.localeCompare(b.health);
      return a.name.localeCompare(b.name);
    });
    return list;
  }, [environments, metricFilter, search, sortKey]);

  const active = environments.find((e) => e.name === selected) ?? null;
  const activeCapture = useMemo(
    () => (captureEnv ? (environments.find((e) => e.name === captureEnv) ?? null) : null),
    [environments, captureEnv],
  );

  /* 选中环境被删除时自动收起抽屉 */
  useEffect(() => {
    if (!active) setDetailOpen(false);
  }, [active]);

  /* 抓包子页里的环境被删除时自动退回列表 */
  useEffect(() => {
    if (captureEnv && !activeCapture) setCaptureEnv(null);
  }, [captureEnv, activeCapture]);

  /* Esc 关闭抽屉：react-aria 列表在冒泡阶段 stopPropagation，
     故挂 window 捕获阶段；模态弹层打开时让位给弹层自身（设计规范 D-7） */
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

  /* 抓包日志子页：同一组件实例内早返回，返回后搜索/排序/页签/抽屉状态原样保留（ADR-011）。 */
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
      className="flex min-h-0 flex-1 flex-col"
      selectedKey={section}
      variant="secondary"
      onSelectionChange={(key) => {
        const next = key === "rules" || key === "proxies" ? key : "instances";
        setSection(next);
        // 资源页签不消费环境抽屉：切走时收起，避免遮挡目录列表
        if (next !== "instances") setDetailOpen(false);
      }}
    >
      <Tabs.ListContainer>
        <Tabs.List aria-label="环境工作台">
          <Tabs.Tab id="instances">
            实例
            <Chip className="ml-2" size="sm" variant="soft">
              <Chip.Label>{environments.length}</Chip.Label>
            </Chip>
            <Tabs.Indicator />
          </Tabs.Tab>
          <Tabs.Tab id="rules">
            规则集
            <Tabs.Indicator />
          </Tabs.Tab>
          <Tabs.Tab id="proxies">
            上游代理
            <Tabs.Indicator />
          </Tabs.Tab>
        </Tabs.List>
      </Tabs.ListContainer>

      <Tabs.Panel className="flex min-h-0 flex-1 flex-col" id="instances">
      <div className="flex min-h-0 flex-1 flex-col">
        {/* 单一筛选入口：搜索框 + 常驻 chips（带计数与激活态，D-8） */}
        <div className="flex flex-wrap items-center gap-2 border-b border-border bg-surface px-6 py-3">
          <SearchField
            aria-label="搜索环境、端口或规则"
            className="flex-none"
            value={search}
            onChange={setSearch}
          >
            <SearchField.Group>
              <SearchField.SearchIcon />
              <SearchField.Input className="w-40 xl:w-56" placeholder="搜索环境 / 端口 / 规则" />
              <SearchField.ClearButton />
            </SearchField.Group>
          </SearchField>

          <div aria-label="按状态筛选" className="flex flex-wrap items-center gap-2" role="group">
            <FilterButton
              count={counts.all}
              isActive={metricFilter === "all"}
              onPress={() => onMetricFilterChange("all")}
            >
              全部
            </FilterButton>
            <FilterButton
              count={counts.running}
              isActive={metricFilter === "running"}
              onPress={() => onMetricFilterChange("running")}
            >
              运行中
            </FilterButton>
            <FilterButton
              count={counts.stopped}
              isActive={metricFilter === "stopped"}
              onPress={() => onMetricFilterChange("stopped")}
            >
              已停止
            </FilterButton>
            <FilterButton
              count={counts.issues}
              isActive={metricFilter === "issues"}
              onPress={() => onMetricFilterChange("issues")}
            >
              需处理
            </FilterButton>
          </div>

          <Separator orientation="vertical" className="h-5" />

          <Button
            size="sm"
            variant="ghost"
            onPress={() =>
              setSortKey(
                (cur: SortKey) =>
                  SORT_CYCLE[(SORT_CYCLE.indexOf(cur) + 1) % SORT_CYCLE.length] ?? "name",
              )
            }
          >
            <IconChevronDown className="size-4 text-muted" />
            {SORT_LABEL[sortKey]}
          </Button>

          <div className="ml-auto flex items-center gap-2">
            <span className="hidden text-sm text-muted sm:inline">双击行查看详情</span>
            {/* 该视图唯一的主操作 */}
            <Button size="sm" variant="primary" onPress={onNew}>
              <IconPlus className="size-4" />
              新建环境
            </Button>
          </div>
        </div>

        <div className="relative flex min-h-0 flex-1 overflow-hidden">
          {/* 列表列：始终独占整行；详情抽屉是悬浮层，不挤占列表宽度（ADR-011） */}
          <div className="flex min-h-0 min-w-0 flex-1 flex-col">
            <div className="min-h-0 flex-1 overflow-y-auto">
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
                <Table>
                  <Table.ScrollContainer data-full-bleed>
                    <Table.Content
                      aria-label="环境列表"
                      className="min-w-192"
                      selectedKeys={selected ? [selected] : []}
                      selectionMode="single"
                      onSelectionChange={(keys) => {
                        const next = Array.from(keys as Set<string | number>)[0];
                        // 单击只负责高亮；双击才打开详情（ADR-011）
                        if (next != null) onSelect(String(next));
                      }}
                    >
                      <Table.Header>
                        <Table.Column isRowHeader>环境</Table.Column>
                        <Table.Column>状态</Table.Column>
                        <Table.Column>监听</Table.Column>
                        <Table.Column>规则</Table.Column>
                        <Table.Column className="text-end">操作</Table.Column>
                      </Table.Header>
                      <Table.Body
                        items={visible}
                        renderEmptyState={() => (
                          <EmptyState
                            title="没有匹配的环境"
                            hint={
                              search || metricFilter !== "all"
                                ? "当前搜索词或状态筛选排除了全部环境。清除筛选即可看到完整列表。"
                                : "还没有任何环境。新建一个环境后，它会获得一个 16000–16999 之间的端口。"
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
                        )}
                      >
                        {(env) => (
                          <EnvRow
                            env={env}
                            onDelete={onDelete}
                            onOpenCapture={(e) => {
                              setDetailOpen(false);
                              setCaptureEnv(e.name);
                            }}
                            onRestart={onRestart}
                            onToggle={onToggle}
                            onViewDetail={(e) => {
                              onSelect(e.name);
                              setDetailOpen(true);
                            }}
                          />
                        )}
                      </Table.Body>
                    </Table.Content>
                  </Table.ScrollContainer>
                </Table>
              ) : null}
            </div>
          </div>

          {/* 右侧悬浮面板：非模态，打开后单击列表行切换选中、双击切换内容（ADR-011） */}
          <aside
            aria-label="环境详情"
            aria-hidden={!detailOpen}
            className={`wb-detail-panel wb-detail-panel--env ${detailOpen ? "wb-detail-panel--open" : ""}`}
            data-env={active?.name}
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
  onToggle,
  onRestart,
  onDelete,
  onOpenCapture,
  onViewDetail,
}: {
  env: EnvView;
  onToggle: (env: EnvView) => void;
  onRestart: (env: EnvView) => void;
  onDelete: (env: EnvView) => void;
  onOpenCapture: (env: EnvView) => void;
  onViewDetail: (env: EnvView) => void;
}) {
  const isRunning = env.health === "running";
  const tone = healthTone(env.health);
  const hasIssue = envNeedsAttention(env);

  return (
    <Table.Row
      className="cursor-pointer"
      data-env-health={env.health}
      data-env-row={env.name}
      id={env.name}
      onDoubleClick={() => onViewDetail(env)}
    >
      <Table.Cell>
        <span className="flex min-w-0 flex-col gap-1">
          <span className="truncate font-mono text-base font-semibold text-foreground">
            {env.name}
          </span>
          <span className="truncate text-sm text-muted">{env.description}</span>
        </span>
      </Table.Cell>

      <Table.Cell>
        <Chip
          className="whitespace-nowrap"
          color={TONE_CHIP[tone]}
          size="sm"
          variant="soft"
          title={env.health_reason ?? undefined}
        >
          <span aria-hidden="true" className={`size-2 rounded-full ${TONE_DOT[tone]}`} />
          <Chip.Label>{healthLabel(env.health)}</Chip.Label>
        </Chip>
        {hasIssue ? (
          <Chip className="ml-2" color="warning" size="sm" variant="soft">
            <Chip.Label>待处理</Chip.Label>
          </Chip>
        ) : null}
      </Table.Cell>

      <Table.Cell>
        <span className="whitespace-nowrap font-mono text-base text-muted">
          {listenAddress(env)}
        </span>
      </Table.Cell>

      <Table.Cell>
        {env.rules === null ? (
          <span className="whitespace-nowrap font-mono text-base text-muted">（不覆盖）</span>
        ) : env.rules_missing ? (
          <span className="whitespace-nowrap font-mono text-base text-warning">
            {env.rules}（缺失）
          </span>
        ) : (
          <span className="whitespace-nowrap font-mono text-base text-foreground">
            {env.rules}
            <span className="text-muted">（{env.rules_count} 条）</span>
          </span>
        )}
      </Table.Cell>

      {/* 上游代理与鉴权已从列表移除（详情「配置」里仍可见，ADR-011） */}

      {/* 操作常驻，不依赖 hover（P7）；按「后果是否可逆」分级（D-4） */}
      <Table.Cell>
        <div className="flex items-center justify-end gap-1">
          <Tooltip delay={200}>
            <Tooltip.Trigger>
              <Button
                isIconOnly
                aria-label={`打开抓包日志 ${env.name}`}
                size="sm"
                variant="ghost"
                onPress={() => onOpenCapture(env)}
              >
                <IconBolt className="size-4" />
              </Button>
            </Tooltip.Trigger>
            <Tooltip.Content>抓包日志（环境子页）</Tooltip.Content>
          </Tooltip>

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

          {/* 重启仅在运行时可用；禁用时不挂 Tooltip（react-aria 不给不可按压元素接事件） */}
          {isRunning ? (
            <Tooltip delay={200}>
              <Tooltip.Trigger>
                <Button
                  isIconOnly
                  aria-label={`重启环境 ${env.name}`}
                  size="sm"
                  variant="ghost"
                  onPress={() => onRestart(env)}
                >
                  <IconRestart className="size-4" />
                </Button>
              </Tooltip.Trigger>
              <Tooltip.Content>重启（仅在运行时可用）</Tooltip.Content>
            </Tooltip>
          ) : (
            <Button
              aria-label={`重启环境 ${env.name}：仅在运行时可用`}
              isIconOnly
              size="sm"
              variant="ghost"
              isDisabled
            >
              <IconRestart className="size-4" />
            </Button>
          )}

          <Tooltip delay={200}>
            <Tooltip.Trigger>
              <Button
                isIconOnly
                aria-label={`删除环境 ${env.name}`}
                className="btn-action--danger"
                size="sm"
                variant="outline"
                onPress={() => onDelete(env)}
              >
                <IconTrash className="size-4" />
              </Button>
            </Tooltip.Trigger>
            <Tooltip.Content>删除环境（二次确认）</Tooltip.Content>
          </Tooltip>
        </div>
      </Table.Cell>
    </Table.Row>
  );
}

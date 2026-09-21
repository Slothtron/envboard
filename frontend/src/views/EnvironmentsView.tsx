import { useEffect, useMemo, useState } from "react";
import {
  Button,
  Chip,
  SearchField,
  Separator,
  Table,
  Tooltip,
} from "@heroui/react";

import type { EnvPatchReq, EnvView } from "../api/types";
import { envNeedsAttention, healthLabel, healthTone, listenAddress } from "../data";
import {
  IconChevronDown,
  IconEye,
  IconPlus,
  IconRestart,
  IconStop,
  IconPlay,
  IconTrash,
} from "../icons";
import { EmptyState, ErrorState, FilterButton, TableSkeleton } from "../shared";
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
}: EnvironmentsViewProps) {
  const [search, setSearch] = useState("");
  const [sortKey, setSortKey] = useState<SortKey>("name");
  const [detailOpen, setDetailOpen] = useState(false);

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

  /* 选中环境被删除时自动收起抽屉 */
  useEffect(() => {
    if (!active) setDetailOpen(false);
  }, [active]);

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

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex min-h-0 flex-1 flex-col">
        {/* 单一筛选入口：搜索框 + 常驻 chips（带计数与激活态，D-8） */}
        <div className="flex flex-wrap items-center gap-2 border-b border-border bg-surface px-5 py-3">
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
            {/* 该视图唯一的主操作 */}
            <Button size="sm" variant="primary" onPress={onNew}>
              <IconPlus className="size-4" />
              新建环境
            </Button>
          </div>
        </div>

        <div className="flex min-h-0 flex-1">
          {/* 列表列：抽屉关闭时独占整行，打开时与抽屉并排 */}
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
                  <Table.ScrollContainer>
                    <Table.Content
                      aria-label="环境列表"
                      className="min-w-240"
                      selectedKeys={selected ? [selected] : []}
                      selectionMode="single"
                      onSelectionChange={(keys) => {
                        const next = Array.from(keys as Set<string | number>)[0];
                        if (next != null) onSelect(String(next));
                      }}
                    >
                      <Table.Header>
                        <Table.Column isRowHeader>环境</Table.Column>
                        <Table.Column>状态</Table.Column>
                        <Table.Column>监听</Table.Column>
                        <Table.Column>规则绑定</Table.Column>
                        <Table.Column>上游代理</Table.Column>
                        <Table.Column>鉴权</Table.Column>
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

          {/* 右侧抽屉：非模态停靠面板，打开后单击列表行切换内容（D-7） */}
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
    </div>
  );
}

/* ------------------------------------------------------------------ */

function EnvRow({
  env,
  onToggle,
  onRestart,
  onDelete,
  onViewDetail,
}: {
  env: EnvView;
  onToggle: (env: EnvView) => void;
  onRestart: (env: EnvView) => void;
  onDelete: (env: EnvView) => void;
  onViewDetail: (env: EnvView) => void;
}) {
  const isRunning = env.health === "running";
  const tone = healthTone(env.health);
  const hasIssue = envNeedsAttention(env);

  return (
    <Table.Row id={env.name} data-env-row={env.name} data-env-health={env.health}>
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

      <Table.Cell>
        <span
          className={`whitespace-nowrap font-mono text-base ${env.upstream ? "text-foreground" : "text-muted"}`}
        >
          {env.upstream ?? "直连"}
        </span>
      </Table.Cell>

      <Table.Cell>
        <span
          className={`whitespace-nowrap font-mono text-base ${env.proxy_auth_enabled ? "text-foreground" : "text-muted"}`}
        >
          {env.proxy_auth_enabled ? "已启用" : "未启用"}
        </span>
      </Table.Cell>

      {/* 操作常驻，不依赖 hover（P7）；按「后果是否可逆」分级（D-4） */}
      <Table.Cell>
        <div className="flex items-center justify-end gap-1">
          <Tooltip delay={200}>
            <Tooltip.Trigger>
              <Button
                isIconOnly
                aria-label={`查看环境详情 ${env.name}`}
                size="sm"
                variant="ghost"
                onPress={() => onViewDetail(env)}
              >
                <IconEye className="size-4" />
              </Button>
            </Tooltip.Trigger>
            <Tooltip.Content>在右侧抽屉中查看详情</Tooltip.Content>
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

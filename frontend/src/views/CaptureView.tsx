import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  Button,
  Chip,
  Kbd,
  Separator,
  Tabs,
  Tooltip,
} from "@heroui/react";

import { api, downloadText } from "../api/client";
import { openDebugStream } from "../api/stream";
import {
  type CaptureRecord,
  type DebugView as DebugViewState,
  type EnvView,
} from "../api/types";
import { formatBytes, listenAddress } from "../data";
import { BodyBlock, contentTypeOf } from "../bodyFormat";
import { CaptureTable } from "./CaptureTable";
import { ConfirmDialog } from "../dialogs";
import { formatErrorNotice } from "../errors";
import {
  IconArrowLeft,
  IconCheck,
  IconClose,
  IconCopy,
  IconDownload,
  IconPlay,
  IconStop,
  IconTrash,
} from "../icons";
import {
  EmptyState,
  ErrorState,
  Hint,
  Kv,
  KvGrid,
  MethodChip,
  SectionTitle,
  TableSkeleton,
  statusTone,
} from "../shared";

type CapFilter = "all" | "err" | "s2" | "s3" | "s4" | "s5";

/** 资源类型（浏览器 DevTools Network 风格）：与状态过滤正交，AND 组合。 */
type ResFilter = "all" | "fetch" | "doc" | "css" | "js" | "img" | "media" | "font" | "other";

const CAP_FILTERS: Array<{ key: CapFilter; label: string }> = [
  { key: "all", label: "全部" },
  { key: "err", label: "异常" },
  { key: "s2", label: "2xx" },
  { key: "s3", label: "3xx" },
  { key: "s4", label: "4xx" },
  { key: "s5", label: "5xx" },
];

const RES_FILTERS: Array<{ key: ResFilter; label: string }> = [
  { key: "all", label: "全部类型" },
  { key: "fetch", label: "Fetch/XHR" },
  { key: "doc", label: "文档" },
  { key: "css", label: "CSS" },
  { key: "js", label: "JS" },
  { key: "img", label: "图片" },
  { key: "media", label: "媒体" },
  { key: "font", label: "字体" },
  { key: "other", label: "其他" },
];

function matchFilter(cap: CaptureRecord, filter: CapFilter): boolean {
  const status = cap.response?.status ?? 0;
  switch (filter) {
    case "err":
      return status >= 400 || cap.error !== null;
    case "s2":
      return status >= 200 && status < 300;
    case "s3":
      return status >= 300 && status < 400;
    case "s4":
      return status >= 400 && status < 500;
    case "s5":
      return status >= 500;
    default:
      return true;
  }
}

/**
 * 资源类型判定：优先看响应 content-type，缺失时看 path 扩展名。
 * 抓包记录不含浏览器 initiator type，故 Fetch/XHR 取「JSON/XML/文本数据」这类
 * 非文档、非静态资源的请求（现代 SPA 的 API 调用绝大多数落这里）。
 */
function resourceTypeOf(cap: CaptureRecord): Exclude<ResFilter, "all"> {
  const ct =
    cap.response?.headers.find(([k]) => k.toLowerCase() === "content-type")?.[1]?.toLowerCase() ??
    "";
  if (ct.startsWith("text/html") || ct === "application/xhtml+xml") return "doc";
  if (ct.includes("json") || ct.includes("xml") || ct === "text/plain") return "fetch";
  if (ct.includes("javascript") || ct.includes("ecmascript")) return "js";
  if (ct.includes("text/css")) return "css";
  if (ct.startsWith("image/")) return "img";
  if (ct.startsWith("audio/") || ct.startsWith("video/")) return "media";
  if (ct.includes("font") || ct.includes("woff")) return "font";
  // content-type 缺失：按扩展名兜底（如实心 favicon / 脚本 / 样式）。
  const path = cap.request.path.toLowerCase();
  if (/\.(css)$/.test(path)) return "css";
  if (/\.(js|mjs|cjs|jsx|ts)$/.test(path)) return "js";
  if (/\.(png|jpe?g|gif|svg|webp|ico|bmp|avif)$/.test(path)) return "img";
  if (/\.(mp3|mp4|webm|ogg|wav|flac|m4a)$/.test(path)) return "media";
  if (/\.(woff2?|ttf|otf|eot)$/.test(path)) return "font";
  if (/\.(html?|htm|xhtml)$/.test(path)) return "doc";
  if (/\.(json|xml)$/.test(path)) return "fetch";
  return "other";
}

export interface CaptureViewProps {
  /** 列表行入口锁定的环境（工作区级抓包会话仍按环境单例）。 */
  env: EnvView;
  /** 返回环境实例列表；子页状态由 EnvironmentsView 持有，返回不丢搜索/排序。 */
  onBack: () => void;
  onFeedback: (status: "success" | "warning" | "danger", title: string, detail?: string) => void;
}

/**
 * 抓包日志（环境子页，ADR-011）：入口在实例列表操作列，不再占顶层导航位。
 * 进入即接管/开启该环境的单例抓包会话；HAR 导入前端入口已退场（后端命令保留）。
 */
export function CaptureView({ env, onBack, onFeedback }: CaptureViewProps) {
  const [debug, setDebug] = useState<DebugViewState | null>(null);
  const [status, setStatus] = useState<"loading" | "error" | "ready">("loading");
  const [capFilter, setCapFilter] = useState<CapFilter>("all");
  const [resFilter, setResFilter] = useState<ResFilter>("all");
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [drawerOpen, setDrawerOpen] = useState(false);
  /* 清空是不可逆销毁数据：与删除环境/规则/代理同档，走二次确认（D-4）。 */
  const [confirmClear, setConfirmClear] = useState(false);
  /* 抽屉焦点管理：打开聚焦面板，关闭还给触发行。 */
  const panelRef = useRef<HTMLElement>(null);
  const returnFocusRef = useRef<number | null>(null);

  /* 抓包增量按 request_id 去重合并（events 帧是游标之后的新记录），并同步计数。
     未变化的记录保持原对象引用 → memo 行整棵跳过重渲染（窗口化表格的前提）。
     oldest = 后端缓冲内最旧 id：字节预算淘汰的记录前端同步丢弃，
     否则长会话下前端只增不减，渲染与内存无界增长（卡顿根因之一）。 */
  const mergeRecords = useCallback(
    (incoming: CaptureRecord[], captured: number, dropped: number, oldest: number | null) => {
      setDebug((prev) => {
        if (!prev?.capture) return prev;
        const byId = new Map(prev.capture.records.map((r) => [r.request_id, r]));
        for (const rec of incoming) byId.set(rec.request_id, rec);
        let records = [...byId.values()].sort((a, b) => a.request_id - b.request_id);
        if (oldest != null) records = records.filter((r) => r.request_id >= oldest);
        return {
          ...prev,
          capture: { ...prev.capture, records, captured, dropped },
        };
      });
    },
    [],
  );

  /* 进入子页：接管同环境会话；否则在环境运行时自动开启。未运行则只读空态。 */
  useEffect(() => {
    let cancelled = false;
    setStatus("loading");
    setDebug(null);
    setSelectedId(null);
    setDrawerOpen(false);
    setCapFilter("all");
    setResFilter("all");

    const running = env.health === "running";
    api
      .debugGet()
      .then((current) => {
        if (cancelled) return null;
        if (current.env === env.name) {
          setDebug(current);
          setStatus("ready");
          return null;
        }
        if (!running) {
          setDebug({ env: null, capture: null });
          setStatus("ready");
          return null;
        }
        return api.debugStart(env.name).then((started) => {
          if (!cancelled) {
            setDebug(started);
            setStatus("ready");
          }
          return null;
        });
      })
      .catch(() => {
        if (!cancelled) setStatus("error");
      });

    /* events 帧合流：500ms 一拍（突发时更密），逐帧 setState 会让渲染追不上推送。
       攒进 pending，rAF 里一次性合并提交；快照帧先到先清 pending（整幅覆盖增量）。 */
    let pending: { records: CaptureRecord[]; captured: number; dropped: number; oldest: number | null } | null = null;
    let raf = 0;
    const flush = () => {
      raf = 0;
      if (!pending || cancelled) return;
      const p = pending;
      pending = null;
      mergeRecords(p.records, p.captured, p.dropped, p.oldest);
    };
    const stream = openDebugStream(
      (frame) => {
        if (cancelled) return;
        pending = null;
        setDebug({ env: frame.env, capture: frame.capture });
        setStatus("ready");
      },
      (frame) => {
        if (cancelled) return;
        if (pending) {
          pending.records.push(...frame.records);
          pending.captured = frame.captured;
          pending.dropped = frame.dropped;
          pending.oldest = frame.oldest;
        } else {
          pending = { records: [...frame.records], captured: frame.captured, dropped: frame.dropped, oldest: frame.oldest };
        }
        if (raf === 0) raf = requestAnimationFrame(flush);
      },
      () => {
        /* 抓包流断线自动重连；连接态由快照流统一展示 */
      },
    );
    return () => {
      cancelled = true;
      if (raf !== 0) cancelAnimationFrame(raf);
      stream.close();
    };
    // 只按环境身份重建：健康态变化不应重开会话（是否自动开启取进入瞬间的状态）。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [env.name, mergeRecords]);

  const ownsSession = debug?.env === env.name && debug.capture != null;
  const capturing = ownsSession;
  const records = ownsSession ? (debug?.capture?.records ?? []) : [];

  /* 单趟扫描：每条记录的资源类型只算一次（原实现 8 个过滤键各扫一遍，
     resourceTypeOf 的 header find + 正则被重复执行 8N 次）。 */
  const { counts, resTypes } = useMemo(() => {
    const cap = { all: records.length, err: 0, s2: 0, s3: 0, s4: 0, s5: 0 } satisfies Record<CapFilter, number>;
    const res = { all: records.length, fetch: 0, doc: 0, css: 0, js: 0, img: 0, media: 0, font: 0, other: 0 } satisfies Record<ResFilter, number>;
    const types = new Map<number, Exclude<ResFilter, "all">>();
    for (const c of records) {
      const status = c.response?.status ?? 0;
      if (status >= 400 || c.error !== null) cap.err += 1;
      if (status >= 200 && status < 300) cap.s2 += 1;
      else if (status >= 300 && status < 400) cap.s3 += 1;
      else if (status >= 400 && status < 500) cap.s4 += 1;
      else if (status >= 500) cap.s5 += 1;
      const t = resourceTypeOf(c);
      types.set(c.request_id, t);
      res[t] += 1;
    }
    return { counts: { cap, res }, resTypes: types };
  }, [records]);

  const visible = useMemo(
    () =>
      records.filter(
        (c) => matchFilter(c, capFilter) && (resFilter === "all" || resTypes.get(c.request_id) === resFilter),
      ),
    [records, capFilter, resFilter, resTypes],
  );

  const current = useMemo(
    () => records.find((c) => c.request_id === selectedId) ?? null,
    [records, selectedId],
  );

  /* 当前详情记录被清空（capture/clear 或会话换代）时自动收起面板 */
  useEffect(() => {
    if (!current) setDrawerOpen(false);
  }, [current]);

  /* Esc：先关详情面板，再退回环境列表；模态在场时让位（设计规范 D-7） */
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (document.querySelector('[role="dialog"], [role="alertdialog"]')) return;
      if (drawerOpen) {
        setDrawerOpen(false);
        setSelectedId(null);
        return;
      }
      onBack();
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [drawerOpen, onBack]);

  /* 抽屉打开 → 焦点移进面板；关闭 → 还给触发行（键盘可达性）。 */
  useEffect(() => {
    if (drawerOpen) {
      panelRef.current?.focus();
    } else if (returnFocusRef.current != null) {
      const row = document.querySelector(
        `tr[data-cap-row="${returnFocusRef.current}"]`,
      );
      (row as HTMLElement | null)?.focus();
      returnFocusRef.current = null;
    }
  }, [drawerOpen]);

  /* 稳定引用：memo 行的 props 不因父组件重渲染而变化。 */
  const handleSelect = useCallback((id: number) => setSelectedId(id), []);
  const handleOpen = useCallback((cap: CaptureRecord) => {
    returnFocusRef.current = cap.request_id;
    setSelectedId(cap.request_id);
    setDrawerOpen(true);
  }, []);

  /* 键盘等价于双击：Enter/Space 打开焦点行的详情；↑/↓ 在行间移动焦点并选中
     （原生 tr 自带焦点，roving tabindex 见 CaptureRow）。挂容器捕获阶段，
     只响应行自身，不截获单元格内控件的按键。 */
  const onListKeyDownCapture = (e: React.KeyboardEvent) => {
    const target = e.target as HTMLElement;
    const row = target.closest?.("tr[data-cap-row]") as HTMLElement | null;
    if (!row || target !== row) return;
    if (e.key === "Enter" || e.key === " ") {
      const cap = records.find((r) => String(r.request_id) === row.dataset.capRow);
      if (!cap) return;
      e.preventDefault();
      e.stopPropagation();
      handleOpen(cap);
      return;
    }
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      const sibling = (e.key === "ArrowDown" ? row.nextElementSibling : row.previousElementSibling) as HTMLElement | null;
      const next = sibling?.matches("tr[data-cap-row]") ? sibling : null;
      if (!next) return;
      e.preventDefault();
      const id = Number(next.dataset.capRow);
      setSelectedId(id);
      next.focus();
    }
  };

  function reload() {
    setStatus("loading");
    api
      .debugGet()
      .then((d) => {
        setDebug(d);
        setStatus("ready");
      })
      .catch(() => setStatus("error"));
  }

  function startCapture() {
    api
      .debugStart(env.name)
      .then((d) => {
        setDebug(d);
        onFeedback("success", `已在环境 ${env.name} 开启抓包`);
      })
      .catch((error: unknown) => {
        const f = formatErrorNotice(error, `在 ${env.name} 开启抓包`);
        onFeedback("danger", f.title, f.detail);
      });
  }

  function stopCapture() {
    api
      .debugStop()
      .then(() => {
        setDebug({ env: null, capture: null });
        onFeedback("warning", "已停止抓包，已捕获的会话保留");
      })
      .catch((error: unknown) => {
        const f = formatErrorNotice(error, "停止抓包");
        onFeedback("danger", f.title, f.detail);
      });
  }

  function clearCapture() {
    if (!debug?.env) return;
    const target = debug.env;
    api
      .envCaptureClear(target)
      .then(() => {
        setDebug((prev) =>
          prev?.capture ? { ...prev, capture: { ...prev.capture, records: [], captured: 0 } } : prev,
        );
        onFeedback("warning", "抓包会话已清空");
      })
      .catch((error: unknown) => {
        const f = formatErrorNotice(error, `清空 ${target} 抓包`);
        onFeedback("danger", f.title, f.detail);
      });
  }

  return (
    <div aria-label="抓包日志" className="flex min-h-0 flex-1 flex-col">
      {/* 工具条两行（第七轮）：第一行身份+会话动作（单行自然对齐），
          第二行紧凑过滤 chip（12px/20px 高，状态|资源单行横排，超宽横滚）。
          旧版两行 32px 过滤按钮把整条工具栏撑到 121px，单行项被居中显得错位。 */}
      <div className="flex flex-col border-b border-border bg-surface px-4">
        <div className="flex flex-wrap items-center gap-2 py-1.5">
          <Button aria-label="返回环境列表" size="sm" variant="ghost" onPress={onBack}>
            <IconArrowLeft className="size-4" />
            环境列表
          </Button>
          <Separator orientation="vertical" className="h-5" />
          <span className="flex min-w-0 items-baseline gap-2">
            <span className="truncate font-mono text-sm font-semibold text-foreground">
              {env.name}
            </span>
            <span className="whitespace-nowrap font-mono text-xs text-muted">
              {listenAddress(env)}
            </span>
          </span>

          {!capturing ? (
            <Button
              isDisabled={env.health !== "running"}
              size="sm"
              variant="primary"
              onPress={startCapture}
            >
              <IconPlay className="size-4" />
              开启抓包
            </Button>
          ) : null}

          {debug?.capture ? (
            <Chip color={capturing ? "success" : "default"} size="sm" variant="soft">
              <span
                aria-hidden="true"
                className={`ph-led ${capturing ? "ph-led--ok ph-led--live" : "ph-led--idle"}`}
              />
              <Chip.Label>
                会话 #{debug.capture.session.id} · 已捕获 {debug.capture.captured}
                {debug.capture.dropped > 0 ? ` · 丢弃 ${debug.capture.dropped}` : ""}
              </Chip.Label>
            </Chip>
          ) : null}

          <div className="ml-auto flex items-center gap-2">
            <Button
              isDisabled={!debug?.env || records.length === 0}
              size="sm"
              variant="secondary"
              onPress={async () => {
                if (!debug?.env) return;
                try {
                  const { filename, text } = await api.envCapturesExport(debug.env, "har");
                  downloadText(filename, text, "application/json");
                  onFeedback("success", "HAR 导出已开始");
                } catch (error) {
                  const f = formatErrorNotice(error, "导出 HAR");
                  onFeedback("danger", f.title, f.detail);
                }
              }}
            >
              <IconDownload className="size-4" />
              导出 HAR
            </Button>

            {capturing ? (
              <Tooltip delay={200}>
                <Tooltip.Trigger>
                  <Button className="btn-action--warn" size="sm" variant="outline" onPress={stopCapture}>
                    <IconStop className="size-4" />
                    停止
                  </Button>
                </Tooltip.Trigger>
                <Tooltip.Content>停止抓包（已捕获的会话保留，可重新开始）</Tooltip.Content>
              </Tooltip>
            ) : null}

            <Button
              aria-label="清空抓包会话"
              className="btn-action--danger"
              size="sm"
              variant="outline"
              isDisabled={!debug?.capture || records.length === 0}
              onPress={() => setConfirmClear(true)}
            >
              <IconTrash className="size-4" />
              清空
            </Button>
          </div>
        </div>

        {/* 过滤行：状态 | 资源类型 单行横排，超宽横向滚动（不折行撑高）。 */}
        <div className="wb-cap-filters flex items-center gap-1 overflow-x-auto pb-1.5">
          <div aria-label="状态过滤" className="flex flex-none items-center gap-1" role="group">
            {CAP_FILTERS.map((f) => (
              <FilterChip
                key={f.key}
                count={counts.cap[f.key]}
                isActive={capFilter === f.key}
                onPress={() => setCapFilter(f.key)}
              >
                {f.label}
              </FilterChip>
            ))}
          </div>
          <Separator orientation="vertical" className="mx-1 h-4 flex-none" />
          <div aria-label="资源类型过滤" className="flex flex-none items-center gap-1" role="group">
            {RES_FILTERS.map((f) => (
              <FilterChip
                key={f.key}
                count={counts.res[f.key]}
                isActive={resFilter === f.key}
                onPress={() => setResFilter(f.key)}
              >
                {f.label}
              </FilterChip>
            ))}
          </div>
        </div>
      </div>

      {/* 内容区：列表 + 悬浮详情面板（absolute；让宽由 --wb-drawer-w 提供） */}
      <div className="wb-content wb-content--capture relative flex min-h-0 flex-1 overflow-hidden">
        <div
          className={`${drawerOpen ? "wb-list-inset " : ""}flex min-h-0 min-w-0 flex-1 flex-col`}
          onKeyDownCapture={onListKeyDownCapture}
        >
          {status === "loading" ? (
            <TableSkeleton rows={6} />
          ) : status === "error" ? (
            <ErrorState
              description="无法读取抓包会话。确认服务已启动后重试。"
              title="抓包数据加载失败"
              onRetry={reload}
            />
          ) : !debug?.env ? (
            <EmptyState
              title={env.health === "running" ? "尚未开启抓包" : "环境未运行"}
              hint={
                env.health === "running"
                  ? "点「开启抓包」，该环境实例的 HTTPS 请求会实时出现在这里。"
                  : "启动该环境后，抓包会话才能记录经过代理的请求。"
              }
              action={
                env.health === "running" ? (
                  <Button size="sm" variant="secondary" onPress={startCapture}>
                    <IconPlay className="size-4" />
                    开启抓包
                  </Button>
                ) : undefined
              }
            />
          ) : (
            <div className="min-h-0 flex-1 overflow-hidden bg-surface">
              <CaptureTable
                records={visible}
                selectedId={selectedId}
                onSelect={handleSelect}
                onOpen={handleOpen}
                drawerOpen={drawerOpen}
                emptyState={
                  <EmptyState
                    title="没有匹配的请求"
                    hint="当前状态或资源类型过滤排除了全部抓包记录。清除过滤即可看到完整会话。"
                    action={
                      <Button
                        size="sm"
                        variant="secondary"
                        onPress={() => {
                          setCapFilter("all");
                          setResFilter("all");
                        }}
                      >
                        清除过滤
                      </Button>
                    }
                  />
                }
              />
            </div>
          )}
        </div>

        {/* 悬浮详情面板（非模态）：Esc 挂 window 捕获阶段；模态在场时让位。
            tabIndex=-1 供打开时程序聚焦；关闭后焦点还给触发行。 */}
        <aside
          aria-label="抓包详情"
          aria-hidden={!drawerOpen}
          className={`wb-detail-panel ${drawerOpen ? "wb-detail-panel--open" : ""}`}
          ref={panelRef}
          tabIndex={-1}
        >
          {drawerOpen && current ? (
            <CaptureDetail
              capture={current}
              env={env}
              onFeedback={onFeedback}
              onClose={() => {
                setDrawerOpen(false);
                setSelectedId(null);
              }}
            />
          ) : null}
        </aside>
      </div>

      <ConfirmDialog
        confirmLabel="清空"
        description={`将清空环境 ${debug?.env ?? env.name} 当前会话的全部抓包记录（${records.length} 条）。`}
        isOpen={confirmClear}
        title="确认清空抓包会话"
        onConfirm={() => {
          setConfirmClear(false);
          clearCapture();
        }}
        onOpenChange={setConfirmClear}
      />
    </div>
  );
}

/**
 * 紧凑过滤 chip（第七轮）：12px 字 / 20px 高的文本 toggle + 计数，
 * 选中 accent 淡底。替代 32px 的 FilterButton，避免两行按钮撑高工具栏。
 * 原生 button（非 HeroUI）以精确控制密度；aria-pressed 保持可访问性。
 */
function FilterChip({
  isActive,
  count,
  onPress,
  children,
}: {
  isActive: boolean;
  count?: number;
  onPress: () => void;
  children: ReactNode;
}) {
  return (
    <button
      aria-pressed={isActive}
      className="wb-fchip"
      data-active={isActive || undefined}
      onClick={onPress}
      type="button"
    >
      {children}
      {typeof count === "number" ? <span className="wb-fchip-count">{count}</span> : null}
    </button>
  );
}

/* 状态码着色唯一口径（V-36）：5xx/ERR 危险、4xx 警告、3xx 弱化、其余正文色。 */
function overviewStatusTone(
  status: number,
  hasError: boolean,
): "default" | "warning" | "danger" | "muted" {
  if (hasError || status >= 500) return "danger";
  if (status >= 400) return "warning";
  if (status >= 300) return "muted";
  return "default";
}

/* ------------------------------------------------------------------ */

function CaptureDetail({
  capture,
  env,
  onFeedback,
  onClose,
}: {
  capture: CaptureRecord;
  env: EnvView;
  onFeedback: (status: "success" | "warning" | "danger", title: string, detail?: string) => void;
  onClose: () => void;
}) {
  const [copied, setCopied] = useState(false);
  const status = capture.response?.status ?? 0;

  async function copyCurl() {
    // path 与 query 分存（v2）：拼回完整目标，query 非空才补 '?'。
    const target = `${capture.request.path}${capture.request.query ? `?${capture.request.query}` : ""}`;
    const curl = `curl -x http://${env.listen.host}:${env.listen.port} 'https://${capture.request.authority}${target}'`;
    try {
      await navigator.clipboard.writeText(curl);
    } catch {
      /* 剪贴板不可用时仍给出反馈，避免静默失败 */
    }
    setCopied(true);
    onFeedback("success", "已复制为 cURL");
    window.setTimeout(() => setCopied(false), 1600);
  }

  const overview = [
    { k: "请求 ID", v: `#${capture.request_id}` },
    { k: "会话", v: `#${capture.session}` },
    { k: "时间", v: new Date(capture.time).toLocaleString("zh-CN", { hour12: false }) },
    { k: "方法", v: capture.request.method },
    { k: "状态码", v: capture.error !== null ? "ERR" : String(status), tone: overviewStatusTone(status, capture.error !== null) },
    { k: "请求大小", v: formatBytes(capture.request.body?.size ?? 0) },
    { k: "响应大小", v: formatBytes(capture.response?.body?.size ?? 0) },
  ];

  // 参数 tab 的计数与 QueryRows 同口径（URLSearchParams，重复 key 各计一条）。
  const queryCount = useMemo(
    () => Array.from(new URLSearchParams(capture.request.query)).length,
    [capture.request.query],
  );

  return (
    <>
      <div className="flex flex-none flex-col gap-2 border-b border-border px-4 py-3">
        <div className="flex w-full items-center gap-3">
          <MethodChip method={capture.request.method} />
          <span className="min-w-0 flex-1 truncate font-mono text-lg font-semibold text-foreground">
            {capture.request.path}
          </span>
          <Chip size="sm" variant="soft">
            <Chip.Label>
              <span className={statusTone(status)}>{capture.error !== null ? "ERR" : status}</span>
            </Chip.Label>
          </Chip>
          <Button size="sm" variant="secondary" onPress={copyCurl}>
            {copied ? <IconCheck className="size-4" /> : <IconCopy className="size-4" />}
            复制为 cURL
          </Button>
          <Tooltip delay={200}>
            <Tooltip.Trigger>
              <Button aria-label="关闭详情" size="sm" variant="ghost" onPress={onClose}>
                <IconClose className="size-4" />
              </Button>
            </Tooltip.Trigger>
            <Tooltip.Content>关闭</Tooltip.Content>
          </Tooltip>
        </div>
        <div className="flex w-full items-center gap-2">
          <div className="min-w-0 flex-1">
            <SectionTitle
              trailing={
                <span className="flex flex-none items-center gap-2 text-sm text-muted">
                  <Kbd>
                    <Kbd.Content>Esc</Kbd.Content>
                  </Kbd>
                  关闭
                </span>
              }
            >
              <span className="font-mono normal-case tracking-normal">
                {capture.request.authority}
              </span>
            </SectionTitle>
          </div>
        </div>
      </div>

      <div className="flex min-h-0 flex-1 flex-col gap-3 p-4">
        {/* 上半：请求信息 */}
        <section aria-label="请求信息" className="flex min-h-0 flex-1 flex-col">
          <Tabs
            align="start"
            className="wb-dtabs flex min-h-0 flex-1 flex-col"
            defaultSelectedKey="overview"
            variant="secondary"
          >
            <Tabs.ListContainer>
              <Tabs.List aria-label="请求详情">
                <Tabs.Tab id="overview">
                  总览
                  <Tabs.Indicator />
                </Tabs.Tab>
                <Tabs.Tab id="req-headers">
                  请求头
                  <Chip className="ml-2" size="sm" variant="soft">
                    <Chip.Label>{capture.request.headers.length}</Chip.Label>
                  </Chip>
                  <Tabs.Indicator />
                </Tabs.Tab>
                <Tabs.Tab id="req-query">
                  参数
                  <Chip className="ml-2" size="sm" variant="soft">
                    <Chip.Label>{queryCount}</Chip.Label>
                  </Chip>
                  <Tabs.Indicator />
                </Tabs.Tab>
                <Tabs.Tab id="req-body">
                  请求体
                  <Tabs.Indicator />
                </Tabs.Tab>
              </Tabs.List>
            </Tabs.ListContainer>

            <Tabs.Panel className="min-h-0 flex-1 overflow-y-auto p-3" id="overview">
              <KvGrid>
                {overview.map((item) => (
                  <Kv key={item.k} k={item.k} v={item.v} tone={"tone" in item ? item.tone : "default"} />
                ))}
              </KvGrid>
            </Tabs.Panel>

            <Tabs.Panel className="min-h-0 flex-1 overflow-y-auto p-3" id="req-headers">
              <HeaderRows rows={capture.request.headers} />
            </Tabs.Panel>

            <Tabs.Panel className="min-h-0 flex-1 overflow-y-auto p-3" id="req-query">
              <QueryRows query={capture.request.query} />
            </Tabs.Panel>

            <Tabs.Panel className="min-h-0 flex-1 overflow-y-auto p-3" id="req-body">
              <BodyBlock body={capture.request.body} contentType={contentTypeOf(capture.request.headers)} />
            </Tabs.Panel>
          </Tabs>
        </section>

        <Separator className="flex-none" />

        {/* 下半：响应信息 */}
        <section aria-label="响应信息" className="flex min-h-0 flex-1 flex-col">
          <Tabs
            align="start"
            className="wb-dtabs flex min-h-0 flex-1 flex-col"
            defaultSelectedKey="resp-headers"
            variant="secondary"
          >
            <Tabs.ListContainer>
              <Tabs.List aria-label="响应详情">
                <Tabs.Tab id="resp-headers">
                  响应头
                  <Chip className="ml-2" size="sm" variant="soft">
                    <Chip.Label>{capture.response?.headers.length ?? 0}</Chip.Label>
                  </Chip>
                  <Tabs.Indicator />
                </Tabs.Tab>
                <Tabs.Tab id="resp-body">
                  响应体
                  <Tabs.Indicator />
                </Tabs.Tab>
              </Tabs.List>
            </Tabs.ListContainer>

            <Tabs.Panel className="min-h-0 flex-1 overflow-y-auto p-3" id="resp-headers">
              <HeaderRows rows={capture.response?.headers ?? []} />
            </Tabs.Panel>

            <Tabs.Panel className="min-h-0 flex-1 overflow-y-auto p-3" id="resp-body">
              <BodyBlock body={capture.response?.body ?? null} contentType={contentTypeOf(capture.response?.headers ?? [])} />
            </Tabs.Panel>
          </Tabs>
        </section>
      </div>
    </>
  );
}

/* ------------------------------------------------------------------ */

function HeaderRows({ rows }: { rows: Array<[string, string]> }) {
  return (
    <div className="flex flex-col gap-1">
      <SectionTitle>头部字段</SectionTitle>
      {rows.map(([k, v], i) => (
        <div key={`${k}-${i}`} className="wb-header-row">
          <span className="truncate text-muted" title={k}>
            {k}
          </span>
          <span className="min-w-0 break-all text-foreground">{v}</span>
        </div>
      ))}
      <Hint>头部按原始顺序展示，未做归一化。</Hint>
    </div>
  );
}

/**
 * 表格单元格截断文本：inline span 的 truncate 不生效（overflow 对非块级无效），
 * 改 block 让省略号真正裁切、不再撑破列宽。
 * **仅在实际发生截断时才挂 Tooltip**（第六轮）：短文本（如 `/`）hover 不再弹出
 * 无意义的全文气泡，也省掉每行 2 个 Tooltip 的 press/hover 订阅。截断判定在
 * hover 进入时量 `scrollWidth > clientWidth`（列宽随抽屉让宽变化，按需测量而非
 * 常驻 ResizeObserver）；Tooltip 受控 isOpen，未截断时恒关。
 * Trigger 默认渲染 div[role=button] 且可聚焦，传 tabIndex=-1 避免 tab 停靠点泛滥。
 */
export function TruncatedText({ text, className }: { text: string; className?: string }) {
  const spanRef = useRef<HTMLSpanElement>(null);
  const [open, setOpen] = useState(false);
  const [truncated, setTruncated] = useState(false);

  const measure = () => {
    const el = spanRef.current;
    if (el) setTruncated(el.scrollWidth > el.clientWidth + 1);
  };

  return (
    <Tooltip delay={200} isOpen={open && truncated} onOpenChange={setOpen}>
      <Tooltip.Trigger
        tabIndex={-1}
        className="block min-w-0"
        onPointerEnter={measure}
        onFocus={measure}
      >
        <span ref={spanRef} className={`block max-w-full truncate ${className ?? ""}`}>{text}</span>
      </Tooltip.Trigger>
      <Tooltip.Content className="wb-capture-tip">{text}</Tooltip.Content>
    </Tooltip>
  );
}

/** query 参数表格：URLSearchParams 解析（保留重复 key / 空值 / 自动解码），两列展示。
 *  用语义化 <table> 而非 HeroUI Table：详情抽屉嵌在 Tabs.Panel 里，react-aria Table
 *  在该嵌套下会触发 concurrent 渲染异常；原生表格无此问题且同样是表格形式。 */
function QueryRows({ query }: { query: string }) {
  const rows = useMemo(
    () =>
      Array.from(new URLSearchParams(query).entries()).map(([k, v], i) => ({
        id: `${k}-${i}`,
        k,
        v,
      })),
    [query],
  );
  if (rows.length === 0) {
    return <Hint>该请求没有 query 参数。</Hint>;
  }
  return (
    <div className="flex flex-col gap-2">
      <div className="overflow-auto rounded border border-border">
        <table aria-label="query 参数" className="w-full table-fixed border-collapse text-sm">
          <thead>
            <tr className="bg-surface-secondary text-left">
              <th className="w-[38%] border-b border-border px-3 py-2 font-medium text-muted">参数</th>
              <th className="border-b border-border px-3 py-2 font-medium text-muted">值</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={row.id} className="border-b border-border last:border-0">
                <td className="px-3 py-2 align-top">
                  <span className="block truncate font-mono text-muted" title={row.k}>
                    {row.k}
                  </span>
                </td>
                <td className="px-3 py-2 align-top">
                  <span className="block whitespace-pre-wrap break-all font-mono text-foreground">
                    {row.v === "" ? <span className="text-muted">（空）</span> : row.v}
                  </span>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <Hint>按 URL 解码展示；重复参数逐行列出。</Hint>
    </div>
  );
}

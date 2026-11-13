import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Button,
  Chip,
  Kbd,
  ScrollShadow,
  Separator,
  Table,
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
import { formatErrorNotice } from "../errors";
import {
  IconArrowLeft,
  IconCheck,
  IconClose,
  IconCopy,
  IconDownload,
  IconPlay,
  IconStop,
} from "../icons";
import {
  EmptyState,
  ErrorState,
  FilterButton,
  Hint,
  Kv,
  KvGrid,
  MethodChip,
  SectionTitle,
  TableSkeleton,
  statusTone,
} from "../shared";

type CapFilter = "all" | "err" | "s2" | "s3" | "s4" | "s5";

const CAP_FILTERS: Array<{ key: CapFilter; label: string }> = [
  { key: "all", label: "全部" },
  { key: "err", label: "异常" },
  { key: "s2", label: "2xx" },
  { key: "s3", label: "3xx" },
  { key: "s4", label: "4xx" },
  { key: "s5", label: "5xx" },
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
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [drawerOpen, setDrawerOpen] = useState(false);

  /* 抓包增量按 request_id 去重合并（events 帧是游标之后的新记录），并同步计数 */
  const mergeRecords = useCallback(
    (incoming: CaptureRecord[], captured: number, dropped: number) => {
      setDebug((prev) => {
        if (!prev?.capture) return prev;
        const byId = new Map(prev.capture.records.map((r) => [r.request_id, r]));
        for (const rec of incoming) byId.set(rec.request_id, rec);
        const records = [...byId.values()].sort((a, b) => a.request_id - b.request_id);
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

    const stream = openDebugStream(
      (frame) => {
        if (cancelled) return;
        setDebug({ env: frame.env, capture: frame.capture });
        setStatus("ready");
      },
      (frame) => {
        if (cancelled) return;
        mergeRecords(frame.records, frame.captured, frame.dropped);
      },
      () => {
        /* 抓包流断线自动重连；连接态由快照流统一展示 */
      },
    );
    return () => {
      cancelled = true;
      stream.close();
    };
    // 只按环境身份重建：健康态变化不应重开会话（是否自动开启取进入瞬间的状态）。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [env.name, mergeRecords]);

  const ownsSession = debug?.env === env.name && debug.capture != null;
  const capturing = ownsSession;
  const records = ownsSession ? (debug?.capture?.records ?? []) : [];

  const counts = useMemo(() => {
    return {
      all: records.length,
      err: records.filter((c) => matchFilter(c, "err")).length,
      s2: records.filter((c) => matchFilter(c, "s2")).length,
      s3: records.filter((c) => matchFilter(c, "s3")).length,
      s4: records.filter((c) => matchFilter(c, "s4")).length,
      s5: records.filter((c) => matchFilter(c, "s5")).length,
    } satisfies Record<CapFilter, number>;
  }, [records]);

  const visible = useMemo(
    () => records.filter((c) => matchFilter(c, capFilter)),
    [records, capFilter],
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
        return;
      }
      onBack();
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [drawerOpen, onBack]);

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
      {/* 面包屑工具条：返回 + 环境身份 + 会话动作 */}
      <div className="flex flex-wrap items-center gap-2 border-b border-border bg-surface px-6 py-3">
        <Button aria-label="返回环境列表" size="sm" variant="ghost" onPress={onBack}>
          <IconArrowLeft className="size-4" />
          环境列表
        </Button>
        <Separator orientation="vertical" className="h-5" />
        <span className="flex min-w-0 items-baseline gap-2">
          <span className="truncate font-mono text-base font-semibold text-foreground">
            {env.name}
          </span>
          <span className="whitespace-nowrap font-mono text-sm text-muted">
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
              className={`size-2 rounded-full ${capturing ? "bg-success" : "bg-muted"}`}
            />
            <Chip.Label>
              会话 #{debug.capture.session.id} · 已捕获 {debug.capture.captured}
              {debug.capture.dropped > 0 ? ` · 丢弃 ${debug.capture.dropped}` : ""}
            </Chip.Label>
          </Chip>
        ) : null}

        <Separator orientation="vertical" className="h-5" />

        <div aria-label="快捷过滤" className="flex flex-wrap items-center gap-2" role="group">
          {CAP_FILTERS.map((f) => (
            <FilterButton
              key={f.key}
              count={counts[f.key]}
              isActive={capFilter === f.key}
              onPress={() => setCapFilter(f.key)}
            >
              {f.label}
            </FilterButton>
          ))}
        </div>

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
          ) : (
            <Button
              aria-label="停止抓包：当前未在抓包"
              className="btn-action--warn"
              size="sm"
              variant="outline"
              isDisabled
            >
              <IconStop className="size-4" />
              停止
            </Button>
          )}

          <Button
            aria-label="清空抓包会话"
            className="btn-action--danger"
            size="sm"
            variant="outline"
            isDisabled={!debug?.capture || records.length === 0}
            onPress={clearCapture}
          >
            清空
          </Button>
        </div>
      </div>

      {/* 内容区：列表 + 悬浮详情面板（absolute，不挤占表格） */}
      <div className="relative flex min-h-0 flex-1 overflow-hidden border-t border-border">
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
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
            <Table>
              <Table.ScrollContainer data-full-bleed>
                <Table.Content
                  aria-label="抓包记录"
                  className="min-w-240"
                  selectedKeys={selectedId != null ? [String(selectedId)] : []}
                  selectionMode="single"
                  onSelectionChange={(keys) => {
                    const next = Array.from(keys as Set<string | number>)[0];
                    if (next != null) setSelectedId(Number(next));
                  }}
                >
                  <Table.Header>
                    <Table.Column isRowHeader>#</Table.Column>
                    <Table.Column>方法</Table.Column>
                    <Table.Column>状态</Table.Column>
                    <Table.Column>域名</Table.Column>
                    <Table.Column>路径</Table.Column>
                    <Table.Column className="text-end">大小</Table.Column>
                  </Table.Header>
                  <Table.Body
                    items={visible}
                    renderEmptyState={() => (
                      <EmptyState
                        title="没有匹配的请求"
                        hint="当前快捷过滤排除了全部抓包记录。切回「全部」即可看到完整会话。"
                        action={
                          <Button size="sm" variant="secondary" onPress={() => setCapFilter("all")}>
                            清除过滤
                          </Button>
                        }
                      />
                    )}
                  >
                    {(cap) => (
                      <Table.Row
                        id={String(cap.request_id)}
                        onDoubleClick={() => {
                          setSelectedId(cap.request_id);
                          setDrawerOpen(true);
                        }}
                      >
                        <Table.Cell>
                          <span className="font-mono text-base text-muted">{cap.request_id}</span>
                        </Table.Cell>
                        <Table.Cell>
                          <MethodChip method={cap.request.method} />
                        </Table.Cell>
                        <Table.Cell>
                          <span className={statusTone(cap.response?.status ?? 0)}>
                            {cap.error !== null ? "ERR" : (cap.response?.status ?? "—")}
                          </span>
                        </Table.Cell>
                        <Table.Cell>
                          <span className="truncate font-mono text-base text-muted">
                            {cap.request.authority}
                          </span>
                        </Table.Cell>
                        <Table.Cell>
                          <span className="truncate font-mono text-base text-foreground">
                            {cap.request.path}
                          </span>
                        </Table.Cell>
                        <Table.Cell className="text-end">
                          <span className="font-mono text-base text-muted">
                            {formatBytes(
                              (cap.request.body?.size ?? 0) + (cap.response?.body?.size ?? 0),
                            )}
                          </span>
                        </Table.Cell>
                      </Table.Row>
                    )}
                  </Table.Body>
                </Table.Content>
              </Table.ScrollContainer>
            </Table>
          )}
        </div>

        {/* 悬浮详情面板（非模态）：Esc 挂 window 捕获阶段；模态在场时让位。 */}
        <aside
          aria-label="抓包详情"
          aria-hidden={!drawerOpen}
          className={`wb-detail-panel ${drawerOpen ? "wb-detail-panel--open" : ""}`}
        >
          {drawerOpen && current ? (
            <CaptureDetail
              capture={current}
              env={env}
              onFeedback={onFeedback}
              onClose={() => setDrawerOpen(false)}
            />
          ) : null}
        </aside>
      </div>
    </div>
  );
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
    const curl = `curl -x http://${env.listen.host}:${env.listen.port} 'https://${capture.request.authority}${capture.request.path}'`;
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
    { k: "状态码", v: capture.error !== null ? "ERR" : String(status), tone: status >= 400 || capture.error !== null ? ("danger" as const) : ("default" as const) },
    { k: "请求大小", v: formatBytes(capture.request.body?.size ?? 0) },
    { k: "响应大小", v: formatBytes(capture.response?.body?.size ?? 0) },
  ];

  return (
    <>
      <div className="flex flex-none flex-col gap-2 border-b border-border px-4 py-3">
        <div className="flex w-full items-center gap-3">
          <MethodChip method={capture.request.method} />
          <span className="min-w-0 flex-1 truncate font-mono text-lg font-bold text-foreground">
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
            <Tooltip.Content>关闭（Esc）</Tooltip.Content>
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
            className="flex min-h-0 flex-1 flex-col"
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
                <Tabs.Tab id="req-body">
                  请求体
                  <Tabs.Indicator />
                </Tabs.Tab>
              </Tabs.List>
            </Tabs.ListContainer>

            <Tabs.Panel className="min-h-0 flex-1 overflow-y-auto pt-3" id="overview">
              <KvGrid>
                {overview.map((item) => (
                  <Kv key={item.k} k={item.k} v={item.v} tone={"tone" in item ? item.tone : "default"} />
                ))}
              </KvGrid>
            </Tabs.Panel>

            <Tabs.Panel className="min-h-0 flex-1 overflow-y-auto pt-3" id="req-headers">
              <HeaderRows rows={capture.request.headers} />
            </Tabs.Panel>

            <Tabs.Panel className="min-h-0 flex-1 overflow-y-auto pt-3" id="req-body">
              <BodyBlock body={capture.request.body} />
            </Tabs.Panel>
          </Tabs>
        </section>

        <Separator className="flex-none" />

        {/* 下半：响应信息 */}
        <section aria-label="响应信息" className="flex min-h-0 flex-1 flex-col">
          <Tabs
            align="start"
            className="flex min-h-0 flex-1 flex-col"
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

            <Tabs.Panel className="min-h-0 flex-1 overflow-y-auto pt-3" id="resp-headers">
              <HeaderRows rows={capture.response?.headers ?? []} />
            </Tabs.Panel>

            <Tabs.Panel className="min-h-0 flex-1 overflow-y-auto pt-3" id="resp-body">
              <BodyBlock body={capture.response?.body ?? null} />
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
          <span className="truncate text-muted">{k}</span>
          <span className="truncate text-foreground">{v}</span>
        </div>
      ))}
      <Hint>头部按原始顺序展示，未做归一化。</Hint>
    </div>
  );
}

/** 请求 / 响应体等宽文本块（utf8 直显；超限 omitted 给提示） */
function BodyBlock({ body }: { body: CaptureRecord["request"]["body"] }) {
  if (body === null || body === undefined) {
    return <Hint>该消息没有正文。</Hint>;
  }
  if ("omitted" in body) {
    return (
      <Hint>
        正文过大（{formatBytes(body.size)}），已省略存储。完整内容看实例日志。
      </Hint>
    );
  }
  if (body.encoding === "base64") {
    return (
      <Hint>
        二进制正文（{formatBytes(body.size)}），base64 预览：
        <span className="mt-1 block truncate font-mono">{body.content.slice(0, 120)}…</span>
      </Hint>
    );
  }
  return (
    <ScrollShadow className="wb-body-block">
      <pre className="whitespace-pre-wrap break-all font-mono text-sm leading-relaxed text-muted">
        {body.content}
      </pre>
    </ScrollShadow>
  );
}

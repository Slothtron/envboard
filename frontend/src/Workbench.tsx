import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Input, Label, TextField } from "@heroui/react";

import { api } from "./api/client";
import { openSnapshotStream } from "./api/stream";
import { ApiError, type EnvView, type StatusInfo } from "./api/types";
import { envNeedsAttention, type ViewKey } from "./data";
import { ConfirmDialog, FormDialog } from "./dialogs";
import { SideBar } from "./SideBar";
import { TopBar, type MetricKey } from "./TopBar";
import { FeedbackAlert } from "./shared";
import { ActivityView } from "./views/ActivityView";
import { CompareView, ProxiesView, RulesView } from "./views/CatalogViews";
import { DebugView } from "./views/DebugView";
import { EnvironmentsView, type LoadStatus } from "./views/EnvironmentsView";
import { SettingsView } from "./views/SettingsView";

interface Feedback {
  status: "success" | "warning" | "danger";
  title: string;
}

interface ConfirmState {
  title: string;
  description: string;
  confirmLabel: string;
  action: () => void;
}

/** 错误 → 反馈文案：原因 + 建议动作（评审档「异常三通道」）。 */
function errorFeedback(error: unknown): Feedback {
  if (error instanceof ApiError) {
    return { status: "danger", title: `${error.message}${error.field ? `（${error.field}）` : ""}` };
  }
  return { status: "danger", title: "请求失败，请重试" };
}

export function Workbench() {
  const [view, setView] = useState<ViewKey>("environments");
  const [metric, setMetric] = useState<MetricKey>("all");
  const [environments, setEnvironments] = useState<EnvView[]>([]);
  const [status, setStatus] = useState<LoadStatus>("loading");
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [streamConnected, setStreamConnected] = useState(false);
  const [statusInfo, setStatusInfo] = useState<StatusInfo | null>(null);

  const [newEnvOpen, setNewEnvOpen] = useState(false);
  const [newEnvName, setNewEnvName] = useState("");
  const [confirm, setConfirm] = useState<ConfirmState | null>(null);
  const [feedback, setFeedback] = useState<Feedback | null>(null);

  /* 快照内容比较：cursor 是 advisory，同代次两帧可能不同，必须比内容（spec/protocol.md） */
  const lastSnapshot = useRef<string>("");

  const pushFeedback = useCallback((nextStatus: Feedback["status"], title: string) => {
    setFeedback({ status: nextStatus, title });
    window.setTimeout(() => setFeedback(null), 3200);
  }, []);

  const applyEnvironments = useCallback((next: EnvView[]) => {
    const serialized = JSON.stringify(next);
    if (serialized !== lastSnapshot.current) {
      lastSnapshot.current = serialized;
      setEnvironments(next);
    }
  }, []);

  /* 初始加载 + 快照流 */
  useEffect(() => {
    let cancelled = false;
    api
      .envList()
      .then((list) => {
        if (!cancelled) {
          applyEnvironments(list);
          setStatus("ready");
        }
      })
      .catch(() => {
        if (!cancelled) setStatus("error");
      });
    void api.status().then((info) => {
      if (!cancelled) setStatusInfo(info);
    });

    const stream = openSnapshotStream(
      (frame) => {
        if (cancelled) return;
        if (frame.ok) {
          applyEnvironments(frame.environments);
          setStatus("ready");
        } else {
          setStatus("error");
        }
      },
      (connected) => {
        if (!cancelled) setStreamConnected(connected);
      },
    );
    return () => {
      cancelled = true;
      stream.close();
    };
  }, [applyEnvironments]);

  const refresh = useCallback(() => {
    setIsRefreshing(true);
    api
      .envList()
      .then((list) => {
        applyEnvironments(list);
        setStatus("ready");
        pushFeedback("success", "已刷新");
      })
      .catch((error: unknown) => {
        setStatus("error");
        const f = errorFeedback(error);
        pushFeedback(f.status, f.title);
      })
      .finally(() => setIsRefreshing(false));
  }, [applyEnvironments, pushFeedback]);

  const counts = useMemo<Record<MetricKey, number>>(() => {
    const running = environments.filter((e) => e.health === "running").length;
    const stopped = environments.filter((e) => e.health === "stopped").length;
    const issues = environments.filter(envNeedsAttention).length;
    return { all: environments.length, running, stopped, issues };
  }, [environments]);

  const navCounts = useMemo<Partial<Record<ViewKey, number>>>(
    () => ({ environments: environments.length }),
    [environments.length],
  );

  const toggleEnv = useCallback(
    (env: EnvView) => {
      const stopping = env.health === "running";
      (stopping ? api.envStop(env.name) : api.envStart(env.name))
        .then(() => pushFeedback(stopping ? "warning" : "success", `${stopping ? "已停止" : "已启动"}环境 ${env.name}`))
        .catch((error: unknown) => {
          const f = errorFeedback(error);
          pushFeedback(f.status, f.title);
        });
    },
    [pushFeedback],
  );

  const restartEnv = useCallback(
    (env: EnvView) => {
      api
        .envRestart(env.name)
        .then(() => pushFeedback("success", `已重启环境 ${env.name}`))
        .catch((error: unknown) => {
          const f = errorFeedback(error);
          pushFeedback(f.status, f.title);
        });
    },
    [pushFeedback],
  );

  const requestDelete = useCallback((env: EnvView) => {
    setConfirm({
      title: "确认操作",
      description: `将删除环境 ${env.name}（${env.listen.host}:${env.listen.port}）及其运行实例，端口会被释放。`,
      confirmLabel: "删除",
      action: () => {
        api
          .envDelete(env.name)
          .then(() => {
            setSelected((cur) => (cur === env.name ? null : cur));
            pushFeedback("danger", `已删除环境 ${env.name}`);
          })
          .catch((error: unknown) => {
            const f = errorFeedback(error);
            pushFeedback(f.status, f.title);
          });
      },
    });
  }, [pushFeedback]);

  const saveConfig = useCallback(
    (name: string, patch: Parameters<typeof api.envPatch>[1]) => {
      api
        .envPatch(name, patch)
        .then(() => pushFeedback("success", `环境 ${name} 配置已保存`))
        .catch((error: unknown) => {
          const f = errorFeedback(error);
          pushFeedback(f.status, f.title);
        });
    },
    [pushFeedback],
  );

  const createEnv = useCallback(() => {
    const name = newEnvName.trim();
    if (!name) return;
    api
      .envCreate({ name })
      .then((created) => {
        setNewEnvName("");
        setSelected(created.name);
        pushFeedback("success", `已创建环境 ${created.name}（端口 ${created.listen.port}）`);
      })
      .catch((error: unknown) => {
        const f = errorFeedback(error);
        pushFeedback(f.status, f.title);
      });
  }, [newEnvName, pushFeedback]);

  const nameInvalid = newEnvName.length > 0 && !/^[a-z][a-z0-9_-]*$/.test(newEnvName);

  return (
    <div className="flex h-screen flex-col bg-background text-foreground">
      <TopBar
        counts={counts}
        coreVersion={statusInfo?.core.version ?? "…"}
        isRefreshing={isRefreshing}
        metric={metric}
        streamConnected={streamConnected}
        onMetricChange={setMetric}
        onRefresh={refresh}
      />

      <div className="flex min-h-0 flex-1">
        <div className="flex w-56 flex-none flex-col">
          <SideBar
            navCounts={navCounts}
            streamConnected={streamConnected}
            view={view}
            onViewChange={setView}
          />
        </div>

        <main aria-label="工作台主区" className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
          {view === "environments" ? (
            <EnvironmentsView
              environments={environments}
              isRefreshing={isRefreshing}
              metricFilter={metric}
              selected={selected}
              status={status}
              onCopied={() => pushFeedback("success", "代理命令已复制")}
              onDelete={requestDelete}
              onMetricFilterChange={setMetric}
              onNew={() => setNewEnvOpen(true)}
              onRestart={restartEnv}
              onRetry={refresh}
              onSaveConfig={saveConfig}
              onSelect={setSelected}
              onToggle={toggleEnv}
            />
          ) : null}

          {view === "rules" ? <RulesView environments={environments} onFeedback={pushFeedback} /> : null}

          {view === "proxies" ? <ProxiesView onFeedback={pushFeedback} /> : null}

          {view === "debug" ? (
            <DebugView environments={environments} onFeedback={pushFeedback} />
          ) : null}

          {view === "compare" ? <CompareView /> : null}

          {view === "activity" ? <ActivityView /> : null}

          {view === "settings" ? (
            <SettingsView coreVersion={statusInfo?.core.version ?? ""} onFeedback={pushFeedback} />
          ) : null}
        </main>
      </div>

      {/* 反馈区：Toast 不在冻结清单内，改用页内固定 Alert（设计规范 D-6） */}
      {feedback ? (
        <div
          aria-live="polite"
          className="pointer-events-auto fixed bottom-5 right-5 z-50 flex flex-col gap-2"
          role="status"
        >
          <FeedbackAlert
            status={feedback.status}
            title={feedback.title}
            onClose={() => setFeedback(null)}
          />
        </div>
      ) : null}

      <FormDialog
        confirmLabel="创建"
        isOpen={newEnvOpen}
        note="创建后环境处于已停止状态，需要在列表中启动。端口从 16000–16999 自动分配。"
        title="新建环境"
        onConfirm={createEnv}
        onOpenChange={setNewEnvOpen}
      >
        <TextField className="w-full" isInvalid={nameInvalid} value={newEnvName} onChange={setNewEnvName}>
          <Label>环境名称</Label>
          <Input placeholder="小写字母开头，限 a-z 0-9 _ -" spellCheck={false} />
          <span className={`text-sm ${nameInvalid ? "text-danger" : "text-muted"}`}>
            {nameInvalid
              ? "名称不合法：小写字母开头，限 a-z 0-9 _ -。"
              : "一个环境 = 一个实例 = 一个端口。"}
          </span>
        </TextField>
      </FormDialog>

      <ConfirmDialog
        confirmLabel={confirm?.confirmLabel ?? "确认"}
        description={confirm?.description ?? ""}
        isOpen={confirm !== null}
        title={confirm?.title ?? "确认操作"}
        onConfirm={() => confirm?.action()}
        onOpenChange={(open) => {
          if (!open) setConfirm(null);
        }}
      />
    </div>
  );
}

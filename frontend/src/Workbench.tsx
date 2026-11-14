import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Description, FieldError, Input, Label, TextField } from "@heroui/react";

import { api } from "./api/client";
import { openSnapshotStream } from "./api/stream";
import type { EnvView, StatusInfo } from "./api/types";
import { envNeedsAttention, type ViewKey } from "./data";
import { formatErrorNotice } from "./errors";
import { ConfirmDialog, FormDialog } from "./dialogs";
import { Boot } from "./Boot";
import { StatusBar } from "./StatusBar";
import { TopBar, type MetricKey } from "./TopBar";
import { FeedbackAlert } from "./shared";
import { SettingsDialog } from "./SettingsDialog";
import { CertView } from "./views/CertView";
import { EnvironmentsView, type LoadStatus } from "./views/EnvironmentsView";

declare global {
  interface Window {
    /**
     * 壳注入（ADR-012）：本 WebView 是托盘重建出来的，不是进程的首次窗口。
     * 首帧之前就已知，故不必等 IPC；用于跳过 boot 自检屏。
     */
    __envboardRecreated?: boolean;
  }
}

interface Feedback {
  status: "success" | "warning" | "danger";
  title: string;
  /** 错误原文层（[code] message · field）；成功/警告类反馈没有这一层。 */
  detail?: string;
}

interface ConfirmState {
  title: string;
  description: string;
  confirmLabel: string;
  action: () => void;
}

/** 错误 → 三层反馈（envboard-engine::api::error 模块文档）：中文主句 + [code] 原文，唯一定义在 errors.ts。 */
function errorFeedback(error: unknown, action?: string): Feedback {
  return { status: "danger", ...formatErrorNotice(error, action) };
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
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [confirm, setConfirm] = useState<ConfirmState | null>(null);
  /* 反馈分槽：写错误（带原文层）独占一槽、不自动撤、不被成功提示顶掉；
     成功/警告走另一槽，3.2s 自动撤。连续操作时错误始终可读。 */
  const [errorFeedbackState, setErrorFeedbackState] = useState<Feedback | null>(null);
  const [notice, setNotice] = useState<Feedback | null>(null);
  const noticeTimer = useRef<number | null>(null);
  // boot 自检屏只属于「首次打开应用」：托盘重建的窗口直接进工作台（ADR-012）。
  const [booted, setBooted] = useState(() => window.__envboardRecreated === true);
  const finishBoot = useCallback(() => setBooted(true), []);

  /* 快照内容比较：cursor 是 advisory，同代次两帧可能不同，必须比内容（envboard-protocol crate 文档） */
  const lastSnapshot = useRef<string>("");

  const pushFeedback = useCallback(
    (nextStatus: Feedback["status"], title: string, detail?: string) => {
      if (detail) {
        // 写操作失败（带原文层）独占错误槽、不自动撤除：随后那次取数成功只说明
        // "读得到"，不能证明刚才那次写成功（envboard-engine::api::error 模块文档
        // 「错误的生命周期」）。成功提示不得顶掉它。
        setErrorFeedbackState({ status: nextStatus, title, detail });
        return;
      }
      setNotice({ status: nextStatus, title });
      if (noticeTimer.current != null) window.clearTimeout(noticeTimer.current);
      noticeTimer.current = window.setTimeout(() => setNotice(null), 3200);
    },
    [],
  );

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
        void api
          .status()
          .then((info) => setStatusInfo(info))
          .catch(() => undefined);
      })
      .catch((error: unknown) => {
        setStatus("error");
        const f = errorFeedback(error, "刷新环境列表");
        pushFeedback(f.status, f.title, f.detail);
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
          const f = errorFeedback(error, `${stopping ? "停止" : "启动"} ${env.name}`);
          pushFeedback(f.status, f.title, f.detail);
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
          const f = errorFeedback(error, `重启 ${env.name}`);
          pushFeedback(f.status, f.title, f.detail);
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
            const f = errorFeedback(error, `删除 ${env.name}`);
            pushFeedback(f.status, f.title, f.detail);
          });
      },
    });
  }, [pushFeedback]);

  /* 返回 Promise 并把失败继续抛给 ConfigTab：表单依结果决定保留还是回退，
     失败时不能谎报「已保存/已放弃」。 */
  const saveConfig = useCallback(
    (name: string, patch: Parameters<typeof api.envPatch>[1]) => {
      return api
        .envPatch(name, patch)
        .then(() => {
          pushFeedback("success", `环境 ${name} 配置已保存`);
        })
        .catch((error: unknown) => {
          const f = errorFeedback(error, `保存 ${name} 配置`);
          pushFeedback(f.status, f.title, f.detail);
          throw error;
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
        const f = errorFeedback(error, `创建 ${name}`);
        pushFeedback(f.status, f.title, f.detail);
      });
  }, [newEnvName, pushFeedback]);

  const nameInvalid = newEnvName.length > 0 && !/^[a-z][a-z0-9_-]*$/.test(newEnvName);

  return (
    <div className="relative z-10 flex h-screen flex-col bg-background text-foreground">
      <TopBar
        counts={counts}
        coreVersion={statusInfo?.core.version ?? "…"}
        isRefreshing={isRefreshing}
        streamConnected={streamConnected}
        navCounts={navCounts}
        view={view}
        onViewChange={setView}
        onOpenSettings={() => setSettingsOpen(true)}
        onRefresh={refresh}
      />

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
              onFeedback={pushFeedback}
            />
          ) : null}

          {view === "cert" ? <CertView onFeedback={pushFeedback} /> : null}
      </main>

      <StatusBar
        environments={environments}
        statusInfo={statusInfo}
        streamConnected={streamConnected}
      />

      {/* 反馈区：方向C 移至外壳头下方右上，容器点击穿透。
          错误槽在上（不自动撤、不被成功顶掉），轻提示槽在下（3.2s 自动撤）。 */}
      {errorFeedbackState || notice ? (
        <div aria-live="polite" className="ph-feedback" role="status">
          {errorFeedbackState ? (
            <FeedbackAlert
              status={errorFeedbackState.status}
              title={errorFeedbackState.title}
              detail={errorFeedbackState.detail}
              onClose={() => setErrorFeedbackState(null)}
            />
          ) : null}
          {notice ? (
            <FeedbackAlert
              status={notice.status}
              title={notice.title}
              onClose={() => setNotice(null)}
            />
          ) : null}
        </div>
      ) : null}

      <FormDialog
        confirmLabel="创建"
        confirmDisabled={!newEnvName.trim() || nameInvalid}
        isOpen={newEnvOpen}
        note="创建后环境处于已停止状态，需要在列表中启动。端口从 16000–16999 自动分配。"
        title="新建环境"
        onConfirm={createEnv}
        onOpenChange={setNewEnvOpen}
      >
        <TextField className="w-full" isInvalid={nameInvalid} value={newEnvName} onChange={setNewEnvName}>
          <Label>环境名称</Label>
          <Input autoFocus placeholder="小写字母开头，限 a-z 0-9 _ -" spellCheck={false} />
          {nameInvalid ? (
            <FieldError>名称不合法：小写字母开头，限 a-z 0-9 _ -。</FieldError>
          ) : (
            <Description>一个环境 = 一个实例 = 一个端口。</Description>
          )}
        </TextField>
      </FormDialog>

      <SettingsDialog
        coreVersion={statusInfo?.core.version ?? ""}
        isOpen={settingsOpen}
        status={statusInfo}
        onFeedback={pushFeedback}
        onOpenChange={setSettingsOpen}
      />

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

      {!booted ? <Boot onDone={finishBoot} /> : null}
    </div>
  );
}

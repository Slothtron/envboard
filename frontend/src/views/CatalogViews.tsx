import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Button,
  Chip,
  Input,
  Label,
  SearchField,
  Separator,
  Surface,
  Table,
  TextArea,
  TextField,
  Tooltip,
} from "@heroui/react";

import { api } from "../api/client";
import { ApiError, type EnvView, type ProxyView } from "../api/types";
import {
  IconEdit,
  IconPlus,
  IconRules,
  IconServer,
  IconTrash,
} from "../icons";
import { ConfirmDialog, FormDialog } from "../dialogs";
import { EmptyState, ErrorState, Hint, SectionTitle, TableSkeleton } from "../shared";

type FeedbackFn = (status: "success" | "warning" | "danger", title: string) => void;

function errorTitle(error: unknown): string {
  return error instanceof ApiError ? error.message : "请求失败，请重试";
}

/** 拉取式资源加载（loading / error+retry / ready 四态，D-6）。 */
function useResource<T>(load: () => Promise<T>) {
  const [data, setData] = useState<T | null>(null);
  const [status, setStatus] = useState<"loading" | "error" | "ready">("loading");
  const [isRetrying, setIsRetrying] = useState(false);

  const reload = useCallback(() => {
    setIsRetrying(true);
    load()
      .then((value) => {
        setData(value);
        setStatus("ready");
      })
      .catch(() => setStatus("error"))
      .finally(() => setIsRetrying(false));
  }, [load]);

  useEffect(() => {
    reload();
  }, [reload]);

  return { data, status, isRetrying, reload };
}

/* ------------------------------------------------------------------ */
/* 规则库（P12：列表直接给出「被哪些环境引用」）                          */
/* ------------------------------------------------------------------ */

export function RulesView({
  environments,
  onFeedback,
}: {
  environments: EnvView[];
  onFeedback: FeedbackFn;
}) {
  const [search, setSearch] = useState("");
  const [importOpen, setImportOpen] = useState(false);
  const [importName, setImportName] = useState("");
  const [importText, setImportText] = useState("");
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  const load = useCallback(() => api.rulesList().then((r) => r.rules), []);
  const { data: names, status, isRetrying, reload } = useResource(load);

  const ruleSets = useMemo(() => {
    const list = (names ?? []).map((name) => ({
      name,
      used: environments.filter((e) => e.rules === name).map((e) => e.name),
    }));
    const q = search.trim().toLowerCase();
    return q ? list.filter((r) => r.name.toLowerCase().includes(q)) : list;
  }, [names, environments, search]);

  const nameInvalid = importName.length > 0 && !/^[a-z][a-z0-9_-]*$/.test(importName);

  function submitImport() {
    const name = importName.trim();
    if (!name || nameInvalid) return;
    void api
      .rulesImport({ name, text: importText })
      .then(() => {
        setImportName("");
        setImportText("");
        onFeedback("success", `规则集 ${name} 已导入`);
        reload();
      })
      .catch((error: unknown) => onFeedback("danger", errorTitle(error)));
  }

  function submitDelete() {
    if (!confirmDelete) return;
    const name = confirmDelete;
    void api
      .rulesDelete(name)
      .then(() => {
        onFeedback("danger", `已删除规则集 ${name}`);
        reload();
      })
      .catch((error: unknown) => onFeedback("danger", errorTitle(error)));
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex flex-wrap items-center gap-2 border-b border-border bg-surface px-5 py-3">
        <SearchField aria-label="搜索规则集" className="flex-none" value={search} onChange={setSearch}>
          <SearchField.Group>
            <SearchField.SearchIcon />
            <SearchField.Input className="w-40 xl:w-56" placeholder="搜索规则集" />
            <SearchField.ClearButton />
          </SearchField.Group>
        </SearchField>

        <div className="ml-auto">
          {/* 该视图唯一的主操作 */}
          <Button size="sm" variant="primary" onPress={() => setImportOpen(true)}>
            <IconPlus className="size-4" />
            导入规则集
          </Button>
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto p-5">
        <Surface className="flex flex-col p-0">
          <div className="flex items-center gap-2 border-b border-border px-4 py-3">
            <IconRules className="size-4 text-muted" />
            <h2 className="text-base font-semibold text-foreground">规则集</h2>
            <Chip size="sm" variant="soft">
              <Chip.Label>{names?.length ?? 0}</Chip.Label>
            </Chip>
          </div>

          {status === "loading" ? <TableSkeleton rows={3} /> : null}
          {status === "error" ? (
            <ErrorState
              description="无法读取规则库账本。确认服务已启动后重试。"
              isRetrying={isRetrying}
              title="规则集加载失败"
              onRetry={reload}
            />
          ) : null}
          {status === "ready" && ruleSets.length === 0 ? (
            <EmptyState
              hint={
                search
                  ? "当前搜索词没有命中任何规则集，换一个关键词试试。"
                  : "还没有导入任何规则集。hosts 规则决定哪些域名被改写到哪个 IP。"
              }
              title={search ? "没有匹配的规则集" : "还没有规则集"}
              action={
                search ? (
                  <Button size="sm" variant="secondary" onPress={() => setSearch("")}>
                    清除搜索
                  </Button>
                ) : (
                  <Button size="sm" variant="secondary" onPress={() => setImportOpen(true)}>
                    <IconPlus className="size-4" />
                    导入规则集
                  </Button>
                )
              }
            />
          ) : null}

          {status === "ready"
            ? ruleSets.map((rule, index) => (
                <div key={rule.name} className="flex flex-col">
                  {index > 0 ? <Separator /> : null}
                  <div className="flex flex-wrap items-center gap-3 px-4 py-3" data-rule-name={rule.name}>
                    {/* 名称列固定宽：让各行「被引用」起点横向对齐 */}
                    <span className="flex w-56 flex-none flex-col gap-1">
                      <span className="truncate font-mono text-base font-semibold text-foreground">
                        {rule.name}
                      </span>
                    </span>

                    {/* 反向引用：改规则前可见影响面 */}
                    <span className="flex flex-wrap items-center gap-2">
                      <span className="text-sm text-muted">被引用：</span>
                      {rule.used.length ? (
                        rule.used.map((env) => (
                          <Chip key={env} size="sm" variant="soft">
                            <Chip.Label>{env}</Chip.Label>
                          </Chip>
                        ))
                      ) : (
                        <Chip size="sm" variant="soft">
                          <Chip.Label>未被引用</Chip.Label>
                        </Chip>
                      )}
                    </span>

                    <span className="ml-auto flex items-center gap-2">
                      <a href={`/api/rules/${encodeURIComponent(rule.name)}`} rel="noreferrer" target="_blank">
                        <Button size="sm" variant="secondary">
                          <IconEdit className="size-4" />
                          查看原文
                        </Button>
                      </a>
                      <Tooltip delay={200}>
                        <Tooltip.Trigger>
                          <Button
                            aria-label={`删除规则集 ${rule.name}`}
                            className="btn-action--danger"
                            size="sm"
                            variant="outline"
                            onPress={() => setConfirmDelete(rule.name)}
                          >
                            <IconTrash className="size-4" />
                            删除
                          </Button>
                        </Tooltip.Trigger>
                        <Tooltip.Content>删除规则集（二次确认；绑定它的环境会标记规则缺失）</Tooltip.Content>
                      </Tooltip>
                    </span>
                  </div>
                </div>
              ))
            : null}
        </Surface>

        <div className="p-3">
          <Hint>
            规则账本的唯一真相在服务端（<b>state.json rules[]</b>）；物化文件可再生。
            被引用关系实时来自环境绑定。
          </Hint>
        </div>
      </div>

      <FormDialog
        confirmLabel="导入"
        isOpen={importOpen}
        note="导入即覆盖同名规则集；正文为 hosts 风格（IP + 完整域名，每行一条）。"
        title="导入规则集"
        onConfirm={submitImport}
        onOpenChange={setImportOpen}
      >
        <TextField
          className="w-full"
          isInvalid={nameInvalid}
          value={importName}
          onChange={setImportName}
        >
          <Label>规则集名称</Label>
          <Input placeholder="小写字母开头，限 a-z 0-9 _ -" spellCheck={false} />
          {nameInvalid ? (
            <span className="text-sm text-danger">名称不合法：小写字母开头，限 a-z 0-9 _ -。</span>
          ) : (
            <span className="text-sm text-muted">同名规则集将被覆盖。</span>
          )}
        </TextField>
        <TextField className="w-full" value={importText} onChange={setImportText}>
          <Label>规则正文</Label>
          <TextArea rows={6} spellCheck={false} />
        </TextField>
      </FormDialog>

      <ConfirmDialog
        confirmLabel="删除"
        description={`将删除规则集 ${confirmDelete ?? ""}。绑定它的环境会标记「规则缺失」，不再覆盖任何域名。`}
        isOpen={confirmDelete !== null}
        title="确认删除规则集"
        onConfirm={submitDelete}
        onOpenChange={(open) => {
          if (!open) setConfirmDelete(null);
        }}
      />
    </div>
  );
}

/* ------------------------------------------------------------------ */
/* 上游代理                                                            */
/* ------------------------------------------------------------------ */

export function ProxiesView({ onFeedback }: { onFeedback: FeedbackFn }) {
  const [editTarget, setEditTarget] = useState<ProxyView | "new" | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<ProxyView | null>(null);

  const load = useCallback(() => api.proxiesList(), []);
  const { data: proxies, status, isRetrying, reload } = useResource(load);
  const rows = proxies ?? [];

  function submitDelete() {
    if (!confirmDelete) return;
    const name = confirmDelete.name;
    void api
      .proxiesDelete(name)
      .then(() => {
        onFeedback("danger", `已删除上游代理 ${name}`);
        reload();
      })
      .catch((error: unknown) => onFeedback("danger", errorTitle(error)));
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex flex-wrap items-center gap-2 border-b border-border bg-surface px-5 py-3">
        <div className="ml-auto">
          {/* 该视图唯一的主操作 */}
          <Button size="sm" variant="primary" onPress={() => setEditTarget("new")}>
            <IconPlus className="size-4" />
            新建上游代理
          </Button>
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto p-5">
        <Surface className="flex flex-col p-0">
          <div className="flex items-center gap-2 border-b border-border px-4 py-3">
            <IconServer className="size-4 text-muted" />
            <h2 className="text-base font-semibold text-foreground">上游代理</h2>
            <Chip size="sm" variant="soft">
              <Chip.Label>{rows.length}</Chip.Label>
            </Chip>
          </div>

          {status === "loading" ? <TableSkeleton rows={2} /> : null}
          {status === "error" ? (
            <ErrorState
              description="无法读取上游代理账本。确认服务已启动后重试。"
              isRetrying={isRetrying}
              title="上游代理加载失败"
              onRetry={reload}
            />
          ) : null}
          {status === "ready" && rows.length === 0 ? (
            <EmptyState
              hint="新建上游代理后，环境可以绑定它，出方向流量先经代理再到目标。"
              title="还没有上游代理"
              action={
                <Button size="sm" variant="secondary" onPress={() => setEditTarget("new")}>
                  <IconPlus className="size-4" />
                  新建上游代理
                </Button>
              }
            />
          ) : null}

          {status === "ready"
            ? rows.map((up, index) => (
                <div key={up.name} className="flex flex-col">
                  {index > 0 ? <Separator /> : null}
                  <div className="flex flex-wrap items-center gap-3 px-4 py-3" data-proxy-name={up.name}>
                    {/* 名称列固定宽：与规则库行同构 */}
                    <span className="flex w-56 flex-none flex-col gap-1">
                      <span className="truncate font-mono text-base font-semibold text-foreground">
                        {up.name}
                      </span>
                      <span className="truncate font-mono text-sm text-muted">
                        {up.host}:{up.port} · {up.has_auth ? "鉴权已配置" : "无鉴权"}
                      </span>
                    </span>

                    <span className="flex flex-wrap items-center gap-2">
                      <span className="text-sm text-muted">被引用：</span>
                      {up.references.length ? (
                        up.references.map((env) => (
                          <Chip key={env} size="sm" variant="soft">
                            <Chip.Label>{env}</Chip.Label>
                          </Chip>
                        ))
                      ) : (
                        <Chip size="sm" variant="soft">
                          <Chip.Label>未被引用</Chip.Label>
                        </Chip>
                      )}
                    </span>

                    <span className="ml-auto flex items-center gap-2">
                      <Button size="sm" variant="secondary" onPress={() => setEditTarget(up)}>
                        <IconEdit className="size-4" />
                        编辑
                      </Button>
                      {/* 禁用原因由行内「被引用」chips 随行可见（P12），解释走 aria-label（D-6） */}
                      {up.references.length > 0 ? (
                        <Button
                          aria-label={`删除上游代理 ${up.name}：仍被 ${up.references.join("、")} 引用，先解除绑定才能删除`}
                          className="btn-action--danger"
                          size="sm"
                          variant="outline"
                          isDisabled
                        >
                          <IconTrash className="size-4" />
                          删除
                        </Button>
                      ) : (
                        <Tooltip delay={200}>
                          <Tooltip.Trigger>
                            <Button
                              className="btn-action--danger"
                              size="sm"
                              variant="outline"
                              onPress={() => setConfirmDelete(up)}
                            >
                              <IconTrash className="size-4" />
                              删除
                            </Button>
                          </Tooltip.Trigger>
                          <Tooltip.Content>删除上游代理（二次确认）</Tooltip.Content>
                        </Tooltip>
                      )}
                    </span>
                  </div>
                </div>
              ))
            : null}
        </Surface>
      </div>

      {editTarget ? (
        <ProxyFormDialog
          proxy={editTarget === "new" ? null : editTarget}
          onClose={() => setEditTarget(null)}
          onSaved={(name) => {
            setEditTarget(null);
            onFeedback("success", `上游代理 ${name} 已保存`);
            reload();
          }}
          onFeedback={onFeedback}
        />
      ) : null}

      <ConfirmDialog
        confirmLabel="删除"
        description={`将删除上游代理 ${confirmDelete?.name ?? ""}。`}
        isOpen={confirmDelete !== null}
        title="确认删除上游代理"
        onConfirm={submitDelete}
        onOpenChange={(open) => {
          if (!open) setConfirmDelete(null);
        }}
      />
    </div>
  );
}

function ProxyFormDialog({
  proxy,
  onClose,
  onSaved,
  onFeedback,
}: {
  proxy: ProxyView | null;
  onClose: () => void;
  onSaved: (name: string) => void;
  onFeedback: FeedbackFn;
}) {
  const [name, setName] = useState(proxy?.name ?? "");
  const [host, setHost] = useState(proxy?.host ?? "");
  const [port, setPort] = useState(String(proxy?.port ?? ""));
  const [user, setUser] = useState("");
  const [password, setPassword] = useState("");

  const nameInvalid = name.length > 0 && !/^[a-z][a-z0-9_-]*$/.test(name);
  const hostInvalid = host.length > 0 && !/^[A-Za-z0-9._-]+$/.test(host);
  const portInvalid = port.length > 0 && !/^\d{1,5}$/.test(port);
  const authHalfFilled = (user.length > 0) !== (password.length > 0);
  const authHasColon = /[:\s]/.test(user) || /[:\s]/.test(password);
  const authInvalid = authHalfFilled || authHasColon;
  const canSave = !nameInvalid && !hostInvalid && !portInvalid && !authInvalid && name && host && port;

  function submit() {
    if (!canSave) return;
    void api
      .proxiesPut({
        name: name.trim(),
        host: host.trim(),
        port: Number(port),
        ...(user && password ? { user, password } : {}),
      })
      .then(() => onSaved(name.trim()))
      .catch((error: unknown) => onFeedback("danger", errorTitle(error)));
  }

  return (
    <FormDialog
      confirmLabel="保存"
      isOpen
      note={proxy ? "改 host/port/凭据对引用它的环境热应用。凭据只进不出，永不回显。" : "新建后需到环境配置里绑定才会生效。"}
      title={proxy ? `编辑上游代理 ${proxy.name}` : "新建上游代理"}
      onConfirm={submit}
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <TextField className="w-full" isInvalid={nameInvalid} value={name} onChange={setName}>
        <Label>名称</Label>
        <Input spellCheck={false} />
        <span className={`text-sm ${nameInvalid ? "text-danger" : "text-muted"}`}>
          {nameInvalid ? "小写字母开头，限 a-z 0-9 _ -。" : "环境按名绑定。"}
        </span>
      </TextField>
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <TextField className="w-full" isInvalid={hostInvalid} value={host} onChange={setHost}>
          <Label>地址</Label>
          <Input placeholder="proxy.corp.example.com" spellCheck={false} />
          <span className={`text-sm ${hostInvalid ? "text-danger" : "text-muted"}`}>
            {hostInvalid ? "只允许域名或 IP 字面量。" : "域名或 IP。"}
          </span>
        </TextField>
        <TextField className="w-full" isInvalid={portInvalid} value={port} onChange={setPort}>
          <Label>端口</Label>
          <Input inputMode="numeric" placeholder="3128" spellCheck={false} />
          {portInvalid ? <span className="text-sm text-danger">端口必须是数字。</span> : null}
        </TextField>
      </div>
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <TextField className="w-full" isInvalid={authInvalid} value={user} onChange={setUser}>
          <Label>用户名（可选）</Label>
          <Input spellCheck={false} />
        </TextField>
        <TextField className="w-full" isInvalid={authInvalid} value={password} onChange={setPassword}>
          <Label>密码（可选）</Label>
          <Input type="password" />
        </TextField>
      </div>
      <span className={`text-sm leading-relaxed ${authInvalid ? "text-danger" : "text-muted"}`}>
        {authInvalid
          ? "用户名与密码必须同时填写，且都不能含「:」或空白字符。"
          : "留空表示不配置鉴权。"}
      </span>
    </FormDialog>
  );
}

/* ------------------------------------------------------------------ */
/* 跨环境对比（GET /api/compare?host= —— 确定性查账，不发真实请求）       */
/* ------------------------------------------------------------------ */

export function CompareView() {
  const [host, setHost] = useState("");
  const [result, setResult] = useState<Awaited<ReturnType<typeof api.compare>> | null>(null);
  const [status, setStatus] = useState<"idle" | "loading" | "error" | "ready">("idle");

  function run() {
    const wanted = host.trim();
    if (!wanted) return;
    setStatus("loading");
    void api
      .compare(wanted)
      .then((r) => {
        setResult(r);
        setStatus("ready");
      })
      .catch(() => setStatus("error"));
  }

  return (
    <div className="min-h-0 flex-1 overflow-y-auto p-5">
      <Surface className="flex flex-col p-0">
        <div className="flex items-center gap-2 border-b border-border px-4 py-3">
          <h2 className="text-base font-semibold text-foreground">跨环境对比</h2>
        </div>

        <div className="flex flex-col gap-4 p-4">
          <SectionTitle>对比域名</SectionTitle>
          <div className="flex flex-wrap items-center gap-2">
            <SearchField aria-label="待对比域名" className="flex-none" value={host} onChange={setHost}>
              <SearchField.Group>
                <SearchField.SearchIcon />
                <SearchField.Input className="w-72" placeholder="api.example.com" />
                <SearchField.ClearButton />
              </SearchField.Group>
            </SearchField>

            {/* 该视图唯一的主操作 */}
            <Button isDisabled={!host.trim()} size="sm" variant="primary" onPress={run}>
              对比
            </Button>
          </div>

          {status === "loading" ? <TableSkeleton rows={3} /> : null}
          {status === "error" ? (
            <ErrorState
              description="对比查询失败。确认域名格式后重试。"
              title="对比失败"
              onRetry={run}
            />
          ) : null}
          {status === "ready" && result ? (
            <Table>
              <Table.ScrollContainer>
                <Table.Content aria-label="跨环境对比结果">
                  <Table.Header>
                    <Table.Column isRowHeader>环境</Table.Column>
                    <Table.Column>端口</Table.Column>
                    <Table.Column>规则绑定</Table.Column>
                    <Table.Column>解析结果</Table.Column>
                    <Table.Column>覆盖该域名</Table.Column>
                  </Table.Header>
                  <Table.Body
                    items={result.environments}
                    renderEmptyState={() => (
                      <EmptyState hint="还没有任何环境。" title="没有可对比的环境" />
                    )}
                  >
                    {(row) => (
                      <Table.Row id={row.env}>
                        <Table.Cell>
                          <span className="font-mono text-base text-foreground">{row.env}</span>
                        </Table.Cell>
                        <Table.Cell>
                          <span className="font-mono text-base text-muted">{row.port}</span>
                        </Table.Cell>
                        <Table.Cell>
                          <span className="font-mono text-base text-foreground">
                            {row.rules ?? "（不覆盖）"}
                          </span>
                        </Table.Cell>
                        <Table.Cell>
                          <span
                            className={`font-mono text-base ${row.ip ? "text-foreground" : "text-muted"}`}
                          >
                            {row.ip ?? "—"}
                          </span>
                        </Table.Cell>
                        <Table.Cell>
                          <Chip color={row.covered ? "success" : "default"} size="sm" variant="soft">
                            <Chip.Label>{row.covered ? "是" : "否"}</Chip.Label>
                          </Chip>
                        </Table.Cell>
                      </Table.Row>
                    )}
                  </Table.Body>
                </Table.Content>
              </Table.ScrollContainer>
            </Table>
          ) : null}
          {status === "idle" ? (
            <Hint>输入域名后点「对比」：同一域名在所有环境里各自解析到哪里、由哪条规则决定。</Hint>
          ) : null}
        </div>
      </Surface>
    </div>
  );
}

import { useEffect, useMemo, useState } from "react";
import {
  Alert,
  Button,
  Checkbox,
  Chip,
  Disclosure,
  Input,
  Label,
  ListBox,
  ScrollShadow,
  Select,
  Separator,
  Surface,
  Tabs,
  TextArea,
  TextField,
  Tooltip,
} from "@heroui/react";

import { api } from "../api/client";
import type { EnvPatchReq, EnvView } from "../api/types";
import { envNeedsAttention, healthLabel, healthTone, isPublicListen, listenAddress } from "../data";
import {
  IconAlert,
  IconCheck,
  IconClose,
  IconCopy,
  IconLink,
  IconPlay,
  IconRestart,
  IconShield,
  IconStop,
  IconTrash,
} from "../icons";
import { Hint, InlineNotice, Kv, KvGrid, SectionTitle } from "../shared";

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

export interface EnvironmentDetailProps {
  env: EnvView;
  onToggle: (env: EnvView) => void;
  onRestart: (env: EnvView) => void;
  onDelete: (env: EnvView) => void;
  onSave: (name: string, patch: EnvPatchReq) => void;
  onCopied: () => void;
  /** 抽屉模式下传入：头部出现关闭按钮 */
  onClose?: () => void;
}

export function EnvironmentDetail({
  env,
  onToggle,
  onRestart,
  onDelete,
  onSave,
  onCopied,
  onClose,
}: EnvironmentDetailProps) {
  const isRunning = env.health === "running";
  const hasIssue = envNeedsAttention(env);
  const tone = healthTone(env.health);

  return (
    <>
      <div className="flex flex-none flex-wrap items-center gap-x-3 gap-y-2 border-b border-border px-4 py-3">
        <div className="flex min-w-0 items-baseline gap-3">
          <span className="truncate font-mono text-2xl font-bold text-foreground">{env.name}</span>
          <Chip color={TONE_CHIP[tone]} size="sm" variant="soft">
            <span aria-hidden="true" className={`size-2 rounded-full ${TONE_DOT[tone]}`} />
            <Chip.Label>{healthLabel(env.health)}</Chip.Label>
          </Chip>
          {/* 窄抽屉放不下时隐藏监听地址（概览分组里仍完整展示） */}
          <span className="hidden truncate font-mono text-sm text-muted 2xl:inline">
            {listenAddress(env)}
          </span>
        </div>

        <div className="ml-auto flex flex-none items-center gap-2">
          {/* 警示档：中断但可逆 */}
          <Tooltip delay={200}>
            <Tooltip.Trigger>
              <Button
                className={isRunning ? "btn-action--warn" : ""}
                size="sm"
                variant={isRunning ? "outline" : "secondary"}
                onPress={() => onToggle(env)}
              >
                {isRunning ? <IconStop className="size-4" /> : <IconPlay className="size-4" />}
                {isRunning ? "停止" : "启动"}
              </Button>
            </Tooltip.Trigger>
            <Tooltip.Content>
              {isRunning ? "停止后会中断该环境的代理服务，可再次启动" : "启动该环境的代理服务"}
            </Tooltip.Content>
          </Tooltip>

          {isRunning ? (
            <Button size="sm" variant="secondary" onPress={() => onRestart(env)}>
              <IconRestart className="size-4" />
              重启
            </Button>
          ) : (
            <Button aria-label="重启：仅在运行时可用" size="sm" variant="secondary" isDisabled>
              <IconRestart className="size-4" />
              重启
            </Button>
          )}

          {/* 危险档：不可逆，走 AlertDialog 二次确认 */}
          <Button
            className="btn-action--danger"
            size="sm"
            variant="outline"
            onPress={() => onDelete(env)}
          >
            <IconTrash className="size-4" />
            删除
          </Button>

          {onClose ? (
            <Tooltip delay={200}>
              <Tooltip.Trigger>
                <Button aria-label="关闭详情" size="sm" variant="ghost" onPress={onClose}>
                  <IconClose className="size-4" />
                </Button>
              </Tooltip.Trigger>
              <Tooltip.Content>关闭（Esc）</Tooltip.Content>
            </Tooltip>
          ) : null}
        </div>
      </div>

      {/* 页签带计数徽标（P4） */}
      <Tabs
        align="start"
        className="flex min-h-0 flex-1 flex-col"
        defaultSelectedKey="overview"
        variant="secondary"
      >
        <Tabs.ListContainer>
          <Tabs.List aria-label="环境详情">
            <Tabs.Tab id="overview">
              概览
              <Tabs.Indicator />
            </Tabs.Tab>
            <Tabs.Tab id="config">
              配置
              <Tabs.Indicator />
            </Tabs.Tab>
            <Tabs.Tab id="rules">
              规则
              <Chip className="ml-2" size="sm" variant="soft">
                <Chip.Label>{env.rules_count}</Chip.Label>
              </Chip>
              <Tabs.Indicator />
            </Tabs.Tab>
          </Tabs.List>
        </Tabs.ListContainer>

        <Tabs.Panel className="min-h-0 flex-1 overflow-y-auto p-4" id="overview">
          <OverviewTab env={env} hasIssue={hasIssue} isRunning={isRunning} onCopied={onCopied} />
        </Tabs.Panel>

        <Tabs.Panel className="min-h-0 flex-1 overflow-y-auto p-4" id="config">
          <ConfigTab env={env} onSave={onSave} />
        </Tabs.Panel>

        <Tabs.Panel className="min-h-0 flex-1 overflow-y-auto p-4" id="rules">
          <RulesTab env={env} />
        </Tabs.Panel>
      </Tabs>
    </>
  );
}

/* ------------------------------------------------------------------ */
/* 概览：分组信息（P5：分组先于平铺）                                   */
/* ------------------------------------------------------------------ */

function OverviewTab({
  env,
  hasIssue,
  isRunning,
  onCopied,
}: {
  env: EnvView;
  hasIssue: boolean;
  isRunning: boolean;
  onCopied: () => void;
}) {
  const [copied, setCopied] = useState(false);
  /* 代理命令由服务端回显（proxy_command），前端不重算（D-10） */
  const cmd = env.proxy_command;
  const publicListen = isPublicListen(env);
  const riskyExposure = publicListen && !env.proxy_auth_enabled;

  async function copyCommand() {
    try {
      await navigator.clipboard.writeText(cmd);
    } catch {
      /* 剪贴板不可用时仍给出视觉反馈，避免静默失败 */
    }
    setCopied(true);
    onCopied();
    window.setTimeout(() => setCopied(false), 1600);
  }

  return (
    <div className="flex flex-col gap-4">
      {hasIssue ? (
        <InlineNotice status="warning">
          期望状态与实际状态不一致：期望「{env.desired === "running" ? "运行中" : "已停止"}」，
          实际「{healthLabel(env.health)}」。
          {env.health_reason ? `原因：${env.health_reason}。` : ""}
        </InlineNotice>
      ) : null}

      {riskyExposure ? (
        <InlineNotice status="danger">
          代理将暴露给局域网且未启用访问鉴权 —— 任何能连通的机器都能借它发请求。
          <b>建议在「配置 → 高级配置」填写代理访问鉴权。</b>
        </InlineNotice>
      ) : null}

      <section>
        <SectionTitle>运行状态</SectionTitle>
        <KvGrid>
          <Kv k="实际状态" v={healthLabel(env.health)} tone={env.health === "running" ? "default" : "muted"} />
          <Kv
            k="期望状态"
            v={env.desired === "running" ? "运行中" : "已停止"}
            tone={hasIssue ? "warning" : "default"}
            alert={hasIssue}
          />
          <Kv k="监听" v={listenAddress(env)} />
          <Kv k="规则数" v={String(env.rules_count)} />
        </KvGrid>
      </section>

      <section>
        <SectionTitle>链路</SectionTitle>
        <KvGrid>
          <Kv k="上游代理" v={env.upstream ?? "直连"} tone={env.upstream ? "default" : "muted"} />
          <Kv
            k="规则绑定"
            v={env.rules === null ? "（不覆盖）" : env.rules_missing ? `${env.rules}（缺失）` : env.rules}
            tone={env.rules_missing ? "warning" : "default"}
          />
          <Kv k="对外服务" v={publicListen ? `${env.listen.host}（局域网可达）` : "仅 127.0.0.1"} />
          <Kv k="描述" v={env.description} />
        </KvGrid>
      </section>

      <section>
        <SectionTitle>安全姿态</SectionTitle>
        <KvGrid>
          <Kv
            k="代理鉴权"
            v={env.proxy_auth_enabled ? "已启用" : "未启用"}
            tone={env.proxy_auth_enabled ? "default" : "muted"}
          />
          <Kv
            k="放宽上游证书校验"
            v={env.insecure_hosts.length ? `${env.insecure_hosts.length} 个域名` : "未放宽"}
            tone={env.insecure_hosts.length ? "warning" : "default"}
          />
          <Kv
            k="局域网暴露风险"
            v={riskyExposure ? "高：无鉴权" : "无"}
            tone={riskyExposure ? "danger" : "default"}
            alert={riskyExposure}
          />
          <Kv k="抓包开关" v={env.capture ? "开启" : "关闭"} tone={env.capture ? "default" : "muted"} />
        </KvGrid>
      </section>

      {/* P1：代理命令首屏 + 一键复制 */}
      <section>
        <SectionTitle>接入方式</SectionTitle>
        <Surface className="overflow-hidden p-0" variant="secondary">
          <div className="flex items-center gap-2 border-b border-border px-3 py-2">
            <IconLink className="size-4 text-muted" />
            <span className="text-sm font-semibold text-foreground">代理命令</span>
            <span className="ml-auto" />
            <Hint>粘贴到终端即可让该进程走这个环境</Hint>
          </div>
          <div className="flex items-center gap-3 p-3">
            <code className="min-w-0 flex-1 break-all font-mono text-base leading-relaxed text-foreground">
              {cmd}
            </code>
            <Button
              className={`flex-none ${copied ? "text-success" : ""}`}
              size="sm"
              variant="secondary"
              onPress={copyCommand}
            >
              {copied ? <IconCheck className="size-4" /> : <IconCopy className="size-4" />}
              {copied ? "已复制" : "复制"}
            </Button>
          </div>
        </Surface>
      </section>

      {env.insecure_hosts.length ? (
        <Alert status="warning">
          <Alert.Indicator>
            <IconShield className="size-4" />
          </Alert.Indicator>
          <Alert.Content>
            <Alert.Title>上游证书校验已放宽</Alert.Title>
            <Alert.Description>
              命中的域名会跳过上游证书校验：{env.insecure_hosts.join("、")}。
              {isRunning ? "该环境正在运行，改动即时生效。" : ""}
            </Alert.Description>
          </Alert.Content>
        </Alert>
      ) : null}
    </div>
  );
}

/* ------------------------------------------------------------------ */
/* 配置：基础常驻 + 高级配置折叠（P8：渐进披露）                         */
/* ------------------------------------------------------------------ */

function ConfigTab({
  env,
  onSave,
}: {
  env: EnvView;
  onSave: (name: string, patch: EnvPatchReq) => void;
}) {
  const [dirty, setDirty] = useState(false);
  const [port, setPort] = useState(String(env.listen.port));
  const [rules, setRules] = useState(env.rules ?? "none");
  const [upstream, setUpstream] = useState(env.upstream ?? "direct");
  const [description, setDescription] = useState(env.description);
  const [insecureText, setInsecureText] = useState(env.insecure_hosts.join("\n"));
  const [isPublic, setIsPublic] = useState(isPublicListen(env));
  const [capture, setCapture] = useState(env.capture);
  const [authUser, setAuthUser] = useState("");
  const [authPass, setAuthPass] = useState("");
  const [isSaving, setIsSaving] = useState(false);

  /* 绑定选项来自真实账本（规则库 / 上游代理） */
  const [ruleNames, setRuleNames] = useState<string[]>([]);
  const [proxyNames, setProxyNames] = useState<string[]>([]);
  useEffect(() => {
    void api.rulesList().then((r) => setRuleNames(r.rules)).catch(() => setRuleNames([]));
    void api.proxiesList().then((p) => setProxyNames(p.map((x) => x.name))).catch(() => setProxyNames([]));
  }, []);

  const authHalfFilled = authUser.length > 0 !== authPass.length > 0;
  const authHasColon = /[:\s]/.test(authUser) || /[:\s]/.test(authPass);
  const authInvalid = authHalfFilled || authHasColon;
  const exposeRisk = isPublic && !(authUser && authPass);

  /* env 变化（SSE 刷新）时回到服务端值 */
  useEffect(() => {
    setPort(String(env.listen.port));
    setRules(env.rules ?? "none");
    setUpstream(env.upstream ?? "direct");
    setDescription(env.description);
    setInsecureText(env.insecure_hosts.join("\n"));
    setIsPublic(isPublicListen(env));
    setCapture(env.capture);
    setDirty(false);
  }, [env]);

  const portInvalid = port.length > 0 && !/^\d{1,5}$/.test(port);

  function submit() {
    const patch: EnvPatchReq = {};
    if (Number(port) !== env.listen.port) patch.listen = { ...env.listen, port: Number(port) };
    const nextHost = isPublic ? "0.0.0.0" : "127.0.0.1";
    if (nextHost !== env.listen.host) patch.listen = { ...(patch.listen ?? env.listen), host: nextHost };
    const nextRules = rules === "none" ? null : rules;
    if (nextRules !== env.rules) patch.rules = nextRules;
    const nextUpstream = upstream === "direct" ? null : upstream;
    if (nextUpstream !== env.upstream) patch.upstream = nextUpstream;
    if (description !== env.description) patch.description = description;
    const nextInsecure = insecureText
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean);
    if (JSON.stringify(nextInsecure) !== JSON.stringify(env.insecure_hosts)) {
      patch.insecure_hosts = nextInsecure;
    }
    if (capture !== env.capture) patch.capture = capture;
    if (authUser && authPass) {
      patch.proxy_user = authUser;
      patch.proxy_password = authPass;
    }

    setIsSaving(true);
    onSave(env.name, patch);
    /* 保存结果经 SSE 快照回流；成功/失败反馈由 Workbench 统一推送 */
    window.setTimeout(() => {
      setIsSaving(false);
      setDirty(false);
      setAuthUser("");
      setAuthPass("");
    }, 400);
  }

  return (
    <div className="flex max-w-3xl flex-col gap-4">
      {dirty ? (
        <InlineNotice
          status="warning"
          action={
            <Button size="sm" variant="ghost" onPress={() => setDirty(false)}>
              放弃改动
            </Button>
          }
        >
          有未保存的改动。端口与代理鉴权是<b>停机生效</b>字段，运行中保存会被拒绝。
        </InlineNotice>
      ) : null}

      <SectionTitle>基础</SectionTitle>

      <TextField
        className="w-full"
        value={description}
        onChange={(v) => {
          setDescription(v);
          setDirty(true);
        }}
      >
        <Label>描述</Label>
        <Input spellCheck={false} />
        <span className="text-sm text-muted">纯展示字段，热生效。</span>
      </TextField>

      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <TextField
          className="w-full"
          isInvalid={portInvalid}
          value={port}
          onChange={(v) => {
            setPort(v);
            setDirty(true);
          }}
        >
          <Label>监听端口</Label>
          <Input inputMode="numeric" spellCheck={false} />
          {portInvalid ? (
            <span className="text-sm text-danger">端口必须是数字。</span>
          ) : (
            <span className="text-sm text-muted">
              范围 16000–16999。<b>停机生效</b>。
            </span>
          )}
        </TextField>

        <Select
          className="w-full"
          selectedKey={rules}
          onSelectionChange={(key) => {
            setRules(String(key));
            setDirty(true);
          }}
        >
          <Label>规则绑定</Label>
          <Select.Trigger>
            <Select.Value />
            <Select.Indicator />
          </Select.Trigger>
          <Select.Popover>
            <ListBox>
              {ruleNames.map((name) => (
                <ListBox.Item key={name} id={name} textValue={name}>
                  {name}
                  <ListBox.ItemIndicator />
                </ListBox.Item>
              ))}
              <ListBox.Item id="none" textValue="（不覆盖）">
                （不覆盖）
                <ListBox.ItemIndicator />
              </ListBox.Item>
            </ListBox>
          </Select.Popover>
          <span className="text-sm text-muted">
            热生效：换绑定后运行中的实例立即按新规则改写，不必重启。
          </span>
        </Select>
      </div>

      <Select
        className="w-full"
        selectedKey={upstream}
        onSelectionChange={(key) => {
          setUpstream(String(key));
          setDirty(true);
        }}
      >
        <Label>上游代理</Label>
        <Select.Trigger>
          <Select.Value />
          <Select.Indicator />
        </Select.Trigger>
        <Select.Popover>
          <ListBox>
            <ListBox.Item id="direct" textValue="（直连）">
              （直连）
              <ListBox.ItemIndicator />
            </ListBox.Item>
            {proxyNames.map((name) => (
              <ListBox.Item key={name} id={name} textValue={name}>
                {name}
                <ListBox.ItemIndicator />
              </ListBox.Item>
            ))}
          </ListBox>
        </Select.Popover>
        <span className="text-sm text-muted">热生效。绑定后出方向先经所选代理再到目标。</span>
      </Select>

      {/* 渐进披露：安全相关的高级字段收进折叠 */}
      <Disclosure isExpanded={false}>
        <Disclosure.Heading>
          <Button className="w-full justify-between" slot="trigger" variant="secondary">
            <span className="flex items-center gap-2">
              <IconAlert className="size-4 text-muted" />
              高级配置
            </span>
            <span className="font-mono text-sm font-normal text-muted">4 项 · 安全相关</span>
          </Button>
        </Disclosure.Heading>
        <Disclosure.Content>
          <Disclosure.Body className="flex flex-col gap-4">
            <TextField
              className="w-full"
              value={insecureText}
              onChange={(v) => {
                setInsecureText(v);
                setDirty(true);
              }}
            >
              <Label>按域名放宽上游证书校验</Label>
              <TextArea rows={3} spellCheck={false} />
              <span className="text-sm leading-relaxed text-muted">
                每行一个<b>完整域名</b>，精确匹配：列出的域名才跳过上游证书校验，子域不继承、后缀不匹配，通配符会被服务端拒绝。热生效。
              </span>
            </TextField>

            <Checkbox
              isSelected={isPublic}
              onChange={(v) => {
                setIsPublic(v);
                setDirty(true);
              }}
            >
              <Checkbox.Content>
                <Checkbox.Control>
                  <Checkbox.Indicator />
                </Checkbox.Control>
                <span className="text-base">对外服务（绑定 0.0.0.0）</span>
              </Checkbox.Content>
            </Checkbox>
            <Hint>默认只监听 127.0.0.1；勾选后局域网内可直接连此代理。停机生效。</Hint>

            <Checkbox
              isSelected={capture}
              onChange={(v) => {
                setCapture(v);
                setDirty(true);
              }}
            >
              <Checkbox.Content>
                <Checkbox.Control>
                  <Checkbox.Indicator />
                </Checkbox.Control>
                <span className="text-base">抓包记录（只控记录，清空走调试页显式动作）</span>
              </Checkbox.Content>
            </Checkbox>
            <Hint>热生效。开启后该环境实例记录 HTTPS 请求/响应，供「调试」视图消费。</Hint>

            <div className="flex flex-col gap-2">
              <Label>代理访问鉴权</Label>
              <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                <TextField
                  aria-label="代理用户名"
                  className="w-full"
                  isInvalid={authInvalid}
                  value={authUser}
                  onChange={setAuthUser}
                >
                  <Input placeholder="用户名" spellCheck={false} />
                </TextField>
                <TextField
                  aria-label="代理密码"
                  className="w-full"
                  isInvalid={authInvalid}
                  value={authPass}
                  onChange={setAuthPass}
                >
                  <Input placeholder="留空则保持不变" spellCheck={false} type="password" />
                </TextField>
              </div>
              <span className={`text-sm leading-relaxed ${authInvalid ? "text-danger" : "text-muted"}`}>
                {authInvalid
                  ? "两栏必须同时填写，且都不能含「:」或空白字符。"
                  : "两栏必须同时填写，都不能含「:」或空白字符。凭据只进不出 —— 服务端永不回显。停机生效。"}
              </span>
            </div>

            {exposeRisk ? (
              <InlineNotice status="warning">
                代理将暴露给局域网且未启用访问鉴权 —— 任何能连通的机器都能借它发请求。
                <b>建议填写上方的「代理访问鉴权」。</b>
              </InlineNotice>
            ) : null}
          </Disclosure.Body>
        </Disclosure.Content>
      </Disclosure>

      <Separator />

      <div className="flex items-center gap-2">
        {/* 保存用次档：该视图唯一主操作配额已由「新建环境」占用（D-4） */}
        <Button
          className="font-semibold"
          size="sm"
          variant="secondary"
          isDisabled={nameInvalidOrEmpty(portInvalid, authInvalid)}
          isPending={isSaving}
          onPress={submit}
        >
          保存
        </Button>
        <Button size="sm" variant="ghost" isDisabled={!dirty} onPress={() => setDirty(false)}>
          放弃改动
        </Button>
        <span className="ml-auto" />
        <Hint>端口与代理鉴权为停机生效字段。</Hint>
      </div>
    </div>
  );
}

function nameInvalidOrEmpty(portInvalid: boolean, authInvalid: boolean): boolean {
  return portInvalid || authInvalid;
}

/* ------------------------------------------------------------------ */
/* 规则：真实正文预览（GET /api/rules/:name）                           */
/* ------------------------------------------------------------------ */

function RulesTab({ env }: { env: EnvView }) {
  const [text, setText] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setText(null);
    if (env.rules === null || env.rules_missing) return;
    void api
      .rulesGet(env.rules)
      .then((r) => {
        if (!cancelled) setText(r.text);
      })
      .catch(() => {
        if (!cancelled) setText("（读取失败）");
      });
    return () => {
      cancelled = true;
    };
  }, [env.rules, env.rules_missing]);

  const preview = useMemo(() => {
    if (text === null) return null;
    const lines = text.split("\n").filter((l) => l.trim().length > 0 && !l.trim().startsWith("#"));
    const head = lines.slice(0, 6);
    const rest = lines.length - head.length;
    return head.join("\n") + (rest > 0 ? `\n… 另有 ${rest} 条` : "");
  }, [text]);

  if (env.rules === null) {
    return (
      <div className="flex flex-col gap-4">
        <Hint>该环境未绑定规则集，不覆盖任何域名。在「配置」页签绑定后即可按 hosts 规则改写上游目标。</Hint>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-4">
      <Surface className="overflow-hidden p-0">
        <div className="flex items-center gap-2 border-b border-border px-4 py-3">
          <h3 className="font-mono text-base font-semibold text-foreground">{env.rules}</h3>
          <Chip color="accent" size="sm" variant="soft">
            <Chip.Label>{env.rules_count} 条 hosts 规则</Chip.Label>
          </Chip>
          <span className="ml-auto" />
          {env.rules_missing ? (
            <Chip color="warning" size="sm" variant="soft">
              <Chip.Label>账本中缺失</Chip.Label>
            </Chip>
          ) : null}
          <a
            className="text-sm text-muted underline decoration-border underline-offset-4"
            href={`/api/rules/${encodeURIComponent(env.rules)}`}
            rel="noreferrer"
            target="_blank"
          >
            查看原文
          </a>
        </div>
        <ScrollShadow className="max-h-56">
          <pre className="whitespace-pre p-3 font-mono text-sm leading-relaxed text-muted">
            {preview ?? "加载中…"}
          </pre>
        </ScrollShadow>
      </Surface>
      <Hint>hosts 规则热生效：改动后立即作用于运行中的实例，不必重启。</Hint>
    </div>
  );
}

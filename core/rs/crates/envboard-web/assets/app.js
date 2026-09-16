// envboard 工作台 —— 原生 JS，无框架、无构建链。
//
// 五条约束（前端与服务端必须一致）：
//   · 变更类请求必须带 `x-envboard-request: 1`（跨站简单请求带不了自定义头，
//         所以这一条就是 CSRF 防线）；配了 token 时还要带 `x-envboard-token`。
//   · SSE 推的是"当前全部环境的快照"，前端不维护增量状态。
//   · DOM 必须可断言：环境项带 data-env，健康徽章的文本就是健康值本身。
//   · **不写内联 style、不写 onclick**：CSP 里没有 'unsafe-inline'，内联样式属性与
//         内联事件处理器会被静默拒绝。显示/隐藏一律走 .is-hidden，定位一律交给 CSS。
//   · 只在数据真的变了才重建 DOM：SSE 每秒推一次快照，无脑重建会把按钮从用户鼠标
//         底下换掉（点击丢失）、让辅助技术的引用立刻失效，也白白烧 CPU。

const REQUEST_HEADER = "x-envboard-request";
const TOKEN_HEADER = "x-envboard-token";
const SVG_NS = "http://www.w3.org/2000/svg";

/// 健康状态的**唯一**语义表：标签（给人读）/ 视觉（CSS class）/ 建议动作（见下面的 ADVICE）。
/// 只映射 class 而不给标签，用户看到的就是 `config_mismatch` 这种原始值，
/// 而且三类「坏」共用同一个 error 配色 —— 它们恰恰最需要被分辨：
/// 配置未生效是契约下发静默失败，端口冲突是端口被占，启动失败是进程没起来。
/// 徽章文本改成了标签，所以原文另存到 `data-health`：断言与排障读那个属性，
/// 不要读文案（文案会随语言和措辞变）。
const HEALTH_META = {
  running: { label: "运行中", style: "running" },
  stopped: { label: "已停止", style: "stopped" },
  starting: { label: "启动中", style: "running" },
  unhealthy: { label: "不健康", style: "unhealthy" },
  config_mismatch: { label: "配置未生效", style: "error" },
  port_conflict: { label: "端口冲突", style: "error" },
  failed: { label: "启动失败", style: "error" },
};

/// 未知状态不吞掉：原样显示，好过显示一个编造的标签。
const healthLabel = (health) => (HEALTH_META[health] || {}).label || health;
const healthStyle = (health) => (HEALTH_META[health] || {}).style || "";

/// 需要用户介入的状态 —— 「需处理」统计卡与筛选都按它算。
const ISSUE_HEALTH = new Set(["unhealthy", "config_mismatch", "port_conflict", "failed"]);

const state = {
  status: null,
  environments: [],
  rules: [],
  token: "",
  /// 当前选中的环境：详情区与日志栏都跟着它走。
  selected: null,
  /// 非空 = 表单处于编辑模式，值是**改名前的原名**（PATCH 要打在这个名字上）。
  editing: null,
  view: "environments",
  tab: "overview",
  /// all | running | issues | rules
  filter: "all",
  search: "",
  /// 次级操作行是否展开（就地展开，不是浮层）。
  moreOpen: false,
  /// 就地二次确认的目标："env:beta" / "rule:beta" / "reallocate:beta"；
  /// null = 没有待确认的破坏性操作。
  confirm: null,
  /// 最近一次成功拿到快照的本地时间（HH:MM:SS）。SSE 断线时界面照旧显示旧数据，
  /// 有了它用户至少能看出"这份数据有多旧"。
  lastOk: null,
  /// SSE 是否还活着 —— 连接徽章与侧栏底部圆点都看它。
  streamOk: false,
  // ---- 日志栏 ----
  logsEnv: null,
  logLines: [],
  logKind: "all",
  logSearch: "",
  logFollow: true,
  logAutoscroll: true,
  logCollapsed: false,
  logRenderKey: "",
};

// --------------------------------------------------------------------------- //
// HTTP 小工具
// --------------------------------------------------------------------------- //

function headers(json) {
  const result = {};
  if (json) result["content-type"] = "application/json";
  result[REQUEST_HEADER] = "1";
  if (state.token) result[TOKEN_HEADER] = state.token;
  return result;
}

async function api(path, options = {}) {
  // **每个请求都在这里统一带上凭据与 CSRF 头**：曾经只有 `mutate()` 显式传 headers，
  // 于是所有 GET（日志、规则原文、跨环境对比）在开了 token 后一起 401 —— 环境列表
  // 走的是 SSE 快照（URL 带 token），所以看着"只有日志坏了"。
  // 收口到一处，新增调用点不可能再忘记。
  const response = await fetch(path, {
    ...options,
    headers: { ...headers(options.body !== undefined), ...(options.headers || {}) },
  });
  const text = await response.text();
  let body = null;
  if (text) {
    try {
      body = JSON.parse(text);
    } catch (error) {
      throw new Error(`响应不是 JSON：${text.slice(0, 120)}`);
    }
  }
  if (!response.ok) {
    const detail = body && body.error ? `${body.error.code}: ${body.error.message}` : text;
    const error = new Error(detail || `HTTP ${response.status}`);
    error.field = body && body.error ? body.error.field : null;
    throw error;
  }
  return body;
}

const get = (path) => api(path);
const mutate = (path, method, payload) =>
  api(path, {
    method,
    headers: headers(payload !== undefined),
    body: payload === undefined ? undefined : JSON.stringify(payload),
  });

const environmentPath = (name) => `/api/environments/${encodeURIComponent(name)}`;

/// 进行中的动作集合，键形如 `start:beta`。
///
/// 用它而不是按钮引用：SSE 每秒可能重建 DOM，引用会失效；而"这个动作正在跑"
/// 是数据层的事实，重建后照样能把它渲染成禁用态。
const pending = new Set();
const isPending = (key) => pending.has(key);

// --------------------------------------------------------------------------- //
// DOM 小工具与组件工厂
// --------------------------------------------------------------------------- //

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined && text !== null) node.textContent = text;
  return node;
}

/// 图标：引用 index.html 里的 <symbol>。描边/圆角由 .ic 决定，颜色继承 currentColor，
/// 所以同一份图标在明暗主题下都成立。
function icon(name, size) {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("class", size ? `ic ${size}` : "ic");
  svg.setAttribute("aria-hidden", "true");
  const use = document.createElementNS(SVG_NS, "use");
  use.setAttribute("href", `#i-${name}`);
  svg.appendChild(use);
  return svg;
}

function setIcon(button, name) {
  const use = button.querySelector("svg.ic use");
  if (use) use.setAttribute("href", `#i-${name}`);
}

const field = (form, name) => form.querySelector(`[name="${name}"]`);

function button({ label, icon: iconName, className = "btn", title, onClick, key }) {
  const node = el("button", className);
  node.type = "button";
  if (title) node.title = title;
  if (iconName) node.appendChild(icon(iconName, className.includes("sm") ? "ic-sm" : ""));
  if (label) node.appendChild(el("span", null, label));
  // 图标按钮没有可见文本，必须有可读名字；有文本的按钮让文本自己当名字，
  // 只有在 title 更具体时才用 title 覆盖它。
  if (title || !label) node.setAttribute("aria-label", title || label || "操作");
  if (key && isPending(key)) {
    node.classList.add("is-busy");
    node.disabled = true;
  }
  if (onClick) node.addEventListener("click", onClick);
  return node;
}

/// 状态徽章：圆点 + 文本。文本是健康值原值，颜色由 --ok / --warn / --bad 决定。
function healthBadge(health) {
  const node = el("span", `badge ${healthStyle(health)}`.trim());
  const dot = el("span", "dot");
  dot.setAttribute("aria-hidden", "true");
  node.setAttribute("data-health", health);
  node.appendChild(dot);
  node.appendChild(el("span", null, healthLabel(health)));
  return node;
}

// --------------------------------------------------------------------------- //
// 轻提示
// --------------------------------------------------------------------------- //

const TOAST_MAX = 3;
const TOAST_MS = { ok: 2600, info: 2600, bad: 6000 };

/// 提示按"新的在上"堆叠，并且**不再互相顶掉** —— 旧实现只保留一条，
/// 连续操作时前一条信息会被后一条吃掉（"已复制"被随后的失败顶掉过）。
function toast(message, kind = "info") {
  const host = document.getElementById("toasts");
  const node = el("div", `toast ${kind}`);
  node.appendChild(icon(kind === "ok" ? "check" : kind === "bad" ? "alert" : "info"));
  node.appendChild(el("span", null, message));
  host.prepend(node);
  while (host.children.length > TOAST_MAX) host.lastElementChild.remove();
  setTimeout(() => node.remove(), TOAST_MS[kind] || 2600);
}

function reportError(error) {
  toast(error.field ? `${error.message}（字段：${error.field}）` : error.message, "bad");
}

// --------------------------------------------------------------------------- //
// 渲染：顶栏 / 统计
// --------------------------------------------------------------------------- //

/// 事件流与改写能力合成一句话：SSE 断开时以它为准，因为那是当下最要紧的事实。
function connectionState() {
  if (!state.streamOk) return { kind: "warn", text: "事件流断开（界面仍在轮询）" };
  const upstream = state.status && state.status.capabilities.rewrite_upstream;
  return upstream
    ? { kind: "ok", text: "真实改写已启用" }
    : { kind: "warn", text: "仅监听（不改写）" };
}

/// 只在值真的变了才写 DOM。不这么写的话，每秒一次的 SSE 快照会重写同一段文案，
/// 产生一批无意义的 mutation（实测 8 秒 8 次，全部落在这些静态文案上）。
function setText(node, value) {
  if (node.textContent !== value) node.textContent = value;
}

/// 暴露面：工作台只监听回环时写"仅本机可访问"，否则如实标出非本机监听。
/// 这个判断放在前端做是因为 /api/status 不暴露监听地址（也就不必改接口契约）——
/// 浏览器自己知道它访问的是哪个地址。
function exposureText() {
  const host = location.hostname;
  const loopback = host === "127.0.0.1" || host === "localhost" || host === "[::1]" || host === "::1";
  return loopback ? "仅本机可访问" : "非本机监听";
}

/// 记下"这份数据是什么时候拿到的"，页脚用它说明新鲜度。
function stampOk() {
  const now = new Date();
  const pad = (value) => String(value).padStart(2, "0");
  state.lastOk = `${pad(now.getHours())}:${pad(now.getMinutes())}:${pad(now.getSeconds())}`;
}

function renderChrome() {
  const status = state.status;
  if (status) {
    setText(document.getElementById("core-badge-text"), `core: ${status.core.name} ${status.core.version}`);
    // 页脚给「我在访问谁 + 暴露面 + 数据有多新」，state_dir 是开发向信息，挪出页脚
    // （它在 /api/status 与 README 里都能查到）。
    setText(
      document.getElementById("sidebar-foot-text"),
      `${location.host} · ${exposureText()} · 更新于 ${state.lastOk || "—"}`,
    );
    setText(
      document.getElementById("env-form-port-hint"),
      `留空则从端口区间 ${status.config.port_range} 里自动分配`,
    );
  }
  const connection = connectionState();
  const badge = document.getElementById("conn-badge");
  badge.className = `badge ${connection.kind}`;
  setText(document.getElementById("conn-badge-text"), connection.text);
  const dot = document.getElementById("conn-dot");
  dot.className = `conn-dot is-${connection.kind}`;
  renderStats();
}

function renderStats() {
  const list = state.environments;
  const running = list.filter((env) => env.health === "running").length;
  const issues = list.filter((env) => ISSUE_HEALTH.has(env.health)).length;
  setText(document.getElementById("stat-total"), String(list.length));
  setText(document.getElementById("stat-running"), String(running));
  setText(document.getElementById("stat-issues"), String(issues));
  setText(document.getElementById("stat-rules"), String(state.rules.length));
  // 侧栏计数跟着**当前可见**的条数走：否则筛到空列表时右边还挂着"2"，
  // 与紧挨着的空态文案自相矛盾。
  setText(document.getElementById("env-count"), String(visibleEnvironments().length));
  setText(document.getElementById("rules-count"), String(state.rules.length));
}

// --------------------------------------------------------------------------- //
// 渲染：侧栏环境列表（含筛选与搜索）
// --------------------------------------------------------------------------- //

const FILTER_LABEL = { all: "全部", running: "运行中", issues: "需处理", rules: "已绑定规则" };

function matchesFilter(env) {
  switch (state.filter) {
    case "running":
      return env.health === "running";
    case "issues":
      return ISSUE_HEALTH.has(env.health);
    case "rules":
      return Boolean(env.rules);
    default:
      return true;
  }
}

function matchesSearch(env) {
  const needle = state.search.trim().toLowerCase();
  if (!needle) return true;
  return [
    env.name,
    String(env.listen.port),
    env.rules || "",
    env.description || "",
    insecureHosts(env).join(" "),
  ].some((value) => value.toLowerCase().includes(needle));
}

/// 「放宽校验域名」清单：契约里一定是数组，但视图字段缺失时按空列表处理，
/// 别让一次字段遗漏把整块渲染打断。
const insecureHosts = (env) => (Array.isArray(env.insecure_hosts) ? env.insecure_hosts : []);

/// 概览里的写法：`,` 分隔的一行（`—` = 全部严格校验）。
const insecureHostsText = (env) => insecureHosts(env).join(", ");

/// 运行中必须锁住的控件（契约里的**停机字段**）：名字 / 监听地址（`public` 切 host）/
/// 代理凭据（实例启动时经 `--set proxyauth=…` 固定）。`description`、`rules` 绑定与
/// `insecure_hosts` 是**热**字段，运行中照样可改，所以不在这里。
const STOP_REQUIRED_FIELDS = ["name", "port", "public", "proxy_user", "proxy_password", "proxy_clear"];

const visibleEnvironments = () =>
  state.environments.filter((env) => matchesFilter(env) && matchesSearch(env));

let sidebarKey = "";

function renderSidebar(force) {
  const list = visibleEnvironments();
  const key = JSON.stringify([
    list.map((env) => [
      env.name,
      env.health,
      env.listen.port,
      env.rules,
      env.rules_count,
      env.rules_missing,
      env.desired,
    ]),
    state.selected,
    state.filter,
    state.search,
    [...pending],
  ]);
  if (!force && key === sidebarKey) return;
  sidebarKey = key;

  const host = document.getElementById("env-list");
  host.replaceChildren();

  if (!state.environments.length) {
    host.appendChild(el("li", "empty-inline", "还没有环境。点上面的「+」建一个。"));
    return;
  }
  if (!list.length) {
    host.appendChild(el("li", "empty-inline", "没有符合条件的环境。"));
    return;
  }
  for (const env of list) host.appendChild(envItem(env));
}

function envItem(env) {
  const item = el("li", `env-item${env.name === state.selected ? " is-active" : ""}`);
  item.dataset.env = env.name;

  const main = el("button", "env-main");
  main.type = "button";
  main.setAttribute("aria-current", String(env.name === state.selected));
  main.addEventListener("click", () => selectEnvironment(env.name));

  const top = el("div", "env-top");
  top.appendChild(el("span", "env-name mono", env.name));
  top.appendChild(healthBadge(env.health));
  main.appendChild(top);

  const meta = el("div", "env-meta");
  meta.appendChild(el("span", "mono", `:${env.listen.port}`));
  // 绑了规则名但绑定没生效（规则缺失）= 这个环境其实**不覆盖任何域名**。
  // 不是错误态（健康仍是 running），但绝不能静默 —— 列表行里就得看得出来。
  const rulesText = env.rules ? `${env.rules} (${env.rules_count})` : "不覆盖";
  const rulesNode = el(
    "span",
    "env-rules mono",
    env.rules_missing ? `${rulesText} · 规则缺失（已忽略）` : rulesText,
  );
  if (env.rules_missing) rulesNode.classList.add("is-inert");
  meta.appendChild(rulesNode);
  main.appendChild(meta);
  item.appendChild(main);

  // 侧栏只留一个"最能推进当前状态"的动作：期望运行 → 停止，否则 → 启动。
  const running = env.desired === "running";
  const action = running ? "stop" : "start";
  const key = `${action}:${env.name}`;
  item.appendChild(
    button({
      icon: running ? "stop" : "play",
      className: "btn icon-btn sm env-quick",
      title: `${running ? "停止" : "启动"} ${env.name}`,
      key,
      onClick: () => runAction(key, () => mutate(`${environmentPath(env.name)}/${action}`, "POST")),
    }),
  );
  return item;
}

function syncFilterUi() {
  for (const card of document.querySelectorAll(".stat-card")) {
    // 「环境总数」代表"不筛选"，它不是一个被选中的筛选项：给它常亮高亮会让首屏
    // 看起来已经选中了某个条件，而用户并没有点过。所以只有真正筛了某个状态才有激活态。
    const active = state.filter !== "all" && card.dataset.filter === state.filter;
    card.classList.toggle("is-active", active);
    card.setAttribute("aria-pressed", String(active));
  }
  const bar = document.getElementById("filter-bar");
  const searching = Boolean(state.search.trim());
  if (state.filter === "all" && !searching) {
    bar.classList.add("is-hidden");
    return;
  }
  bar.classList.remove("is-hidden");
  document.getElementById("filter-tag").textContent = searching
    ? `搜索「${state.search.trim()}」${state.filter === "all" ? "" : ` · ${FILTER_LABEL[state.filter]}`}`
    : `筛选：${FILTER_LABEL[state.filter]}`;
}

// --------------------------------------------------------------------------- //
// 渲染：详情面板
// --------------------------------------------------------------------------- //

const currentEnvironment = () =>
  state.environments.find((env) => env.name === state.selected) || null;

const ADVICE = {
  unhealthy: "　建议：先看底部日志栏的尾部输出，再决定重启还是先改规则。",
  config_mismatch: "　建议：停止后重新启动，让实例与实际配置对齐。",
  port_conflict: "　建议：用「更多操作 → 重分配端口」，或先停掉占用该端口的实例。",
  failed: "　建议：看日志尾巴上的报错，修掉后重新启动。",
};

let detailKey = "";

function renderDetail(force) {
  const env = currentEnvironment();
  const empty = document.getElementById("detail-empty");
  const detail = document.getElementById("detail");

  if (!env) {
    empty.hidden = false;
    detail.hidden = true;
    renderTabs();
    return;
  }
  empty.hidden = true;
  detail.hidden = false;

  const key = JSON.stringify([
    env,
    state.editing,
    state.tab,
    state.confirm,
    state.moreOpen,
    [...pending],
  ]);
  if (!force && key === detailKey) return;
  detailKey = key;

  document.getElementById("detail-name").textContent = env.name;
  const badge = document.getElementById("detail-badge");
  badge.className = `badge ${healthStyle(env.health)}`.trim();
  badge.setAttribute("data-health", env.health);
  document.getElementById("detail-badge-text").textContent = healthLabel(env.health);
  badge.hidden = false;

  renderDetailActions(env);
  renderMore(env);
  renderReason(env);
  renderOverview(env);
  renderBoundRules(env);
  renderTabs();
  if (state.editing) {
    const target = state.environments.find((item) => item.name === state.editing);
    if (target) syncEditLock(target);
  }
}

function renderDetailActions(env) {
  const host = document.getElementById("detail-actions");
  host.replaceChildren();

  // 破坏性操作走就地二次确认：不用原生 confirm（阻塞、样式不可控、且与"禁止原生弹窗"
  // 的约束冲突），改成把动作行换成一句"后果说明 + 确认/取消"。
  if (state.confirm === `env:${env.name}`) {
    const box = el("div", "confirm");
    box.appendChild(
      el("span", "confirm-text", `删除 ${env.name}？实例会先被停掉；规则文件本身不受影响。`),
    );
    const key = `delete:${env.name}`;
    box.appendChild(
      button({
        label: "确认删除",
        icon: "trash",
        className: "btn danger sm",
        key,
        onClick: () =>
          runAction(key, async () => {
            state.confirm = null;
            return mutate(environmentPath(env.name), "DELETE");
          }),
      }),
    );
    box.appendChild(
      button({
        label: "取消",
        className: "btn ghost sm",
        onClick: () => {
          state.confirm = null;
          renderDetail(true);
        },
      }),
    );
    host.appendChild(box);
    return;
  }

  const running = env.desired === "running";
  const action = running ? "stop" : "start";
  const primaryKey = `${action}:${env.name}`;
  host.appendChild(
    button({
      label: running ? "停止" : "启动",
      icon: running ? "stop" : "play",
      className: "btn primary",
      key: primaryKey,
      onClick: () => runAction(primaryKey, () => mutate(`${environmentPath(env.name)}/${action}`, "POST")),
    }),
  );

  const restartKey = `restart:${env.name}`;
  host.appendChild(
    button({
      label: "重启",
      icon: "restart",
      key: restartKey,
      onClick: () => runAction(restartKey, () => mutate(`${environmentPath(env.name)}/restart`, "POST")),
    }),
  );

  host.appendChild(
    button({
      label: "更多操作",
      icon: state.moreOpen ? "chevron-down" : "more",
      className: `btn${state.moreOpen ? " is-active" : ""}`,
      title: "编辑配置 / 复制代理命令 / 跟随日志 / 重分配端口 / 删除环境",
      onClick: () => {
        state.moreOpen = !state.moreOpen;
        renderDetail(true);
      },
    }),
  );
}

/// 次级操作：就地展开的一行。浮层要按锚点算坐标，而 CSP 不允许内联 style；
/// 展开行没有这个问题，键盘与读屏器也不必处理"浮层焦点陷阱"。
function renderMore(env) {
  const host = document.getElementById("detail-more");
  if (!state.moreOpen) {
    host.classList.add("is-hidden");
    host.replaceChildren();
    return;
  }
  host.classList.remove("is-hidden");
  host.replaceChildren();

  host.appendChild(
    button({
      label: "编辑配置",
      icon: "edit",
      className: "btn sm",
      onClick: () => startEdit(env),
    }),
  );
  host.appendChild(
    button({
      label: "复制代理命令",
      icon: "copy",
      className: "btn sm",
      onClick: (event) => copyText(env.proxy_command, "代理命令已复制到剪贴板", event.currentTarget),
    }),
  );
  host.appendChild(
    button({
      label: "跟随日志",
      icon: "log",
      className: "btn sm",
      onClick: () => {
        state.logFollow = true;
        syncLogFollowButton();
        loadLogs(true);
      },
    }),
  );
  // 重分配端口是**二级操作**：它不会丢数据，但会让客户端此前的 export https_proxy=…
  // 立即失效。所以它和删除环境一样走就地二次确认，并把后果写清楚 ——
  // 只在 port_conflict 下出现，而那正是用户最想"赶紧点一下"的时刻。
  if (env.health === "port_conflict") {
    const key = `reallocate:${env.name}`;
    if (state.confirm === key) {
      const box = el("div", "confirm");
      box.appendChild(
        el(
          "span",
          "confirm-text",
          `重新分配 ${env.name} 的监听端口？端口会变，客户端现有的 export https_proxy=… 将立即失效，需要同步更新。`,
        ),
      );
      box.appendChild(
        button({
          label: "确认重分配",
          icon: "shuffle",
          className: "btn danger sm",
          key,
          onClick: () =>
            runAction(key, async () => {
              state.confirm = null;
              return mutate(`${environmentPath(env.name)}/reallocate`, "POST");
            }),
        }),
      );
      box.appendChild(
        button({
          label: "取消",
          className: "btn ghost sm",
          onClick: () => {
            state.confirm = null;
            renderDetail(true);
          },
        }),
      );
      host.appendChild(box);
    } else {
      host.appendChild(
        button({
          label: "重分配端口",
          icon: "shuffle",
          className: "btn sm",
          key,
          onClick: () => {
            state.confirm = key;
            renderDetail(true);
          },
        }),
      );
    }
  }
  host.appendChild(
    button({
      label: "删除环境",
      icon: "trash",
      className: "btn sm danger",
      onClick: () => {
        state.confirm = `env:${env.name}`;
        renderDetail(true);
      },
    }),
  );
}

function renderReason(env) {
  const host = document.getElementById("detail-reason");
  const text = document.getElementById("detail-reason-text");
  const reason = env.health_reason;
  if (!reason) {
    host.classList.add("is-hidden");
    text.textContent = "";
    return;
  }
  // 异常态三通道：颜色 + 原因 + 建议动作。缺一条，用户就只能猜。
  const severe = ISSUE_HEALTH.has(env.health) && env.health !== "unhealthy";
  host.classList.remove("is-hidden");
  host.classList.toggle("danger", severe);
  host.classList.toggle("warn", !severe);
  text.textContent = `${reason}${ADVICE[env.health] || ""}`;
}

function renderOverview(env) {
  // 期望 vs 实际：两者不一致是最该被一眼看见的运维信号。原来两格都是原始英文值、
  // 要人肉比对；现在给标签，并在不一致时把「实际状态」标红。
  const healthCell = document.getElementById("ov-health");
  healthCell.textContent = healthLabel(env.health);
  healthCell.dataset.health = env.health;
  healthCell.classList.toggle("is-mismatch", env.desired !== env.health);
  const portCell = document.getElementById("ov-port");
  portCell.textContent = `${env.listen.host}:${env.listen.port}`;
  // 非回环监听 = 暴露面变大，必须被一眼看见：加个 warn 类（CSP 禁内联 style，走 CSS）
  portCell.classList.toggle("is-warning", !isLoopbackHost(env.listen.host));
  document.getElementById("ov-desired").textContent = healthLabel(env.desired);
  // 规则绑定：绑了名字但绑定没生效时（规则缺失）这里必须写出来 —— 那一行看着"绑了规则"，
  // 实际该环境不覆盖任何域名，是"看起来生效其实没有"里最坏的一种。
  const rulesCell = document.getElementById("ov-rules");
  const rulesMissing = Boolean(env.rules) && Boolean(env.rules_missing);
  rulesCell.textContent = rulesMissing
    ? `${env.rules} · 规则缺失（已忽略）`
    : env.rules || "（不覆盖）";
  rulesCell.classList.toggle("is-inert", rulesMissing);
  document.getElementById("ov-rules-count").textContent = env.rules ? String(env.rules_count) : "—";
  // 放宽校验域名：整份清单直接列出来（通常只有几条）。非空 = 有域名被放宽，标成 warn 色
  // —— 它是安全姿态的放宽，不是错误，但必须与规则/描述这类普通单元格区分开。
  const hostsCell = document.getElementById("ov-insecure-hosts");
  const hosts = insecureHostsText(env);
  hostsCell.textContent = hosts || "—";
  hostsCell.classList.toggle("is-relaxed", Boolean(hosts));
  document.getElementById("ov-desc").textContent = env.description || "—";
  document.getElementById("ov-auth").textContent = env.proxy_auth_enabled ? "已启用" : "未启用";
  const command = document.getElementById("ov-cmd");
  command.textContent = env.proxy_command;
}

function renderTabs() {
  for (const tab of document.querySelectorAll(".tab")) {
    const active = tab.dataset.tab === state.tab;
    tab.classList.toggle("is-active", active);
    tab.setAttribute("aria-selected", String(active));
    tab.tabIndex = active ? 0 : -1;
  }
  for (const panel of document.querySelectorAll(".tab-panel")) {
    panel.classList.toggle("is-hidden", panel.dataset.panel !== state.tab);
  }
}

/// 「规则」标签页：展示当前环境绑定的规则文件内容。
///
/// 这不是装饰 —— "绑了个已经不存在的规则文件"以前只能从下拉菜单里那一行小字看出来，
/// 而它恰好是 `start` 会响亮失败的成因。这里把它变成一眼可见的状态。
function renderBoundRules(env) {
  const host = document.getElementById("bound-rules");
  const name = env.rules;

  if (!name) {
    const box = el("div", "empty-state");
    const iconHost = el("span", "empty-icon");
    iconHost.appendChild(icon("server", "ic-lg"));
    box.appendChild(iconHost);
    box.appendChild(el("p", "empty-title", "这个环境不覆盖任何域名"));
    box.appendChild(
      el(
        "p",
        "empty-hint",
        "所有请求原样直连。要改写上连目标，先在「规则库」导入一份规则，再到「配置」里绑定它。",
      ),
    );
    host.replaceChildren(box);
    return;
  }

  host.replaceChildren(el("p", "empty-inline", "正在读取规则内容…"));
  loadRuleText(name).then((entry) => {
    // 结果可能已经过期（用户切走了 / 换了绑定）：不做过期检查的话，
    // 慢响应会把上一个环境的内容画到当前环境上。
    const current = currentEnvironment();
    if (!current || current.name !== env.name || current.rules !== name) return;

    if (entry.error) {
      const notice = el("div", "notice warn");
      notice.appendChild(icon("alert", "ic-sm"));
      notice.appendChild(
        el(
          "span",
          null,
          `读不到规则文件 ${name}：${entry.error}　这份绑定当前不生效（已忽略，环境不覆盖任何域名）。` +
            `建议：在「规则库」重新导入同名文件，或把绑定改成（不覆盖）。`,
        ),
      );
      host.replaceChildren(notice);
      return;
    }

    const head = el("div", "bound-head");
    const tag = el("span", "badge purple");
    const tagDot = el("span", "dot");
    tagDot.setAttribute("aria-hidden", "true");
    tag.appendChild(tagDot);
    tag.appendChild(el("span", "mono", name));
    head.appendChild(tag);
    if (env.rules_missing) head.appendChild(el("span", "badge warn", "规则缺失（已忽略）"));
    head.appendChild(el("span", "rule-meta", `${entry.text.split("\n").filter(Boolean).length} 行 / ${entry.text.length} 字符`));
    head.appendChild(
      button({
        label: "去规则库编辑",
        icon: "edit",
        className: "btn sm",
        onClick: () => {
          setView("rules");
          loadRule(name);
        },
      }),
    );
    host.replaceChildren(head, el("pre", "rule-preview", entry.text || "（文件是空的）"));
  });
}

// --------------------------------------------------------------------------- //
// 渲染：规则库
// --------------------------------------------------------------------------- //

let rulesKey = "";

function renderRules(force) {
  const names = state.rules;
  // 环境可能绑着一个**已经被删掉**的规则名（那时 start 会响亮失败）。这个选项必须
  // 留在下拉里：否则打开编辑再保存，就会把"绑了个不存在的规则"悄悄改成"不覆盖"。
  const missing = state.environments
    .map((env) => env.rules)
    .filter((name) => name && !names.includes(name));
  const options = [...names, ...new Set(missing)];

  const key = JSON.stringify([options, state.confirm, [...pending]]);
  if (!force && key === rulesKey) return;
  rulesKey = key;

  const host = document.getElementById("rules-list");
  host.replaceChildren();

  if (!options.length) {
    host.appendChild(el("li", "empty-inline", "规则库是空的。下面导入一份：一行一条「IP 域名」。"));
  }

  for (const name of options) {
    const absent = !names.includes(name);
    const users = state.environments.filter((env) => env.rules === name).map((env) => env.name);
    const item = el("li", `rule-item${absent ? " is-missing" : ""}`);
    item.dataset.rule = name;
    item.appendChild(el("span", "rule-name mono", name));
    // 规模：不可见即不可信 —— 实测单份规则 536 条，列表上不写就只能逐个「载入」去数。
    // 取不到时留空而不是显示 0：0 条和「还没取到」是两件事。
    const stats = absent ? null : ruleStatsCache.get(name);
    if (stats) {
      item.appendChild(el("span", "rule-size mono", `${stats.entries} 条 · ${stats.ips} 个 IP`));
    } else if (!absent) {
      ensureRuleStats(name);
    }
    item.appendChild(
      el(
        "span",
        "rule-meta",
        absent
          ? "文件不存在"
          : users.length
            ? `被 ${users.join(" / ")} 绑定`
            : "未被绑定",
      ),
    );

    if (state.confirm === `rule:${name}`) {
      const box = el("div", "confirm");
      // 写明后果：删掉一份被绑定的规则，环境不会报错，而是**静默地不再覆盖** ——
      // 这正是最该在动手前说清的一类后果。
      box.appendChild(
        el(
          "span",
          "confirm-text",
          users.length
            ? `删除规则文件 ${name}？绑定它的 ${users.join(" / ")} 会失去覆盖，重启后按不覆盖运行。`
            : `删除规则文件 ${name}？它当前没有被任何环境绑定，文件本身会从磁盘删掉。`,
        ),
      );
      const key2 = `ruledelete:${name}`;
      box.appendChild(
        button({
          label: "确认删除",
          className: "btn danger sm",
          key: key2,
          onClick: () =>
            runAction(key2, async () => {
              state.confirm = null;
              return mutate(`/api/rules/${encodeURIComponent(name)}`, "DELETE");
            }),
        }),
      );
      box.appendChild(
        button({
          label: "取消",
          className: "btn ghost sm",
          onClick: () => {
            state.confirm = null;
            renderRules(true);
          },
        }),
      );
      item.appendChild(box);
    } else {
      const actions = el("div", "rule-actions");
      if (!absent) {
        actions.appendChild(
          button({ label: "载入", className: "btn sm", onClick: () => loadRule(name) }),
        );
      }
      actions.appendChild(
        button({
          label: "删除",
          icon: "trash",
          className: "btn sm danger",
          onClick: () => {
            state.confirm = `rule:${name}`;
            renderRules(true);
          },
        }),
      );
      item.appendChild(actions);
    }
    host.appendChild(item);
  }

  renderRuleOptions(options, new Set(missing));
}

let ruleOptionsKey = "";

function renderRuleOptions(options, missing) {
  const select = document.getElementById("rules-select");
  const key = JSON.stringify([options, [...missing]]);
  if (key === ruleOptionsKey) return;
  ruleOptionsKey = key;

  const previous = select.value;
  select.replaceChildren();
  const empty = el("option", null, "（不覆盖）");
  empty.value = "";
  select.appendChild(empty);
  for (const name of options) {
    const option = el("option", null, missing.has(name) ? `${name}（文件不存在）` : name);
    option.value = name;
    select.appendChild(option);
  }
  select.value = previous;
}

/// 「载入」：把已导入的规则原文取回表单，改完再导入（覆盖同名文件）。
async function loadRule(name) {
  try {
    const entry = await loadRuleText(name);
    if (entry.error) throw new Error(entry.error);
    const form = document.getElementById("rules-form");
    field(form, "name").value = name;
    field(form, "text").value = entry.text;
    document.getElementById("rules-hint").textContent =
      `已载入 ${name}（${entry.text.length} 字符）：改完点「导入」覆盖同名文件。`;
    setView("rules");
    field(form, "text").focus();
  } catch (error) {
    reportError(error);
  }
}

/// 规则原文缓存：只在一次渲染周期内复用，避免每秒重复拉同一份内容。
const ruleTextCache = new Map();

async function loadRuleText(name) {
  if (ruleTextCache.has(name)) return ruleTextCache.get(name);
  let entry;
  try {
    const data = await get(`/api/rules/${encodeURIComponent(name)}`);
    entry = { text: data.text };
  } catch (error) {
    entry = { error: error.message };
  }
  ruleTextCache.set(name, entry);
  return entry;
}

/// 规则文件的规模：条数（host 条目）与 IP 数。
/// 生成器在文件头写了 `# entries: N  ip: M`，优先读它（那是权威值）；万一没有
/// （手写或外部生成的文件），退回按数据行现算 —— 一行是 `ip host...`，
/// 所以条目数 = 每行除首个 token 外的 token 总数，IP 数 = 首个 token 去重。
function ruleStats(text) {
  if (typeof text !== "string" || !text) return null;
  const header = /^#\s*entries:\s*(\d+)\s+ip:\s*(\d+)/m.exec(text);
  if (header) return { entries: Number(header[1]), ips: Number(header[2]) };
  const ips = new Set();
  let entries = 0;
  for (const raw of text.split("\n")) {
    const row = raw.trim();
    if (!row || row.startsWith("#")) continue;
    const parts = row.split(/\s+/);
    ips.add(parts[0]);
    entries += Math.max(0, parts.length - 1);
  }
  return { entries, ips: ips.size };
}

/// name → { entries, ips } | null（null = 取不到）。列表要显示规模，
/// 而 `/api/rules` 只回名字，所以按需取一次正文并缓存；空库时不发请求。
const ruleStatsCache = new Map();
const ruleStatsPending = new Set();

function ensureRuleStats(name) {
  if (ruleStatsCache.has(name) || ruleStatsPending.has(name)) return;
  ruleStatsPending.add(name);
  loadRuleText(name)
    .then((entry) => ruleStatsCache.set(name, entry.error ? null : ruleStats(entry.text)))
    .catch(() => ruleStatsCache.set(name, null))
    .finally(() => {
      ruleStatsPending.delete(name);
      renderRules(true);
    });
}

/// 正文缓存与规模缓存是同一条数据的两种视图，清一个就必须清另一个，
/// 否则导入新内容后列表还挂着旧的条数。
function clearRuleCaches() {
  ruleTextCache.clear();
  ruleStatsCache.clear();
}

// --------------------------------------------------------------------------- //
// 动作
// --------------------------------------------------------------------------- //

/// 统一跑一个会改状态的请求：进行中禁用 + 出错提示 + 跑完重新对账。
async function runAction(key, mutateFn) {
  if (pending.has(key)) return;
  pending.add(key);
  renderSidebar(true);
  renderDetail(true);
  try {
    await mutateFn();
  } catch (error) {
    reportError(error);
  } finally {
    pending.delete(key);
    clearRuleCaches();
    try {
      await refreshAll();
    } catch (error) {
      reportError(error);
    }
  }
}

/// 复制成功后在按钮上给一个瞬时反馈（「已复制」+ 对勾）。
/// 只碰这一个按钮的局部 DOM：详情区每次快照都可能重画，整块重渲染会把反馈冲掉。
function flashCopied(node) {
  if (!node || node.dataset.copied === "1") return;
  node.dataset.copied = "1";
  const label = node.querySelector("span");
  const use = node.querySelector("svg.ic use");
  const originalLabel = label ? label.textContent : "";
  const originalIcon = use ? use.getAttribute("href") : "";
  if (label) label.textContent = "已复制";
  if (use) use.setAttribute("href", "#i-check");
  node.classList.add("is-copied");
  setTimeout(() => {
    delete node.dataset.copied;
    if (label && label.isConnected) label.textContent = originalLabel;
    if (use && use.isConnected) use.setAttribute("href", originalIcon);
    node.classList.remove("is-copied");
  }, 1400);
}

async function copyText(text, message, node) {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text);
    } else {
      // 回退路径：非安全上下文下 clipboard API 不可用（127.0.0.1 算安全上下文，
      // 但换成本机其它别名访问时不算）。
      const area = document.createElement("textarea");
      area.value = text;
      area.setAttribute("readonly", "");
      area.className = "sr-only";
      document.body.appendChild(area);
      area.select();
      const ok = document.execCommand("copy");
      area.remove();
      if (!ok) throw new Error("浏览器拒绝了剪贴板写入");
    }
    toast(message, "ok");
    flashCopied(node);
  } catch (error) {
    toast(`复制失败：${error.message}`, "bad");
  }
}

function selectEnvironment(name) {
  if (state.selected === name) {
    setView("environments");
    return;
  }
  state.selected = name;
  state.confirm = null;
  state.moreOpen = false;
  clearRuleCaches();
  setView("environments");
  renderSidebar(true);
  renderDetail(true);
  loadLogs(true);
}

// ---- 新建 / 编辑（同一个表单） ----

/// 进入编辑模式：把当前值填进表单，并在表单上记住**原名**。
///
/// 记住原名而不是新名：改名时请求要打在旧名字的 URL 上，改完才换成新名字。
function startEdit(env) {
  const form = document.getElementById("env-form");
  state.editing = env.name;
  form.dataset.editing = env.name;
  field(form, "name").value = env.name;
  field(form, "port").value = String(env.listen.port);
  field(form, "description").value = env.description || "";
  ensureRuleOption(env.rules);
  field(form, "rules").value = env.rules || "";
  // 放宽清单可以回显（它不是凭据），一行一个域名，与文本框的输入形态一致。
  field(form, "insecure_hosts").value = insecureHosts(env).join("\n");
  // 对外服务 = listen.host 是否非回环。凭据本身**永不回显** —— 服务端只回
  // `proxy_auth_enabled` 这个布尔，所以两栏一律清空：不填 = 保持原样，
  // 想清掉只能走下面的「清除代理鉴权」。
  field(form, "public").checked = !isLoopbackHost(env.listen.host);
  field(form, "proxy_user").value = "";
  field(form, "proxy_password").value = "";
  field(form, "proxy_clear").checked = false;
  document.getElementById("env-form-proxy-clear-field").classList.remove("is-hidden");
  document.getElementById("env-form-title").textContent = `编辑环境 · ${env.name}`;
  document.getElementById("env-form-submit-text").textContent = "保存";
  document.getElementById("env-form-cancel").hidden = false;
  const mode = document.getElementById("env-form-mode");
  mode.hidden = false;
  document.getElementById("env-form-mode-text").textContent = "编辑中";
  state.tab = "config";
  state.moreOpen = false;
  renderDetail(true);
  syncEditLock(env);
  syncAuthWarning();
  field(form, "name").focus();
}

/// 运行中只锁**停机字段**（名字 / 监听地址 / 代理凭据）—— 它们的改动会让运行中的实例与
/// 配置分叉，服务端以 `conflict` 拒绝。描述、规则绑定与放宽域名清单是**热**字段，
/// 运行中照样可改，所以这里不锁。状态变了要重新同步 —— 表单还开着的时候环境可能被停掉。
function syncEditLock(env) {
  const form = document.getElementById("env-form");
  const locked = env.health === "running";
  for (const name of STOP_REQUIRED_FIELDS) {
    const input = field(form, name);
    input.disabled = locked;
    input.title = locked ? "运行中不能改：先点「停止」" : "";
  }
  document.getElementById("env-form-hint").textContent = locked
    ? `编辑 ${env.name}：实例在运行。描述 / 规则绑定 / 放宽校验域名可以热改（保存后几秒内生效）；` +
      `名字 / 监听地址 / 代理鉴权是停机字段，要先停止。`
    : `编辑 ${env.name}：改完点「保存」。规则绑定、放宽校验域名与描述热生效；名字 / 监听地址 / 代理鉴权停机生效。`;
}

function ensureRuleOption(name) {
  if (!name) return;
  const select = document.getElementById("rules-select");
  if ([...select.options].some((option) => option.value === name)) return;
  const option = el("option", null, `${name}（文件不存在）`);
  option.value = name;
  select.appendChild(option);
}

function cancelEdit() {
  const form = document.getElementById("env-form");
  state.editing = null;
  form.dataset.editing = "";
  for (const name of STOP_REQUIRED_FIELDS) {
    const input = field(form, name);
    input.disabled = false;
    input.title = "";
    input.removeAttribute("aria-invalid");
  }
  form.reset();
  // 「清除代理鉴权」只在编辑模式有意义：新建时没有已保存的凭据可清。
  field(form, "proxy_clear").checked = false;
  document.getElementById("env-form-proxy-clear-field").classList.add("is-hidden");
  syncAuthWarning();
  document.getElementById("env-form-title").textContent = "新建环境";
  document.getElementById("env-form-submit-text").textContent = "创建";
  document.getElementById("env-form-cancel").hidden = true;
  document.getElementById("env-form-mode").hidden = true;
  document.getElementById("env-form-hint").textContent = "";
}

/// 把「放宽校验域名」文本框折成契约里的 `insecure_hosts` 数组。
///
/// 只做拆分与去空白：每行一个完整域名，空行忽略，顺序保持。
/// 合法性（通配符 / 域名形状 / 条数上限）由服务端裁决 —— 判定权在 core，
/// 前端不复制一份域名规则，只在服务端返回字段错误时把它标到这一栏。
/// `insecure_hosts` 是整体替换，所以这份数组**总是显式**发出去。
function parseInsecureHosts(text) {
  return String(text || "").split("\n").map((line) => line.trim()).filter(Boolean);
}

/// 表单 → 请求体。创建与编辑共用，差别只有三处（都写在下面，避免两份字段映射漂移）：
///
/// * 编辑时 `rules` **总是显式给值**（空选 = `null` = 不覆盖）：PATCH 里"没给这个字段"
///   才是"不动它"，所以想解绑就必须真的把 null 发出去；
/// * `insecure_hosts` **总是显式给值**（与 `rules` / 凭据同款理由：整体替换，
///   不显式给就永远删不掉一条）；
/// * 编辑时端口栏留空 = 保持当前端口（不写 `listen`），创建时留空 = 自动分配。
///
/// `listen` 是**整体替换**（契约如此），所以 host 与 port 要么都发、要么都不发：
/// 对外服务开关只切 host（`0.0.0.0` ↔ `127.0.0.1`），端口沿用输入框或当前值。
/// 创建时勾了对外服务却留空端口：自动分配是管理器的职责，契约里 `listen.port`
/// 必填，这里直接拦下并提示 —— 与其让服务端报错，不如当场说清。
function formPayload(form, editing) {
  const port = field(form, "port").value.trim();
  const rules = field(form, "rules").value;
  const publicBind = field(form, "public").checked;
  const host = publicBind ? "0.0.0.0" : "127.0.0.1";
  const proxyUser = field(form, "proxy_user").value;
  const proxyPassword = field(form, "proxy_password").value;
  const clearCredentials = Boolean(editing) && field(form, "proxy_clear").checked;
  const payload = {
    name: field(form, "name").value.trim(),
    description: field(form, "description").value,
    // 整体替换：总是显式给数组，空数组 = 全部恢复严格校验（与 rules / 凭据同款）。
    insecure_hosts: parseInsecureHosts(field(form, "insecure_hosts").value),
  };
  if (clearCredentials) {
    payload.proxy_user = null;
    payload.proxy_password = null;
  } else {
    // 只发操作员真的填了的键：凭据不回显，没填 = 保持原样。只填一边时照样发出去，
    // 让服务端按"同生共死"报缺失的那一边 —— 前端不复制这条校验。
    if (proxyUser) payload.proxy_user = proxyUser;
    if (proxyPassword) payload.proxy_password = proxyPassword;
  }
  if (editing) {
    payload.rules = rules || null;
    // host 没变且端口留空 → 不发 listen（免得"看起来改了其实只是原样重发"）
    const current = state.environments.find((env) => env.name === state.editing);
    const hostChanged = !current || current.listen.host !== host;
    if (hostChanged || port) {
      const effectivePort = port ? Number(port) : current ? current.listen.port : null;
      if (!effectivePort) throw handled(new Error("listen.port is required"));
      payload.listen = { host, port: effectivePort };
    }
  } else {
    if (rules) payload.rules = rules;
    if (publicBind && !port) {
      const portInput = field(form, "port");
      portInput.setAttribute("aria-invalid", "true");
      portInput.insertAdjacentElement(
        "afterend",
        el("span", "field-error", "勾选「对外服务」时端口不能留空：自动分配只支持默认监听 127.0.0.1，请显式填写端口。"),
      );
      portInput.focus();
      throw handled(new Error("listen.port is required"));
    }
    if (port) payload.listen = { host, port: Number(port) };
  }
  return payload;
}

/// 本地（客户端）校验失败的标记：错误已在表单字段下方就地呈现，提交处的 catch
/// 不要再弹一条英文 toast。
const handled = (error) => ((error.handled = true), error);

/// 回环判定：与服务的 is_loopback 对齐（前端只需要认 v4 回环与 ::1）。
function isLoopbackHost(host) {
  return host === "127.0.0.1" || host === "::1" || host === "localhost";
}

/// 对外服务与鉴权的联动警示：勾选 0.0.0.0 又没填凭据时给一行红字。
/// 不阻止提交（用户可能真要裸奔），但必须让"暴露面变大"这件事被看见。
function syncAuthWarning() {
  const form = document.getElementById("env-form");
  if (!form) return;
  const exposed = field(form, "public").checked;
  document
    .getElementById("env-form-auth-warning")
    .classList.toggle("is-hidden", !exposed || credentialsInForm());
}

/// 表单里此刻是否"有凭据"。凭据**永不回显**，所以只能这样推断：
/// 两栏填了任意一栏 = 有；否则编辑模式下已保存的凭据仍然生效（除非勾了「清除」）。
function credentialsInForm() {
  const form = document.getElementById("env-form");
  if (!form) return false;
  if (field(form, "proxy_user").value || field(form, "proxy_password").value) return true;
  if (field(form, "proxy_clear").checked) return false;
  const current = state.environments.find((env) => env.name === state.editing);
  return Boolean(current && current.proxy_auth_enabled);
}

/// 字段级错误：把出错的输入框标出来并聚焦。只丢一条 toast 的话，用户还得自己
/// 在一屏字段里找哪一个是它说的那个。
/// 字段级错误：描边变色 + 输入框下方一行原因。
/// 原来只有描边，原因只走 toast —— 而 toast 几秒后自己消失，用户回头看表单时
/// 已经不知道错在哪一项了（规范第 7 节「输入框」要求下方有 --bad 错误文本）。
function clearInvalid(form) {
  for (const node of form.querySelectorAll(".field-error")) node.remove();
  for (const node of form.querySelectorAll("[aria-invalid]")) node.removeAttribute("aria-invalid");
}

/// 服务端错误字段路径 → 表单控件名。路径一律以 `environment.` 开头。
///
/// * 列表项错误（`…insecure_hosts.3`）往上退一层，落到整块 textarea 上；
/// * 表里值为 `null` 的字段**已经没有控件**（已删除的 options 透传）：返回 null，
///   由 markInvalid 退成表单级错误 —— 静默丢弃会让提交看起来"什么也没发生"。
const FIELD_ALIASES = { "environment.options": null };

/// `environment.insecure_hosts.3` → `insecure_hosts`；`environment.proxy_user` → `proxy_user`。
function fieldNameForPath(path) {
  const text = String(path || "");
  if (Object.prototype.hasOwnProperty.call(FIELD_ALIASES, text)) return FIELD_ALIASES[text];
  const parts = text.split(".");
  if (parts.length && /^\d+$/.test(parts[parts.length - 1])) parts.pop();
  return parts[parts.length - 1] || null;
}

function markInvalid(form, path, message) {
  const name = fieldNameForPath(path);
  const input = name ? field(form, name) : null;
  if (!input) {
    // 没有对应控件（例如服务端仍在拒绝的非空 options）：退成表单级错误。
    const stale = form.querySelector(".form-error");
    if (stale) stale.remove();
    form.appendChild(el("p", "field-error form-error", message || "这一项不符合要求。"));
    return;
  }
  input.setAttribute("aria-invalid", "true");
  const previous = input.parentElement.querySelector(".field-error");
  if (previous) previous.remove();
  // 消息原文来自服务端错误码（判定权在 core，前端只翻译与呈现），
  // 放在字段下方比放 toast 更可行动：知道是哪一项、也知道原因。
  input.insertAdjacentElement("afterend", el("span", "field-error", message || "这一项不符合要求。"));
  input.focus();
}

// --------------------------------------------------------------------------- //
// 日志栏
// --------------------------------------------------------------------------- //

/// 事件类别。**不是按日志级别分类** —— 实例日志由 mitmdump 写出，实测形态是
///   [11:41:21.457][127.0.0.1:46306] server connect 127.0.0.1:17990
///   127.0.0.1:46306: GET http://127.0.0.1:17990/ HTTP/1.1
///        << HTTP/1.0 404 File not found 460b
/// 行首既没有 INFO/WARN，也没有级别列。所以过滤器按**真正可判别的维度**分：
/// 请求 / 响应 / 连接 / 运行 / 异常，这五类互斥且能覆盖全部行。
const LOG_KINDS = [
  { id: "all", label: "全部" },
  { id: "req", label: "请求" },
  { id: "res", label: "响应" },
  { id: "conn", label: "连接" },
  { id: "run", label: "运行" },
  { id: "err", label: "异常" },
];

const LOG_KIND_BADGE = { req: "REQ", res: "RES", conn: "CONN", run: "RUN", err: "ERR" };

const LOG_TS = /^\[(\d{2}:\d{2}:\d{2}(?:\.\d{1,3})?)\]/;
const LOG_PEER = /^\[([^\]]+)\]/;
/// 裸连接行形如 `127.0.0.1:46306: GET …` —— peer 用冒号而不是方括号。
const LOG_PEER_PREFIX = /^([A-Za-z0-9_.:\[\]-]+:\d+):\s+([\s\S]+)$/;
const LOG_METHOD = /^(GET|POST|PUT|DELETE|PATCH|HEAD|OPTIONS|CONNECT|PRI)\s+\S+/;
const LOG_CONN = /^(client|server)\s+(connect|disconnect)\b|^connection\s+(killed|closed|reset)/i;
const LOG_ERROR = /\b(error|failed|failure|exception|traceback|refused|timeout|unreachable)\b/i;
const LOG_RESPONSE = /^\s*<</;

function parseLogLine(line) {
  if (/^---\s*envboard:/.test(line)) return { kind: "mark", text: line };
  if (LOG_RESPONSE.test(line)) {
    const match = /<<\s*HTTP\/\d(?:\.\d)?\s+(\d{3})[\s\S]*/.exec(line);
    const status = match ? Number(match[1]) : null;
    return {
      kind: "res",
      text: match ? match[0].trim() : line.trim(),
      status,
      fail: status !== null && status >= 400,
      cont: true,
    };
  }

  let rest = line;
  let ts = null;
  let peer = null;
  const stamp = LOG_TS.exec(rest);
  if (stamp) {
    ts = stamp[1];
    rest = rest.slice(stamp[0].length);
  }
  const bracket = LOG_PEER.exec(rest);
  if (bracket) {
    peer = bracket[1];
    rest = rest.slice(bracket[0].length);
  }
  rest = rest.trim();
  if (!peer) {
    const prefix = LOG_PEER_PREFIX.exec(rest);
    if (prefix) {
      peer = prefix[1];
      rest = prefix[2].trim();
    }
  }

  if (LOG_METHOD.test(rest)) return { kind: "req", ts, peer, text: rest };
  if (LOG_CONN.test(rest)) return { kind: "conn", ts, peer, text: rest };
  if (LOG_ERROR.test(rest)) return { kind: "err", ts, peer, text: rest };
  // 剩下的：有 peer 的行是连接级输出（不带时间戳），没有 peer 的是 core 自己的运行输出。
  return { kind: peer ? "conn" : "run", ts, peer, text: rest };
}

let logFilterKey = "";

function renderLogFilters(counts, total) {
  const key = JSON.stringify([counts, total, state.logKind]);
  if (key === logFilterKey) return;
  logFilterKey = key;

  const host = document.getElementById("log-filters");
  host.replaceChildren();
  for (const kind of LOG_KINDS) {
    const count = kind.id === "all" ? total : counts[kind.id] || 0;
    const active = state.logKind === kind.id;
    const chip = el("button", `chip${active ? " is-active" : ""}${count === 0 && kind.id !== "all" ? " is-empty" : ""}`);
    chip.type = "button";
    chip.setAttribute("aria-pressed", String(active));
    chip.title = count === 0 && kind.id !== "all" ? `当前日志里没有「${kind.label}」行` : `${kind.label}：${count} 行`;
    chip.appendChild(el("span", null, kind.label));
    chip.appendChild(el("span", "chip-count", String(count)));
    chip.addEventListener("click", () => {
      state.logKind = kind.id;
      renderLogs(true);
    });
    host.appendChild(chip);
  }
}

function logLineNode(row, needle) {
  if (row.kind === "mark") return el("div", "log-line k-mark", row.text);
  const node = el(
    "div",
    `log-line k-${row.kind}${row.fail ? " is-fail" : ""}${row.cont ? " is-cont" : ""}`,
  );
  node.appendChild(el("span", "log-ts", row.ts || ""));
  node.appendChild(el("span", "log-peer", row.peer || ""));
  node.appendChild(el("span", "log-kind", LOG_KIND_BADGE[row.kind] || ""));
  node.appendChild(highlight(row.text, needle));
  return node;
}

/// 关键词高亮：用 <mark> 而不是改文本颜色 —— 命中片段要能在一行里被一眼定位。
function highlight(text, needle) {
  const span = el("span", "log-msg");
  if (!needle) {
    span.textContent = text;
    return span;
  }
  const haystack = text.toLowerCase();
  let index = 0;
  let hits = 0;
  for (;;) {
    const at = haystack.indexOf(needle, index);
    if (at === -1) break;
    if (at > index) span.appendChild(document.createTextNode(text.slice(index, at)));
    const mark = el("mark", null, text.slice(at, at + needle.length));
    span.appendChild(mark);
    index = at + needle.length;
    hits += 1;
  }
  if (!hits) {
    span.textContent = text;
    return span;
  }
  if (index < text.length) span.appendChild(document.createTextNode(text.slice(index)));
  return span;
}

/// 解析 + 过滤后的可见行。renderLogs 与「导出」共用，避免过滤条件在两处各写一遍
/// （写两遍就会出现「导出到的行和看到的不一样」这类偏差）。
function visibleLogRows() {
  const rows = state.logLines.map(parseLogLine);
  const needle = state.logSearch.trim().toLowerCase();
  const visible = rows.filter((row) => {
    if (row.kind === "mark") return true; // 分段标记是现场的时间锚点，任何过滤下都保留
    if (state.logKind !== "all" && row.kind !== state.logKind) return false;
    if (needle && !row.text.toLowerCase().includes(needle)) return false;
    return true;
  });
  return { rows, visible, needle };
}

function renderLogs(force) {
  const { rows, visible, needle } = visibleLogRows();
  const counts = { req: 0, res: 0, conn: 0, run: 0, err: 0 };
  let total = 0;
  for (const row of rows) {
    if (row.kind === "mark") continue;
    total += 1;
    if (counts[row.kind] !== undefined) counts[row.kind] += 1;
  }
  renderLogFilters(counts, total);

  document.getElementById("log-count").textContent =
    `${visible.filter((row) => row.kind !== "mark").length}/${total}`;

  const key = `${state.logKind}|${state.logSearch}|${state.logLines.join("\u0000")}`;
  if (!force && key === state.logRenderKey) return;
  state.logRenderKey = key;

  const view = document.getElementById("logs");
  const previousTop = view.scrollTop;
  const fragment = document.createDocumentFragment();
  if (!visible.length) {
    // 空态：区分"这个环境本来就没有日志"和"过滤后为空" —— 否则看起来像功能坏了
    fragment.appendChild(
      el(
        "div",
        "empty-inline",
        state.logLines.length
          ? "当前过滤条件下没有日志行。"
          : `还没有 ${state.logsEnv || "该环境"} 的日志。启动实例后这里会滚动出现。`,
      ),
    );
  }
  for (const row of visible) fragment.appendChild(logLineNode(row, needle));
  view.replaceChildren(fragment);
  applyAutoscroll(previousTop);
}

/// 导出当前视图（尊重事件类型过滤与关键词搜索）。文件名带环境名与时间戳。
function downloadLogs() {
  const { visible } = visibleLogRows();
  const rows = visible.filter((row) => row.kind !== "mark");
  if (!rows.length) {
    toast("当前视图没有可导出的日志行。", "info");
    return;
  }
  const body = rows
    .map((row) => [row.ts, row.peer, LOG_KIND_BADGE[row.kind], row.text].filter(Boolean).join(" "))
    .join("\n");
  // 文件名用**本地时间**：toISOString() 是 UTC，+08:00 的用户会看到刚导出的文件
  // 带着 8 小时前的"早晨"时间戳，一眼就以为导错了文件。
  const now = new Date();
  const pad = (value) => String(value).padStart(2, "0");
  const stamp =
    `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}` +
    `_${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
  const name = `${state.logsEnv || "envboard"}-${stamp}.log`;
  const url = URL.createObjectURL(new Blob([`${body}\n`], { type: "text/plain" }));
  const link = el("a");
  link.href = url;
  link.download = name;
  document.body.appendChild(link);
  link.click();
  link.remove();
  // blob URL 会一直持有这份内存直到被 revoke，别留到页面关闭
  setTimeout(() => URL.revokeObjectURL(url), 1000);
  toast(`已导出 ${rows.length} 行到 ${name}`, "ok");
}

/// 清空日志视图。**只清界面缓冲，不动磁盘上的日志文件** —— 所以必须同时暂停跟随：
/// 否则下一个快照（每秒一次）会把刚清掉的内容原样拉回来，按钮看起来像没生效。
function clearLogs() {
  state.logLines = [];
  state.logFollow = false;
  syncLogFollowButton();
  renderLogs(true);
  toast("已清空日志视图（磁盘日志文件不受影响）。跟随已暂停，点 ▶ 可恢复。", "info");
}

let lastProgrammaticTop = -1;

function applyAutoscroll(previousTop) {
  const view = document.getElementById("logs");
  const top = state.logAutoscroll
    ? view.scrollHeight - view.clientHeight
    : (previousTop ?? view.scrollTop);
  lastProgrammaticTop = top;
  view.scrollTop = top;
}

/// 读当前选中环境的日志尾部。失败自己吞掉：日志栏是旁路，它读不到不该把整次
/// `refresh()` 判成失败 —— 否则保存成功的那句"已保存。"会被这条错误顶掉，
/// 用户会以为自己的改动没生效（改名之后真的踩到过：面板还指着旧名字）。
async function loadLogs(force = false) {
  if (!state.logFollow && !force) return;
  const env = currentEnvironment();
  const tag = document.getElementById("logs-env");
  if (!env) {
    state.logsEnv = null;
    tag.textContent = "—";
    state.logLines = [];
    renderLogs(true);
    return;
  }
  state.logsEnv = env.name;
  tag.textContent = env.name;
  try {
    const data = await get(`/api/environments/${encodeURIComponent(env.name)}/logs?lines=200`);
    state.logLines = data.lines;
  } catch (error) {
    state.logLines = [`--- 读不到 ${env.name} 的日志：${error.message} ---`];
  }
  renderLogs(force);
}

function syncLogFollowButton() {
  const follow = document.getElementById("log-follow");
  follow.classList.toggle("is-active", state.logFollow);
  follow.setAttribute("aria-pressed", String(state.logFollow));
  follow.title = state.logFollow
    ? "暂停跟随：暂停后日志视图不再自动刷新，便于读现场"
    : "继续跟随：日志恢复自动刷新";
  setIcon(follow, state.logFollow ? "pause" : "play");
}

function syncAutoscrollButton() {
  const autoscroll = document.getElementById("log-autoscroll");
  autoscroll.classList.toggle("is-active", state.logAutoscroll);
  autoscroll.setAttribute("aria-pressed", String(state.logAutoscroll));
}

// --------------------------------------------------------------------------- //
// 视图切换
// --------------------------------------------------------------------------- //

function setView(view) {
  state.view = view;
  for (const node of document.querySelectorAll(".view-switch .seg-btn")) {
    const active = node.dataset.view === view;
    node.classList.toggle("is-active", active);
    node.setAttribute("aria-pressed", String(active));
  }
  document.getElementById("view-environments").classList.toggle("is-hidden", view !== "environments");
  document.getElementById("view-rules").classList.toggle("is-hidden", view !== "rules");
  document.getElementById("view-compare").classList.toggle("is-hidden", view !== "compare");
  // 规则数计数 chip 跟着视图亮起来（计数 chip 的激活态定义见 app.css 的 .count.is-active）
  document.getElementById("rules-count").classList.toggle("is-active", view === "rules");
}

// --------------------------------------------------------------------------- //
// 对账与刷新
// --------------------------------------------------------------------------- //

/// 选中项自愈：环境被改名或删除后，选中名会指向一个不存在的目标。
/// 不比渲染层兜底更晚 —— 在这里收敛，日志栏与详情区就不会各自去猜。
function reconcileSelection() {
  const list = state.environments;
  if (!list.length) {
    state.selected = null;
    return;
  }
  if (!state.selected || !list.some((env) => env.name === state.selected)) {
    state.selected = list[0].name;
    state.confirm = null;
    state.moreOpen = false;
  }
  if (state.editing && !list.some((env) => env.name === state.editing)) {
    cancelEdit();
    toast("正在编辑的环境已不存在，已退出编辑。", "bad");
  }
}

async function refreshAll({ force = false, spinner = false } = {}) {
  const button = document.getElementById("refresh");
  if (spinner) button.classList.add("is-busy");
  // 显式刷新 = 用户要的是"现在的事实"，规则原文缓存跟着失效（热重载改的就是它）。
  if (force) clearRuleCaches();
  try {
    const [status, environments, rules] = await Promise.all([
      get("/api/status"),
      get("/api/environments"),
      get("/api/rules"),
    ]);
    state.status = status;
    state.environments = environments;
    state.rules = rules.rules;
    state.streamOk = true;
    stampOk();
    reconcileSelection();
    renderChrome();
    renderSidebar(force);
    renderDetail(force);
    renderRules(force);
    await loadLogs(true);
  } finally {
    if (spinner) button.classList.remove("is-busy");
  }
}

/// SSE 每秒推的是环境快照，只有这一份在变；其余（status / rules）保持上一次的结果，
/// 免得每秒多做两次请求。
async function applySnapshot(environments) {
  state.environments = environments;
  state.streamOk = true;
  stampOk();
  reconcileSelection();
  renderChrome();
  renderSidebar(false);
  renderDetail(false);
  renderRules(false);
  await loadLogs(false);
}

// --------------------------------------------------------------------------- //
// 事件绑定
// --------------------------------------------------------------------------- //

function wire() {
  document.getElementById("refresh").addEventListener("click", () => {
    refreshAll({ force: true, spinner: true }).catch(reportError);
  });

  for (const node of document.querySelectorAll(".view-switch .seg-btn")) {
    node.addEventListener("click", () => setView(node.dataset.view));
  }

  document.getElementById("env-new").addEventListener("click", () => {
    cancelEdit();
    state.tab = "config";
    state.moreOpen = false;
    setView("environments");
    renderDetail(true);
    field(document.getElementById("env-form"), "name").focus();
  });
  document.getElementById("empty-new").addEventListener("click", () => {
    state.tab = "config";
    renderDetail(true);
    field(document.getElementById("env-form"), "name").focus();
  });

  for (const card of document.querySelectorAll(".stat-card")) {
    card.addEventListener("click", () => {
      const filter = card.dataset.filter;
      state.filter = state.filter === filter ? "all" : filter;
      syncFilterUi();
      renderSidebar(true);
    });
  }
  document.getElementById("filter-clear").addEventListener("click", () => {
    state.filter = "all";
    state.search = "";
    document.getElementById("env-search").value = "";
    document.getElementById("env-search-clear").classList.add("is-hidden");
    syncFilterUi();
    renderSidebar(true);
  });

  const envSearch = document.getElementById("env-search");
  const envSearchClear = document.getElementById("env-search-clear");
  envSearch.addEventListener("input", () => {
    state.search = envSearch.value;
    envSearchClear.classList.toggle("is-hidden", !envSearch.value);
    syncFilterUi();
    renderSidebar(true);
  });
  envSearchClear.addEventListener("click", () => {
    envSearch.value = "";
    state.search = "";
    envSearchClear.classList.add("is-hidden");
    syncFilterUi();
    renderSidebar(true);
    envSearch.focus();
  });

  for (const tab of document.querySelectorAll(".tab")) {
    tab.addEventListener("click", () => {
      state.tab = tab.dataset.tab;
      renderDetail(true);
    });
  }

  const copyCmd = document.getElementById("copy-cmd");
  copyCmd.addEventListener("click", () => {
    const env = currentEnvironment();
    if (env) copyText(env.proxy_command, "代理命令已复制到剪贴板", copyCmd);
  });

  document.getElementById("env-form-cancel").addEventListener("click", () => {
    cancelEdit();
    renderDetail(true);
  });

  // 对外服务 × 代理鉴权的联动警示：任一变化都要重估暴露面提示。
  for (const id of [
    "env-form-public",
    "env-form-proxy-user",
    "env-form-proxy-password",
    "env-form-proxy-clear",
  ]) {
    document.getElementById(id).addEventListener("change", syncAuthWarning);
    document.getElementById(id).addEventListener("input", syncAuthWarning);
  }

  document.getElementById("env-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = event.target;
    const editing = state.editing;
    const key = editing ? `save:${editing}` : "create";
    if (pending.has(key)) return;
    pending.add(key);
    const submit = document.getElementById("env-form-submit");
    submit.classList.add("is-busy");
    submit.disabled = true;
    clearInvalid(form);
    const hint = document.getElementById("env-form-hint");
    try {
      const payload = formPayload(form, editing);
      if (editing) {
        const saved = await mutate(environmentPath(editing), "PATCH", payload);
        cancelEdit();
        // 改名后把选中项搬到新名字：否则详情区会立刻回落到第一个环境，用户会以为改丢了。
        state.selected = saved && saved.name ? saved.name : editing;
        hint.textContent = "已保存。";
      } else {
        const created = await mutate("/api/environments", "POST", payload);
        form.reset();
        state.selected = created && created.name ? created.name : state.selected;
        hint.textContent = "已创建。";
      }
      await refreshAll({ force: true });
    } catch (error) {
      if (error.field) markInvalid(form, error.field, error.message);
      hint.textContent = "";
      if (!error.handled) reportError(error);
    } finally {
      pending.delete(key);
      submit.classList.remove("is-busy");
      submit.disabled = false;
    }
  });

  document.getElementById("compare-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const box = document.getElementById("compare-result");
    const domain = field(event.target, "host").value.trim();
    box.replaceChildren(el("p", "empty-inline", "正在查表…"));
    try {
      const data = await get(`/api/compare?host=${encodeURIComponent(domain)}`);
      const rows = data.environments || [];
      const covered = rows.filter((row) => row.covered).length;
      box.replaceChildren();
      box.appendChild(
        el(
          "p",
          "cmp-summary",
          rows.length
            ? `${domain}：${rows.length} 个环境里 ${covered} 个覆盖它`
            : "还没有环境，没有可对比的对象。",
        ),
      );
      for (const row of rows) {
        const line = el("div", `cmp-row ${row.covered ? "is-covered" : "is-uncovered"}`);
        line.dataset.env = row.env;
        line.appendChild(el("span", "cmp-env mono", row.env));
        line.appendChild(
          el(
            "span",
            "cmp-value mono",
            row.covered ? `→ ${row.ip}（端口 ${row.port}）` : `未覆盖（端口 ${row.port}）`,
          ),
        );
        box.appendChild(line);
      }
    } catch (error) {
      box.replaceChildren();
      reportError(error);
    }
  });

  document.getElementById("rules-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = event.target;
    const name = field(form, "name").value.trim();
    const key = `ruleimport:${name}`;
    if (pending.has(key)) return;
    const submit = form.querySelector("button[type=submit]");
    pending.add(key);
    submit.classList.add("is-busy");
    submit.disabled = true;
    try {
      await mutate("/api/rules", "POST", { name, text: field(form, "text").value });
      // 刻意**不**清空表单：规则大多是"载入 → 改几行 → 再导入"，清掉反而逼人重新载入。
      document.getElementById("rules-hint").textContent =
        `已导入 ${name}（覆盖同名文件）；绑定它的实例按 mtime 热重载。`;
      clearRuleCaches();
      await refreshAll({ force: true });
    } catch (error) {
      reportError(error);
    } finally {
      pending.delete(key);
      submit.classList.remove("is-busy");
      submit.disabled = false;
    }
  });

  // ---- 日志栏 ----
  document.getElementById("log-collapse").addEventListener("click", () => {
    state.logCollapsed = !state.logCollapsed;
    document.body.classList.toggle("log-collapsed", state.logCollapsed);
    const node = document.getElementById("log-collapse");
    node.setAttribute("aria-expanded", String(!state.logCollapsed));
    node.title = state.logCollapsed ? "展开日志栏" : "折叠 / 展开日志栏";
    if (!state.logCollapsed) applyAutoscroll(null);
  });

  document.getElementById("log-follow").addEventListener("click", () => {
    state.logFollow = !state.logFollow;
    syncLogFollowButton();
    if (state.logFollow) loadLogs(true);
  });

  document.getElementById("log-autoscroll").addEventListener("click", () => {
    state.logAutoscroll = !state.logAutoscroll;
    syncAutoscrollButton();
    if (state.logAutoscroll) applyAutoscroll(null);
  });

  document.getElementById("log-download").addEventListener("click", downloadLogs);
  document.getElementById("log-clear").addEventListener("click", clearLogs);

  const logSearch = document.getElementById("log-search");
  const logSearchClear = document.getElementById("log-search-clear");
  logSearch.addEventListener("input", () => {
    state.logSearch = logSearch.value;
    logSearchClear.classList.toggle("is-hidden", !logSearch.value);
    renderLogs(true);
  });
  logSearchClear.addEventListener("click", () => {
    logSearch.value = "";
    state.logSearch = "";
    logSearchClear.classList.add("is-hidden");
    renderLogs(true);
    logSearch.focus();
  });

  // 自动滚动跟随"用户是否在底部"：手动往上翻就停，翻回底部自动恢复。
  const view = document.getElementById("logs");
  view.addEventListener("scroll", () => {
    if (view.scrollTop === lastProgrammaticTop) return;
    const atBottom = view.scrollHeight - view.scrollTop - view.clientHeight <= 24;
    if (atBottom === state.logAutoscroll) return;
    state.logAutoscroll = atBottom;
    syncAutoscrollButton();
  });
}

function connectEvents() {
  // EventSource 带不了自定义头，token 只能走 URL（服务端对 ?token= 与 header 等价）。
  const source = new EventSource(
    state.token ? `/api/events?token=${encodeURIComponent(state.token)}` : "/api/events",
  );
  source.addEventListener("snapshot", (event) => {
    const payload = JSON.parse(event.data);
    if (!payload.ok) return;
    state.streamOk = true;
    // 日志面板跟着刷新：只在点"日志"时取过一次的话，之后永远停在那一次的快照上
    // （崩溃现场、热重载规则都看不到）。快照本身就是每秒一次，顺势读一次尾部即可。
    applySnapshot(payload.environments).catch(() => {});
  });
  source.addEventListener("error", () => {
    state.streamOk = false;
    renderChrome();
  });
}

/** 引导链的隔离壳：一个小组件坏掉，不该拖住整条链。
 *
 * 实测踩过一次：`renderLogFilters` 里一个写错的 id 让 `renderLogs` 抛空指针，
 * 而这个调用排在 `connectEvents()` / `refreshAll()` **之前** —— 于是数据请求
 * 根本没发出、页面停在空壳，`envboardReady` 永远停在 "pending"。
 * 验收只能看到"没就绪"，看不出坏在哪一步。
 *
 * 现在：坏掉的步骤记进 `dataset.envboardError`，ready 明确置成 "no"，
 * 失败自带定位信息 —— 而数据加载无论如何都会继续。
 */
const bootErrors = [];

function step(label, fn) {
  try {
    fn();
  } catch (error) {
    bootErrors.push(`${label}: ${error.message}`);
    reportError(error);
  }
}

// URL 携带的 token：捕获进 state 后立刻从地址栏抹掉 —— 浏览器历史、截图、复制
// 分享出去的链接里都不该留着凭据；之后所有请求照旧走 header（SSE 的 EventSource
// 带不了自定义头，服务端对 `?token=` 一视同仁，这里抹掉不影响已建立的连接）。
{
  const params = new URLSearchParams(location.search);
  if (params.has("token")) {
    const token = params.get("token").trim();
    if (token) state.token = token;
    params.delete("token");
    const rest = params.toString();
    history.replaceState(
      null,
      "",
      location.pathname + (rest ? `?${rest}` : "") + location.hash,
    );
  }
}

// 给浏览器验收用的显式标记：**JS 真的跑完了**才写它。
document.documentElement.dataset.envboardReady = "pending";

step("绑定事件", wire);
step("视图切换", () => setView("environments"));
step("环境筛选条", syncFilterUi);
step("日志跟随按钮", syncLogFollowButton);
step("自动滚动按钮", syncAutoscrollButton);
step("详情页签", renderTabs);
step("日志面板", () => renderLogs(true));
step("事件流", connectEvents);

refreshAll({ force: true, spinner: true })
  .then(() => {
    if (bootErrors.length) {
      document.documentElement.dataset.envboardError = bootErrors.join(" | ");
      document.documentElement.dataset.envboardReady = "no";
      return;
    }
    document.documentElement.dataset.envboardReady = "yes";
  })
  .catch((error) => {
    document.documentElement.dataset.envboardError = `首次加载: ${error.message}`;
    document.documentElement.dataset.envboardReady = "no";
    reportError(error);
  });

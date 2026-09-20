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
  view: "environments",
  tab: "overview",
  /// all | running | issues | rules
  filter: "all",
  search: "",
  // ---- 配置页签的常驻编辑器会话 ----
  /// 表单当前是否偏离基线（true = 「保存」解锁）。
  formDirty: false,
  /// 上次回显时的服务端值（editBaseline 的快照）。
  formBaseline: null,
  /// 最近一次成功拿到快照的本地时间（HH:MM:SS）。SSE 断线时界面照旧显示旧数据，
  /// 有了它用户至少能看出"这份数据有多旧"。
  lastOk: null,
  /// SSE 是否还活着 —— 连接徽章与侧栏底部圆点都看它。
  streamOk: false,
  // ---- 日志栏 ----
  logsEnv: null,
  trajEnv: null,
  trajRows: [],
  trajCursor: null,
  trajFollow: true,
  trajEs: null,
  trajKey: "",
  activity: [],
  logLines: [],
  logKind: "all",
  logSearch: "",
  logFollow: true,
  logAutoscroll: true,
  logRenderKey: "",
  // ---- 设置页 ----
  ca: null,
  // ---- 规则库侧栏搜索 ----
  ruleSearch: "",
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
/// 规范 6.5 节 的 2.5s 淡出用于 ok/info；错误提示停 6s —— 用户要去处理异常，
/// 2.5 秒根本读不完「原因 + 建议动作」。这条偏差记录在规范 7.3 节。
const TOAST_MS = { ok: 2500, info: 2500, bad: 6000 };

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

/// 侧栏页脚的事件流状态句（设计稿第 1 页：「事件流已连接 · 快照实时推送」）。
function streamFootText() {
  return state.streamOk
    ? "事件流已连接 · 快照实时推送"
    : `事件流断开 · ${exposureText()} · 正在轮询`;
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
    // 页脚按设计稿写成事件流状态句；暴露面与数据新鲜度在状态徽章与统计卡可见。
    setText(
      document.getElementById("sidebar-foot-text"),
      streamFootText(),
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
  // 下载链接随鉴权档位带上 token（与 SSE 同款：<a> 带不了自定义头）。
  const download = document.getElementById("ca-download");
  if (download) download.setAttribute("href", caUrl("/api/ca.pem"));
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
  setText(document.getElementById("rule-side-count"), String(visibleRuleSets().length));
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
  unhealthy: "　建议：打开「日志」页签看尾部输出，再决定重启还是先改规则。",
  config_mismatch: "　建议：停止后重新启动，让实例与实际配置对齐。",
  port_conflict: "　建议：用「更多操作 → 重分配端口」，或先停掉占用该端口的实例。",
  failed: "　建议：看日志尾巴上的报错，修掉后重新启动。",
};

let detailKey = "";

function renderDetail(force) {
  const env = currentEnvironment();
  const empty = document.getElementById("detail-empty");
  const detail = document.getElementById("detail");

  // 新建走独立弹窗（#modal-create，设计稿第 8 页）：详情区只在真的选中了
  // 一个环境时才出现，"新建模式下的详情面板"这一形态整个消失。
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
    state.tab,
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
  renderReason(env);
  renderOverview(env);
  renderBoundRules(env);
  renderTabs();
  if (state.tab === "config") fillEditForm(env);
  if (state.tab === "trajectory" && env) loadTrajectory(env);
  if (state.tab !== "trajectory") stopTrajectoryStream();
}

function renderDetailActions(env) {
  const host = document.getElementById("detail-actions");
  host.classList.remove("is-hidden"); // 新建模式曾把它藏起来：回到真实环境必须回来
  host.replaceChildren();

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

  // 删除进主动作行（用户裁决：动作行只留 停止 / 重启 / 删除 三键）。
  // 确认不再是就地展开 —— 规范 7.2.3 节：破坏性操作必须在模态窗内呈现，
  // 写明后果与不可逆性，确认按钮是显式的第二次点击。
  host.appendChild(
    button({
      label: "删除",
      icon: "trash",
      className: "btn danger",
      title: `删除环境 ${env.name}`,
      onClick: () =>
        openConfirm({
          title: `删除环境 ${env.name}？`,
          text:
            "实例会先被停掉，环境配置与它的日志文件随之删除；监听端口 " + env.listen.port + " 会被释放。" +
            "规则文件本身不受影响。指向该端口的客户端代理配置将立即失效，需要同步更新。",
          actionLabel: "确认删除",
          actionKey: `delete:${env.name}`,
          run: () => mutate(environmentPath(env.name), "DELETE"),
        }),
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

  // port_conflict 的解药（重分配端口）就放在原因条里：动作行只留三键
  // （停止 / 重启 / 删除），这是 port_conflict 下用户唯一还需要做的事。
  // 它会让客户端现有的 export https_proxy=… 立即失效，所以确认走破坏性模态
  // （规范 7.2.3 节：写明后果与不可逆性 + 显式第二次点击）。
  if (env.health === "port_conflict") {
    host.appendChild(
      button({
        label: "重分配端口",
        icon: "shuffle",
        className: "btn sm",
        onClick: () =>
          openConfirm({
            title: `重新分配 ${env.name} 的监听端口？`,
            text:
              "端口会变，客户端现有的 export https_proxy=… 将立即失效，需要同步更新。" +
              "旧端口会被释放，环境以新端口重新拉起。",
            actionLabel: "确认重分配",
            actionKey: `reallocate:${env.name}`,
            run: () => mutate(`${environmentPath(env.name)}/reallocate`, "POST"),
          }),
      }),
    );
  }
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

    // 规模与预览行数：预览只展开前 3 行（设计稿第 3 页），完整内容走「去规则库编辑」。
    const lines = entry.text.split("\n").filter((line) => line.trim());
    const stats = ruleStats(entry.text);
    const head = el("div", "bound-head");
    const tag = el("span", "badge purple");
    const tagDot = el("span", "dot");
    tagDot.setAttribute("aria-hidden", "true");
    tag.appendChild(tagDot);
    tag.appendChild(el("span", "mono", name));
    head.appendChild(tag);
    if (env.rules_missing) head.appendChild(el("span", "badge warn", "规则缺失（已忽略）"));
    head.appendChild(
      el(
        "span",
        "rule-meta",
        [
          stats ? `${stats.entries} 条 · ${stats.ips} 个 IP` : null,
          "热重载已启用",
        ]
          .filter(Boolean)
          .join(" · "),
      ),
    );
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
    const previewLines = lines.slice(0, 3);
    if (lines.length > 3) {
      previewLines.push(`… 其余 ${lines.length - 3} 条`);
    }
    const box = el("div", "rule-preview-box");
    const pre = el("pre", "rule-preview", previewLines.join("\n") || "（文件是空的）");
    box.appendChild(pre);
    host.replaceChildren(head, box);
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

  const key = JSON.stringify([options, [...pending]]);
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
      // 设计稿第 4 页的行内元信息：条数 · IP 数 · 落盘文件名（规则集名 + .hosts）。
      item.appendChild(
        el("span", "rule-size mono", `${stats.entries} 条 · ${stats.ips} 个 IP · ${name}.hosts`),
      );
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

    const actions = el("div", "rule-actions");
    if (!absent) {
      actions.appendChild(
        button({ label: "载入", className: "btn sm", onClick: () => loadRule(name) }),
      );
    }
    // 删除走破坏性确认模态（规范 7.2.3 节）。被绑定的规则删掉后环境**静默不再覆盖**，
    // 这正是最该在动手前说清的一类后果，所以引用环境要写进后果文本。
    actions.appendChild(
      button({
        label: "删除",
        icon: "trash",
        className: "btn sm danger",
        onClick: () =>
          openConfirm({
            title: `删除规则文件 ${name}？`,
            text: users.length
              ? "绑定它的 " + users.join(" / ") + " 不会报错，而是静默地不再覆盖任何域名；重启后按不覆盖运行。请先解绑或改绑。"
              : "它当前没有被任何环境绑定，文件本身会从磁盘删掉。",
            actionLabel: "确认删除",
            actionKey: `ruledelete:${name}`,
            run: () => mutate(`/api/rules/${encodeURIComponent(name)}`, "DELETE"),
          }),
      }),
    );
    item.appendChild(actions);
    host.appendChild(item);
  }

  renderRuleOptions(options, new Set(missing));
  renderRuleSidebar();
}

// ---- 侧栏「规则集」列表（规则库视图，设计稿第 4 页） ----

const visibleRuleSets = () => {
  const needle = state.ruleSearch.trim().toLowerCase();
  const names = state.rules;
  if (!needle) return names;
  return names.filter((name) => name.toLowerCase().includes(needle));
};

let ruleSideKey = "";

function renderRuleSidebar(force) {
  const names = visibleRuleSets();
  const key = JSON.stringify([names, state.ruleSearch, [...pending]]);
  if (!force && key === ruleSideKey) return;
  ruleSideKey = key;

  const host = document.getElementById("rule-side-list");
  host.replaceChildren();
  if (!state.rules.length) {
    host.appendChild(el("li", "empty-inline", "还没有规则集。导入一份 hosts 文本即可创建。"));
    return;
  }
  if (!names.length) {
    host.appendChild(el("li", "empty-inline", "没有符合条件的规则集。"));
    return;
  }
  for (const name of names) {
    const users = state.environments.filter((env) => env.rules === name);
    const item = el("li", "rule-side-item");
    item.dataset.rule = name;
    const main = el("button", "rule-side-main");
    main.type = "button";
    // 点侧栏规则集 = 载入到导入表单（与主列表「载入」同义，设计稿语义一致）。
    main.addEventListener("click", () => loadRule(name));
    main.appendChild(el("span", "rule-side-name mono", name));
    const stats = ruleStatsCache.get(name);
    main.appendChild(
      el("span", "rule-side-meta", stats ? `${stats.entries} 条 · ${name}.hosts` : name),
    );
    if (!stats && state.rules.includes(name)) ensureRuleStats(name);
    item.appendChild(main);
    // 「N 环境引用」徽标：删错一份被引用的规则是静默故障，引用数要一直可见。
    const badge = el("span", `badge ${users.length ? "purple" : ""}`.trim());
    badge.appendChild(el("span", null, `${users.length} 环境引用`));
    item.appendChild(badge);
    host.appendChild(item);
  }
}

let ruleOptionsKey = "";

function renderRuleOptions(options, missing) {
  const key = JSON.stringify([options, [...missing]]);
  if (key === ruleOptionsKey) return;
  ruleOptionsKey = key;

  // 两个下拉吃同一份选项：编辑表单（覆盖语义，首项 = 不覆盖）与
  // 创建弹窗（设计稿第 8 页：未选择 = 可稍后绑定）。
  const specs = [
    { id: "rules-select", emptyLabel: "（不覆盖）" },
    { id: "env-create-rules", emptyLabel: "未选择（可稍后绑定）" },
  ];
  for (const { id, emptyLabel } of specs) {
    const select = document.getElementById(id);
    if (!select) continue;
    const previous = select.value;
    select.replaceChildren();
    const empty = el("option", null, emptyLabel);
    empty.value = "";
    select.appendChild(empty);
    for (const name of options) {
      const option = el("option", null, missing.has(name) ? `${name}（文件不存在）` : name);
      option.value = name;
      select.appendChild(option);
    }
    select.value = previous;
  }
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

function selectEnvironment(name) {
  if (state.selected === name) {
    setView("environments");
    return;
  }
  state.selected = name;
  clearRuleCaches();
  setView("environments");
  renderSidebar(true);
  renderDetail(true);
  loadLogs(true);
}

// ---- 配置页签 = 常驻编辑器 ----
//
// 「编辑配置」入口已随更多操作一起退场：配置页签本身就是编辑器 —— 选中环境即
// 回显当前值，有改动才解锁「保存」。SSE 每秒推一次快照，所以回显必须幂等：
// 只有"换了环境 / 保存成功 / 服务端值变了"才重填，用户改到一半的内容绝不被冲掉。

/// 表单基线：上次回显时的服务端值。保存按钮按"当前输入 ≠ 基线"解锁。
function editBaseline(env) {
  return {
    name: env.name,
    port: String(env.listen.port),
    rules: env.rules || "",
    insecure_hosts: insecureHosts(env).join("\n"),
    public: !isLoopbackHost(env.listen.host),
    description: env.description || "",
  };
}

/// 回显指纹：环境的服务端值一变（保存成功 / 热字段被别处改掉）它就变，
/// 干净的表单跟着重填；脏表单不重填。
function editEchoKey(env) {
  return [
    env.name,
    env.listen.host,
    env.listen.port,
    env.rules || "",
    env.rules_count || 0,
    insecureHosts(env).join(","),
    env.description || "",
    env.health,
  ].join("|");
}

function fillEditForm(env) {
  const form = document.getElementById("env-form");
  const sameEnv = form.dataset.filledEnv === env.name;
  const echoKey = editEchoKey(env);

  if (sameEnv && state.formDirty) {
    // 改到一半：不回填（保住现场），但锁位 / 警示 / 保存按钮要跟最新状态走。
    syncEditLock(env);
    syncAuthWarning();
    updateSaveButton();
    return;
  }
  if (sameEnv && form.dataset.echoKey === echoKey) {
    // 数据没变：什么都不重填 —— 每秒一次的快照不许抖动表单。
    syncEditLock(env);
    updateSaveButton();
    return;
  }

  form.dataset.filledEnv = env.name;
  form.dataset.echoKey = echoKey;
  state.formDirty = false;
  field(form, "name").value = env.name;
  field(form, "port").value = String(env.listen.port);
  field(form, "description").value = env.description || "";
  ensureRuleOption(env.rules);
  field(form, "rules").value = env.rules || "";
  // 放宽清单可以回显（它不是凭据），一行一个域名，与文本框的输入形态一致。
  field(form, "insecure_hosts").value = insecureHosts(env).join("\n");
  // 对外服务 = listen.host 是否非回环。凭据本身**永不回显** —— 服务端只回
  // `proxy_auth_enabled` 这个布尔，所以两栏一律清空：不填 = 保持原样。
  field(form, "public").checked = !isLoopbackHost(env.listen.host);
  field(form, "proxy_user").value = "";
  field(form, "proxy_password").value = "";
  field(form, "proxy_clear").checked = false;
  // 「清除代理鉴权」只在确有已保存凭据时才有意义。
  document
    .getElementById("env-form-proxy-clear-field")
    .classList.toggle("is-hidden", !env.proxy_auth_enabled);
  document.getElementById("env-form-title").textContent = `编辑环境 · ${env.name}`;
  state.formBaseline = editBaseline(env);
  clearInvalid(form);
  syncEditLock(env);
  syncAuthWarning();
  updateSaveButton();
}

/// 表单此刻是否偏离基线。凭据两栏任何输入都算改动（它们不回显，
/// 非空即意图）；「清除代理鉴权」勾上同理。
function formIsDirty() {
  const baseline = state.formBaseline;
  if (!baseline) return false;
  const form = document.getElementById("env-form");
  return (
    field(form, "name").value.trim() !== baseline.name ||
    field(form, "port").value.trim() !== baseline.port ||
    field(form, "rules").value !== baseline.rules ||
    parseInsecureHosts(field(form, "insecure_hosts").value).join("\n") !==
      baseline.insecure_hosts ||
    field(form, "public").checked !== baseline.public ||
    field(form, "description").value !== baseline.description ||
    field(form, "proxy_user").value !== "" ||
    field(form, "proxy_password").value !== "" ||
    field(form, "proxy_clear").checked
  );
}

/// 保存按钮：无改动 = 禁用（用户裁决：监听表单改动）。
function updateSaveButton() {
  const submit = document.getElementById("env-form-submit");
  if (submit) submit.disabled = !state.formDirty;
}

/// 输入会话驱动：任何 input / change 都重估脏态。跑在 rAF 外、直接同步算 ——
/// 表单就十来个字段，比对成本可忽略，而提交瞬间需要的就是当下值。
function syncFormDirty() {
  if (state.tab !== "config" || !currentEnvironment()) return;
  state.formDirty = formIsDirty();
  updateSaveButton();
  syncAuthWarning();
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
  // 表单头部的状态位：运行中 = 停机字段被锁，要如实标注（而不是让人对着灰输入框猜）。
  const mode = document.getElementById("env-form-mode");
  if (mode) {
    mode.hidden = !locked;
    document.getElementById("env-form-mode-text").textContent = "运行中";
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

// ---- 创建环境弹窗（设计稿第 8 页） ----// --------------------------------------------------------------------------- //
// 模态窗控制器（规范 6.6.4 节 / 6.6.8 节）
// --------------------------------------------------------------------------- //
//
// 三个模态（创建环境 / 二维码 / 破坏性确认）共用一套：焦点陷阱、ESC 策略、
// 遮罩点击策略、焦点归还。data-modal-state 是自动化断言锚点：open / validating / submitting。

const modalTriggers = new Map(); // modal id -> 触发元素（关闭后焦点归还）
const MODAL_FOCUSABLE =
  "a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled])";

function activeModal() {
  return document.querySelector(".modal-overlay:not(.is-hidden)");
}

function openModal(id, trigger, focusSelector) {
  const modal = document.getElementById(id);
  if (!modal) return;
  if (trigger && trigger.focus) modalTriggers.set(id, trigger);
  modal.classList.remove("is-hidden");
  modal.dataset.modalState = "open";
  const target =
    (focusSelector && modal.querySelector(focusSelector)) || modal.querySelector(MODAL_FOCUSABLE);
  if (target) target.focus();
}

/// force=true = 用户显式点取消/关闭：submitting 中也放行（规范：取消与关闭保持可点，
/// 已发出的请求不因关闭面板而撤销）。ESC 不 force —— submitting 时忽略 ESC，
/// 避免关掉一个结果未知的弹窗。
function closeModal(id, { force = false } = {}) {
  const modal = document.getElementById(id);
  if (!modal || modal.classList.contains("is-hidden")) return;
  if (!force && modal.dataset.modalState === "submitting") return;
  modal.classList.add("is-hidden");
  modal.dataset.modalState = "";
  const trigger = modalTriggers.get(id);
  modalTriggers.delete(id);
  if (trigger && trigger.isConnected && trigger.focus) trigger.focus();
}

function wireModalChrome() {
  // 全局键盘：ESC 关闭 + Tab 焦点陷阱，只作用于当前打开的模态。
  document.addEventListener("keydown", (event) => {
    const modal = activeModal();
    if (!modal) return;
    if (event.key === "Escape") {
      event.preventDefault();
      closeModal(modal.id);
      return;
    }
    if (event.key !== "Tab") return;
    const focusables = [...modal.querySelectorAll(MODAL_FOCUSABLE)];
    if (!focusables.length) return;
    const first = focusables[0];
    const last = focusables[focusables.length - 1];
    const active = document.activeElement;
    if (event.shiftKey && (active === first || !modal.contains(active))) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (active === last || !modal.contains(active))) {
      event.preventDefault();
      first.focus();
    }
  });

  // 遮罩点击策略（规范 6.6.4 节）：表单型（创建环境）点击遮罩不关闭 ——
  // 防止未保存输入被误丢；确认 / 二维码不是表单，点遮罩 = 取消。
  for (const overlay of document.querySelectorAll(".modal-overlay")) {
    overlay.addEventListener("mousedown", (event) => {
      if (event.target !== overlay) return;
      if (overlay.dataset.modalMode === "env") return;
      closeModal(overlay.id);
    });
  }
}

// ---- 破坏性操作确认模态（规范 7.2.3 节）----
//
// 删除环境 / 删除规则 / 重分配端口都走这里：触发按钮是第一击，模态内「确认」
// 是显式第二击；正文写明后果与不可逆性。旧的「就地展开确认」整体退场。

let confirmJob = null;

function openConfirm({ title, text, actionLabel, actionKey, run }) {
  document.getElementById("confirm-title").textContent = title;
  document.getElementById("confirm-text").textContent = text;
  document.getElementById("confirm-go-text").textContent = actionLabel;
  confirmJob = { key: actionKey, run };
  // 焦点落在「取消」而不是「确认」：默认动作不该是破坏性的。
  openModal("modal-confirm", document.activeElement, "#confirm-cancel");
}

function runConfirm() {
  if (!confirmJob) return;
  const job = confirmJob;
  confirmJob = null;
  closeModal("modal-confirm", { force: true });
  runAction(job.key, job.run);
}


//
// 新建与编辑从此分家：弹窗只收四个字段（名称 / 描述 / 端口 / 规则集），
// 不含 0.0.0.0 对外绑定与代理鉴权 —— 那些是"把环境跑给别人用"的进阶配置，
// 放进编辑表单（配置页签）里慢慢配，别让第一次创建就被表单淹没。
function openCreateModal() {
  const form = document.getElementById("env-create-form");
  form.reset();
  clearInvalid(form);
  // 打开时焦点落到首个字段（规范 6.6.4），而不是标题栏的关闭按钮。
  openModal("modal-create", document.activeElement, "#env-create-form input[name=name]");
}

function closeCreateModal() {
  // 取消与关闭保持可点（规范 6.6.6 节）：submitting 中也 force。
  closeModal("modal-create", { force: true });
}

/// 校验文案（规范 6.6.5 节）：前半句给原因、后半句给建议动作 —— 满足异常三通道。
function validateCreateField(form, name) {
  if (name === "name") {
    const value = field(form, "name").value.trim();
    if (!value) return "名称为必填项。请填写后提交。";
    if (!/^[a-z][a-z0-9_\-]*$/.test(value)) return "名称含非法字符（仅允许字母、数字、-、_）。请修改后重试。";
    return null;
  }
  if (name === "port") {
    const value = field(form, "port").value.trim();
    if (value && (!/^\d+$/.test(value) || Number(value) < 1024 || Number(value) > 65535))
      return "端口需在 1024–65535。请修正或留空自动分配。";
    return null;
  }
  return null;
}

/// 提交时全量校验：标红全部错误字段，但焦点只落在**第一个**错误字段上（规范 6.6.4 节）。
function validateCreate(form) {
  clearInvalid(form);
  let firstBad = null;
  for (const name of ["name", "port"]) {
    const message = validateCreateField(form, name);
    if (message) {
      markInvalid(form, `environment.${name}`, message, { noFocus: true });
      if (!firstBad) firstBad = name;
    }
  }
  if (firstBad) field(form, firstBad).focus();
  return !firstBad;
}

async function submitCreate(form) {
  const key = "create";
  if (pending.has(key)) return;
  const modal = document.getElementById("modal-create");
  if (!validateCreate(form)) {
    modal.dataset.modalState = "validating";
    return;
  }
  // submitting（规范 6.6.6 节）：确认按钮 loading +「创建中…」、字段只读、
  // 取消与关闭仍可点、ESC 被模态控制器忽略。
  modal.dataset.modalState = "submitting";
  pending.add(key);
  const submit = document.getElementById("create-submit");
  const submitLabel = submit.querySelector("span");
  submit.classList.add("is-busy");
  submit.disabled = true;
  if (submitLabel) submitLabel.textContent = "创建中…";
  const controls = [...form.elements].filter((node) => node.name);
  for (const control of controls) control.disabled = true;
  try {
    const payload = { name: field(form, "name").value.trim(), description: field(form, "description").value };
    const port = field(form, "port").value.trim();
    if (port) payload.listen = { host: "127.0.0.1", port: Number(port) };
    const rules = field(form, "rules").value;
    if (rules) payload.rules = rules;
    const created = await mutate("/api/environments", "POST", payload);
    closeModal("modal-create", { force: true });
    state.selected = created && created.name ? created.name : state.selected;
    await refreshAll({ force: true });
    toast(`已创建 ${state.selected}。`, "ok");
  } catch (error) {
    // 服务端错误映射回对应字段，不弹全局错误（规范 6.6.5 节）：
    // 重名（409）落回名称栏，用规范文案而不是透传英文。
    if (/already exists/.test(String(error.message || ""))) {
      markInvalid(form, "environment.name", "已存在同名环境。请更换名称或查看已有环境。");
    } else if (error.field) {
      markInvalid(form, error.field, error.message);
    } else if (!error.handled) {
      reportError(error);
    }
  } finally {
    pending.delete(key);
    for (const control of controls) control.disabled = false;
    submit.classList.remove("is-busy");
    submit.disabled = false;
    if (submitLabel) submitLabel.textContent = "创建环境";
    if (!modal.classList.contains("is-hidden")) modal.dataset.modalState = "open";
  }
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

/// 表单 → PATCH 请求体（配置页签的常驻编辑器只服务 PATCH；创建走弹窗）。
///
/// 三处语义（都写在下面，避免字段映射漂移）：
///
/// * `rules` **总是显式给值**（空选 = `null` = 不覆盖）：PATCH 里"没给这个字段"
///   才是"不动它"，所以想解绑就必须真的把 null 发出去；
/// * `insecure_hosts` **总是显式给值**（同款理由：整体替换，不显式给就永远删不掉一条）；
/// * `listen` 是**整体替换**，host 与 port 要么都发、要么都不发：host 没变且
///   端口没改 → 不发 listen（免得"看起来改了其实只是原样重发"）；
///   对外服务开关只切 host（`0.0.0.0` ↔ `127.0.0.1`）。
function formPayload(form, env) {
  const port = field(form, "port").value.trim();
  const rules = field(form, "rules").value;
  const publicBind = field(form, "public").checked;
  const host = publicBind ? "0.0.0.0" : "127.0.0.1";
  const proxyUser = field(form, "proxy_user").value;
  const proxyPassword = field(form, "proxy_password").value;
  const clearCredentials = field(form, "proxy_clear").checked;
  const payload = {
    name: field(form, "name").value.trim(),
    description: field(form, "description").value,
    // 整体替换：总是显式给数组，空数组 = 全部恢复严格校验（与 rules / 凭据同款）。
    insecure_hosts: parseInsecureHosts(field(form, "insecure_hosts").value),
    rules: rules || null,
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
  // host 没变且端口没改 → 不发 listen（免得"看起来改了其实只是原样重发"）。
  const hostChanged = !env || env.listen.host !== host;
  const portChanged = Boolean(port) && Number(port) !== (env ? env.listen.port : null);
  if (hostChanged || portChanged) {
    const effectivePort = port ? Number(port) : env ? env.listen.port : null;
    if (!effectivePort) throw handled(new Error("listen.port is required"));
    payload.listen = { host, port: effectivePort };
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
/// 两栏填了任意一栏 = 有；否则当前环境已保存的凭据仍然生效（除非勾了「清除」）。
function credentialsInForm() {
  const form = document.getElementById("env-form");
  if (!form) return false;
  if (field(form, "proxy_user").value || field(form, "proxy_password").value) return true;
  if (field(form, "proxy_clear").checked) return false;
  const current = currentEnvironment();
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

function markInvalid(form, path, message, opts = {}) {
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
  const err = el("span", "field-error", message || "这一项不符合要求。");
  err.setAttribute("data-error", name || "form"); // 规范 6.6.8 节：错误容器可断言
  input.insertAdjacentElement("afterend", err);
  if (!opts.noFocus) input.focus(); // 失焦校验不能再把焦点抢回输入框
}

// --------------------------------------------------------------------------- //
// 日志栏
// --------------------------------------------------------------------------- //

/// 事件类别。**不是按日志级别分类** —— 实例日志由 request-log 插件写出，形态是
///   [11:41:21.457] GET 127.0.0.1:17990/ -> 404 (req 0B resp 46B 3ms)
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

// ---- 轨迹页签：数据面请求事件的实时时间线 ---- //

/// 载入并跟随某环境的请求轨迹：先拉一次尾部窗口，再开 SSE 增量跟随。
/// 环境切换或页签离开时由调用方停流（stopTrajectoryStream）。
async function loadTrajectory(env) {
  if (state.trajEnv !== env.name) {
    stopTrajectoryStream();
    state.trajEnv = env.name;
    state.trajRows = [];
    state.trajCursor = null;
    state.trajKey = "";
    await fetchTrajectoryPage(env.name);
    renderTrajectory(true);
  }
  startTrajectoryStream(env.name);
}

async function exportCapture() {
  const env = state.trajEnv || currentEnvironment()?.name;
  if (!env) return;
  try {
    const response = await fetch(
      `${environmentPath(env)}/captures/export?format=har`,
      { headers: headers(false) },
    );
    if (!response.ok) throw await response.json();
    const blob = await response.blob();
    const url = URL.createObjectURL(blob);
    const link = el("a");
    link.href = url;
    link.download = `${env}-captures.har`;
    link.click();
    URL.revokeObjectURL(url);
    toast("抓包会话已导出为 HAR。", "ok");
  } catch (error) {
    reportError(error);
  }
}

async function fetchTrajectoryPage(name) {
  try {
    const data = await get(`${environmentPath(name)}/trajectory?limit=300`);
    state.trajRows = (data.events || []).map(rowFromEnvelope);
    state.trajCursor = state.trajRows.length
      ? state.trajRows[state.trajRows.length - 1].seq
      : 0;
  } catch (error) {
    reportError(error);
  }
}

/// SSE：连接即发 baseline（尾部窗口 + cursor），此后 events 增量。
/// 断线后 EventSource 自动重连；cursor 随 URL 重置，续传交给服务端。
function startTrajectoryStream(name) {
  if (state.trajEs && state.trajEnv === name) return;
  stopTrajectoryStream();
  const stream = new EventSource(
    `${environmentPath(name)}/trajectory/stream?cursor=${state.trajCursor ?? 0}`,
  );
  state.trajEs = stream;
  const ingest = (eventList) => {
    if (!Array.isArray(eventList)) return;
    for (const item of eventList) {
      const row = rowFromEnvelope(item);
      state.trajRows.push(row);
    }
    if (state.trajFollow) renderTrajectory(false);
  };
  stream.addEventListener("baseline", (event) => {
    // 重连/换流都以 baseline 为准替换视图内容（不叠加）。
    try {
      const payload = JSON.parse(event.data);
      state.trajRows = (payload.events || []).map(rowFromEnvelope);
      state.trajKey = "";
      renderTrajectory(true);
    } catch { /* 畸形帧忽略 */ }
  });
  stream.addEventListener("events", (event) => {
    try {
      const payload = JSON.parse(event.data);
      if (typeof payload.cursor === "number") state.trajCursor = payload.cursor;
      ingest(payload.events);
    } catch { /* 畸形帧忽略 */ }
  });
  stream.addEventListener("error", () => {
    // EventSource 会自动重连；这里只标记连接态（页脚的状态灯由 snapshot 流负责）。
  });
}

function stopTrajectoryStream() {
  if (state.trajEs) {
    state.trajEs.close();
    state.trajEs = null;
  }
}

const TRAJ_DOT = {
  "request/start": "▶",
  "request/upstream": "→",
  "request/body": "·",
  "response/head": "◀",
  "request/end": "■",
  custom: "◇",
};

function rowFromEnvelope(envelope) {
  const data = envelope.data || {};
  const kind = envelope.type || "custom";
  let text;
  switch (kind) {
    case "request/start":
      text = `${data.method} ${data.authority}${data.path}（sni=${data.sni ?? "—"}${data.insecure ? "，放宽校验" : ""}）`;
      break;
    case "request/upstream":
      text = `上连 ${data.resolved_addr}${data.rewritten ? "（hosts 改写）" : ""}`;
      break;
    case "request/body":
      text = `请求体 ${data.bytes} B`;
      break;
    case "response/head":
      text = `响应 ${data.status}（${data.bytes} B）`;
      break;
    case "request/end":
      text = data.error ? `结束：${data.error}` : `完成，${data.duration_ms} ms`;
      break;
    default:
      text = kind === "custom" ? `${data.kind} ${JSON.stringify(data.payload ?? {})}` : JSON.stringify(data);
  }
  return {
    seq: envelope.seq,
    time: envelope.time,
    requestId: data.request_id,
    kind,
    dot: TRAJ_DOT[kind] || "◇",
    text,
  };
}

// ---- 调试页：抓包会话（工作区级单例）+ 导入会话（HAR，多个并存） ---- //

function renderDebugView() {
  const select = document.getElementById("debug-env");
  const candidates = state.environments.filter((env) => env.health === "running");
  select.replaceChildren(
    ...candidates.map((env) => {
      const option = document.createElement("option");
      option.value = env.name;
      option.textContent = `${env.name} (${env.listen.host}:${env.listen.port})`;
      return option;
    }),
  );
}

async function refreshDebug() {
  if (state.view !== "debug") return;
  renderDebugView();
  try {
    const view = await get("/api/debug?limit=500");
    const bar = document.getElementById("debug-count");
    if (!view.env) {
      bar.textContent = "当前没有调试会话 —— 选一个运行中的环境开启。";
      document.getElementById("debug-records").replaceChildren();
      return;
    }
    const dropped = view.capture.dropped
      ? `，已淘汰 ${view.capture.dropped} 条（更早记录已被挤出）`
      : "";
    bar.textContent =
      `抓包会话 #${view.capture.session.id} · ${view.env} · gen ${view.capture.session.generation} · ` +
      `已捕获 ${view.capture.captured} 条${dropped}`;
    if (view.capture.session) selectDebugTarget(view.env);
    renderDebugRecords(view.capture.records || []);
  } catch { /* 无会话 */ }
}

function selectDebugTarget(env) {
  const select = document.getElementById("debug-env");
  for (const option of select.options) option.selected = option.value === env;
}

async function renderDebugRecordDetail(requestId) {
  const view = await get("/api/debug?limit=500");
  const record = (view.capture?.records || []).find((r) => r.request_id === requestId);
  const panel = document.getElementById("debug-records");
  if (record) {
    panel.replaceChildren(el("pre", "traj-row mono", JSON.stringify(record, null, 2)));
  } else {
    panel.replaceChildren(el("div", "empty-inline", `#${requestId} 的记录已被淘汰。`));
  }
}

function renderDebugRecords(records) {
  const view = document.getElementById("debug-records");
  if (!records.length) {
    view.replaceChildren(el("div", "empty-inline", "还没有抓包记录。请求经过目标环境后这里会出现。"));
    return;
  }
  const fragment = document.createDocumentFragment();
  for (const record of records) {
    const request = record.request || {};
    const response = record.response || {};
    const node = el("div", "traj-row");
    node.style.borderLeftColor = groupColor(record.request_id ?? 0);
    node.style.cursor = "pointer";
    node.title = "点击查看请求/响应详情";
    node.textContent =
      `${request.method} ${request.authority}${request.path} → ${response.status}` +
      ` (#${record.request_id}${response.body?.omitted ? "，正文超限未记" : ""})`;
    node.addEventListener("click", () => {
      renderDebugRecordDetail(record.request_id).catch(reportError);
    });
    fragment.appendChild(node);
  }
  view.replaceChildren(fragment);
}

// ---- 导入会话（HAR） ---- //

async function refreshHarList() {
  const data = await get("/api/har");
  const list = document.getElementById("har-list");
  const count = document.getElementById("har-count");
  const sessions = data.sessions || [];
  count.textContent = `${sessions.length} 个导入会话`;
  if (!sessions.length) {
    list.replaceChildren(el("div", "empty-inline", "还没有导入会话。选择一个 HAR 文件导入（可同时打开多个，只读）。"));
    return;
  }
  const fragment = document.createDocumentFragment();
  for (const session of sessions) {
    const node = el("div", "traj-row");
    node.style.borderLeftColor = groupColor(session.id);
    node.style.cursor = "pointer";
    node.title = "点击查看条目";
    node.textContent = `#${session.id} ${session.name} · ${session.entries} 条`;
    const del = el("button", "btn icon-btn sm", "×");
    del.setAttribute("aria-label", `删除会话 ${session.name}`);
    del.addEventListener("click", (event) => {
      event.stopPropagation();
      mutate(`/api/har/${session.id}`, "DELETE")
        .then(() => refreshHarList())
        .catch(reportError);
    });
    node.appendChild(del);
    node.addEventListener("click", () => {
      get(`/api/har/${session.id}?limit=500`).then((view) => {
        const panel = document.getElementById("har-records");
        panel.hidden = false;
        panel.textContent = JSON.stringify(view.records || [], null, 2);
      }).catch(reportError);
    });
    fragment.appendChild(node);
  }
  list.replaceChildren(fragment);
}

/// 渲染轨迹时间线：同一 request_id 的事件用同色左边线归组，一眼可读。
function renderTrajectory(force) {
  const view = document.getElementById("trajectory");
  const count = document.getElementById("traj-count");
  const key = `${state.trajEnv}|${state.trajRows.length}|${state.trajRows.at(-1)?.seq ?? 0}`;
  if (!force && key === state.trajKey) return;
  state.trajKey = key;
  if (count) count.textContent = `${state.trajRows.length} 条事件 · cursor ${state.trajCursor ?? "—"}`;
  if (!state.trajRows.length) {
    view.replaceChildren(
      el("div", "empty-inline", `还没有 ${state.trajEnv || "该环境"} 的请求轨迹。发一个经过代理的请求，这里会实时出现。`),
    );
    return;
  }
  const fragment = document.createDocumentFragment();
  for (const row of state.trajRows) {
    const node = el("div", "traj-row");
    node.dataset.requestId = row.requestId ?? "";
    node.style.borderLeftColor = groupColor(row.requestId ?? 0);
    const time = new Date(row.time).toLocaleTimeString();
    node.textContent = `${time} #${row.requestId ?? "—"} ${row.dot} ${row.text}`;
    fragment.appendChild(node);
  }
  const previousTop = view.scrollTop;
  const stick = view.scrollHeight - view.scrollTop - view.clientHeight < 40;
  view.replaceChildren(fragment);
  if (stick) view.scrollTop = view.scrollHeight;
  else view.scrollTop = previousTop;
}

/// request_id → 稳定颜色（8 色循环；确定性关联，不猜最近一个未完成的）。
function groupColor(id) {
  const palette = ["#2a5fe8", "#0f8a5f", "#b3661a", "#8a2a8a", "#b31a1a", "#0f7a8a", "#5a5a5a", "#8a6a0f"];
  return palette[id % palette.length] || "#5a5a5a";
}

// ---- 活动视图：控制面审计事件 ---- //

async function loadActivity() {
  try {
    const data = await get("/api/history?limit=300");
    state.activity = (data.events || []).map(activityRow);
    renderActivity();
  } catch (error) {
    reportError(error);
  }
}

function activityRow(envelope) {
  const data = envelope.data || {};
  const kind = envelope.type || "custom";
  const who = data.name || data.rules_name || "";
  let text;
  switch (kind) {
    case "environment/created": text = `创建环境（监听 ${data.listen}）`; break;
    case "environment/updated": text = `更新字段：${(data.fields || []).join(", ")}`; break;
    case "environment/deleted": text = "删除环境"; break;
    case "rules/imported": text = `导入规则 ${data.rules_name}（sha ${String(data.rules_sha256 || "").slice(0, 12)}…）`; break;
    case "rules/deleted": text = `删除规则 ${data.rules_name}`; break;
    case "engine/applied": text = `热应用配置 ${String(data.config_hash || "").slice(0, 12)}…`; break;
    case "engine/rejected": text = `配置被拒（旧快照继续服务）：${data.reason}`; break;
    case "instance/started": text = "实例启动"; break;
    case "instance/stopped": text = "实例停止"; break;
    case "instance/reconciled": text = `reconcile：${data.from} → ${data.to}`; break;
    default: text = kind === "custom" ? `${data.kind} ${JSON.stringify(data.payload ?? {})}` : JSON.stringify(data);
  }
  return { seq: envelope.seq, time: envelope.time, kind, who, text };
}

function renderActivity() {
  const view = document.getElementById("activity");
  const count = document.getElementById("activity-count");
  if (count) count.textContent = `${state.activity.length} 条事件`;
  if (!state.activity.length) {
    view.replaceChildren(el("div", "empty-inline", "还没有控制面事件。新建一个环境，这里就会出现第一条记录。"));
    return;
  }
  const fragment = document.createDocumentFragment();
  for (const row of [...state.activity].reverse()) {
    const node = el("div", "traj-row");
    node.style.borderLeftColor = groupColor(row.kind.length);
    const time = new Date(row.time).toLocaleTimeString();
    node.textContent = `${time} ${row.who ? `${row.who}: ` : ""}${row.text}`;
    fragment.appendChild(node);
  }
  view.replaceChildren(fragment);
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
  // 设计稿 v3：主导航在侧栏（.nav-btn）。视图主体的开关**由 data-view 派生**，
  // 不逐个枚举：枚举漏掉一个视图时，它的 section 会因标记里的 is-hidden 初值
  // 永远不显示，表现是「导航点亮、内容区整块空白」，而测试仍然全绿。
  for (const node of document.querySelectorAll("#view-switch .nav-btn")) {
    const active = node.dataset.view === view;
    node.classList.toggle("is-active", active);
    node.setAttribute("aria-pressed", String(active));
    document
      .getElementById(`view-${node.dataset.view}`)
      .classList.toggle("is-hidden", !active);
  }
  // 侧栏列表区：环境视图给环境列表，规则库视图给规则集列表（设计稿第 4 页），
  // 其余视图（对比 / 活动 / 调试 / 设置）不带列表区。
  document.getElementById("side-env").classList.toggle("is-hidden", view !== "environments");
  document.getElementById("side-rules").classList.toggle("is-hidden", view !== "rules");
  if (view === "rules") renderRuleSidebar(true);
  if (view === "activity") loadActivity();
  if (view === "debug") {
    refreshDebug().catch(reportError);
    refreshHarList().catch(reportError);
  }
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
  }
  // 编辑会话不再独立存在（配置页签 = 常驻编辑器）：环境没了，表单随
  // renderDetail 自动落到新的选中项，无需专门的"退出编辑"收尾。
}

async function refreshAll({ force = false, spinner = false } = {}) {
  const button = document.getElementById("refresh");
  if (spinner) button.classList.add("is-busy");
  // 显式刷新 = 用户要的是"现在的事实"，规则原文缓存跟着失效（热重载改的就是它）。
  if (force) clearRuleCaches();
  try {
    const [status, environments, rules, ca] = await Promise.all([
      get("/api/status"),
      get("/api/environments"),
      get("/api/rules"),
      get("/api/ca").catch(() => null),
    ]);
    state.status = status;
    state.environments = environments;
    state.rules = rules.rules;
    state.ca = ca;
    state.streamOk = true;
    stampOk();
    reconcileSelection();
    renderChrome();
    renderSidebar(force);
    renderDetail(force);
    renderRules(force);
    renderCa();
    renderGeneral();
    renderAbout();
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

  for (const node of document.querySelectorAll("#view-switch .nav-btn")) {
    node.addEventListener("click", () => setView(node.dataset.view));
  }

  // 新建环境：打开独立弹窗（设计稿第 8 页），不再借详情面板的表单。
  document.getElementById("env-new").addEventListener("click", () => {
    setView("environments");
    openCreateModal();
  });
  document.getElementById("empty-new").addEventListener("click", openCreateModal);
  document.getElementById("create-close").addEventListener("click", closeCreateModal);
  document.getElementById("create-cancel").addEventListener("click", closeCreateModal);
  document.getElementById("env-create-form").addEventListener("submit", (event) => {
    event.preventDefault();
    submitCreate(event.target);
  });
  // 失焦校验单字段（规范 6.6.5 节）：离开名称/端口栏时就地判定，不等到提交才发现。
  document.getElementById("env-create-form").addEventListener("focusout", (event) => {
    const input = event.target;
    const name = input && input.name;
    if (name !== "name" && name !== "port") return;
    const form = input.form;
    const holder = input.closest(".field");
    const stale = holder ? holder.querySelector(".field-error") : null;
    if (stale) stale.remove();
    input.removeAttribute("aria-invalid");
    const message = validateCreateField(form, name);
    if (message) markInvalid(form, `environment.${name}`, message, { noFocus: true });
  });

  // 破坏性确认模态（删除环境 / 删除规则 / 重分配端口共用一个壳）。
  document.getElementById("confirm-close").addEventListener("click", () => closeModal("modal-confirm", { force: true }));
  document.getElementById("confirm-cancel").addEventListener("click", () => closeModal("modal-confirm", { force: true }));
  document.getElementById("confirm-go").addEventListener("click", runConfirm);
  wireModalChrome();

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

  // 配置页签的常驻编辑器：任何输入都重估脏态（保存按钮的解锁/禁用）与暴露面警示。
  const envForm = document.getElementById("env-form");
  envForm.addEventListener("input", syncFormDirty);
  envForm.addEventListener("change", syncFormDirty);

  document.getElementById("env-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = event.target;
    const env = currentEnvironment();
    // 无改动时按钮本就是禁用态；这里再拦一道（回车提交也会走到 submit 事件）。
    if (!env || !state.formDirty) return;
    const key = `save:${env.name}`;
    if (pending.has(key)) return;
    pending.add(key);
    const submit = document.getElementById("env-form-submit");
    submit.classList.add("is-busy");
    submit.disabled = true;
    clearInvalid(form);
    const hint = document.getElementById("env-form-hint");
    try {
      const payload = formPayload(form, env);
      const saved = await mutate(environmentPath(env.name), "PATCH", payload);
      // 改名后把选中项搬到新名字：否则详情区会立刻回落到第一个环境，用户会以为改丢了。
      state.selected = saved && saved.name ? saved.name : env.name;
      state.formDirty = false;
      hint.textContent = "已保存。";
      await refreshAll({ force: true });
    } catch (error) {
      if (error.field) markInvalid(form, error.field, error.message);
      hint.textContent = "";
      if (!error.handled) reportError(error);
    } finally {
      pending.delete(key);
      submit.classList.remove("is-busy");
      updateSaveButton();
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
            ? `${domain} 在 ${rows.length} 个环境中的解析：${covered} 个覆盖`
            : "还没有环境，没有可对比的对象。",
        ),
      );
      for (const row of rows) {
        const line = el("div", `cmp-row ${row.covered ? "is-covered" : "is-uncovered"}`);
        line.dataset.env = row.env;
        line.appendChild(el("span", "cmp-env mono", row.env));
        line.appendChild(el("span", "cmp-arrow", "→"));
        // 命中给蓝色解析值 + 来源规则集 chip；未覆盖如实写「直连」。
        line.appendChild(
          el(
            "span",
            "cmp-value mono",
            row.covered ? row.ip : "（未覆盖，直连）",
          ),
        );
        const chip = el("span", row.covered ? "badge purple cmp-chip" : "badge warn cmp-chip");
        chip.appendChild(
          el("span", null, row.covered ? `命中 ${row.rules || "?"}.hosts` : "无规则命中"),
        );
        line.appendChild(chip);
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

  // ---- 日志（详情页签） ----
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
  document.getElementById("traj-follow").addEventListener("click", (event) => {
    state.trajFollow = !state.trajFollow;
    const button = event.currentTarget;
    button.classList.toggle("is-active", state.trajFollow);
    button.setAttribute("aria-pressed", String(state.trajFollow));
    setIcon(button, state.trajFollow ? "pause" : "play");
  });
  document.getElementById("activity-refresh").addEventListener("click", () => {
    loadActivity().catch(reportError);
  });
  document.getElementById("debug-start").addEventListener("click", () => {
    const env = document.getElementById("debug-env").value;
    if (!env) { toast("没有运行中的环境可调试。", "info"); return; }
    mutate("/api/debug", "POST", { env }).then(refreshDebug).catch(reportError);
  });
  document.getElementById("debug-stop").addEventListener("click", () => {
    mutate("/api/debug/stop", "POST").then(refreshDebug).catch(reportError);
  });
  document.getElementById("debug-clear").addEventListener("click", () => {
    const env = document.getElementById("debug-env").value;
    if (!env) return;
    mutate(`/api/environments/${encodeURIComponent(env)}/capture/clear`, "POST")
      .then(refreshDebug).catch(reportError);
  });
  document.getElementById("debug-export").addEventListener("click", () => {
    const env = document.getElementById("debug-env").value;
    if (!env) return;
    window.open(`/api/environments/${encodeURIComponent(env)}/captures/export?format=har`, "_blank");
  });
  document.getElementById("har-file").addEventListener("change", async (event) => {
    const file = event.target.files?.[0];
    if (!file) return;
    try {
      const body = JSON.parse(await file.text());
      await mutate(`/api/har/import?name=${encodeURIComponent(file.name)}`, "POST", body);
      await refreshHarList();
      toast("HAR 会话已导入。", "ok");
    } catch (error) {
      reportError(error);
    }
    event.target.value = "";
  });
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

  // ---- 规则库侧栏（规则集列表） ----
  const ruleSearch = document.getElementById("rule-search");
  const ruleSearchClear = document.getElementById("rule-search-clear");
  ruleSearch.addEventListener("input", () => {
    state.ruleSearch = ruleSearch.value;
    ruleSearchClear.classList.toggle("is-hidden", !ruleSearch.value);
    renderRuleSidebar(true);
  });
  ruleSearchClear.addEventListener("click", () => {
    ruleSearch.value = "";
    state.ruleSearch = "";
    ruleSearchClear.classList.add("is-hidden");
    renderRuleSidebar(true);
    ruleSearch.focus();
  });
  document.getElementById("rule-new").addEventListener("click", () => {
    field(document.getElementById("rules-form"), "name").focus();
  });

  // ---- 设置页（设计稿第 10 页：证书 / 通用 / 关于） ----
  for (const tab of document.querySelectorAll(".stab")) {
    tab.addEventListener("click", () => {
      for (const other of document.querySelectorAll(".stab")) {
        const active = other === tab;
        other.classList.toggle("is-active", active);
        other.setAttribute("aria-selected", String(active));
      }
      for (const panel of document.querySelectorAll(".spanel")) {
        panel.classList.toggle("is-hidden", panel.dataset.spanel !== tab.dataset.stab);
      }
    });
  }
  document.getElementById("ca-qr").addEventListener("click", openQrModal);
  document.getElementById("qr-close").addEventListener("click", closeQrModal);
}

// ---- 设置页渲染 ----

/// 下载/二维码的地址都要能带 token（EventSource 同款限制：<img>/<a> 带不了
/// 自定义头，token 只能走 query）。回环免鉴权档不带，URL 保持干净。
function caUrl(path) {
  return state.token ? `${path}?token=${encodeURIComponent(state.token)}` : path;
}

function renderCa() {
  const status = document.getElementById("ca-status");
  const text = document.getElementById("ca-status-text");
  const info = state.ca;
  if (!info) {
    status.className = "badge warn";
    text.textContent = "无法读取";
    document.getElementById("ca-info").replaceChildren(
      el("p", "empty-inline", "读不到共享 CA 的证书信息（confdir 里的证书缺失或形状异常）。"),
    );
    return;
  }
  status.className = "badge ok";
  text.textContent = info.is_ca ? "已就绪" : "异常（非 CA）";

  // 证书信息卡：基本信息 / 公钥 / 颁发者，全部来自服务端解析的证书 DER（只读）。
  const host = document.getElementById("ca-info");
  host.replaceChildren();
  const basic = el("div", "kv-grid");
  const basicCells = [
    ["版本", `v${info.version}`, false],
    ["是否根证书", info.is_ca ? "是" : "否", false],
    ["序列号", info.serial_hex, true],
    ["签名算法", info.sig_alg, false],
    ["有效开始", info.not_before, true],
    ["有效结束", info.not_after, true],
  ];
  for (const [label, value, mono] of basicCells) {
    basic.appendChild(kvCell(label, value, mono));
  }
  host.appendChild(el("p", "kv-section", "基本信息"));
  host.appendChild(basic);

  const pubkeyText = [info.pubkey_alg, info.pubkey_curve, info.pubkey_bits ? `${info.pubkey_bits} 位` : null]
    .filter(Boolean)
    .join(" ");
  host.appendChild(el("p", "kv-section", "公钥"));
  const pub = el("div", "kv-grid");
  pub.appendChild(kvCell("公钥算法", pubkeyText, false));
  pub.appendChild(kvCell("SHA-256 指纹（公钥）", info.fingerprint || "—", true));
  host.appendChild(pub);

  host.appendChild(el("p", "kv-section", "颁发者信息"));
  const issuer = el("div", "kv-grid");
  issuer.appendChild(kvCell("国家 / Country", info.country || "—", false));
  issuer.appendChild(kvCell("组织 / Organization", info.organization || "—", false));
  issuer.appendChild(kvCell("通用名 / CommonName", info.common_name || "—", false));
  issuer.appendChild(kvCell("使用者备用名称 / SAN", (info.san || []).join(" · ") || "—", true));
  host.appendChild(issuer);
}

function kvCell(label, value, mono) {
  const cell = el("div", "kv-cell");
  cell.appendChild(el("span", "kv-label", label));
  cell.appendChild(el("span", mono ? "kv-value mono" : "kv-value", value));
  return cell;
}

/// 「通用」页签：只读的运行信息（能在 UI 改的当前版本只有证书这一块，
/// 其余如实列出 —— 与设计稿的「当前版本仅支持证书配置」chip 同一句实话）。
function renderGeneral() {
  const status = state.status;
  const host = document.getElementById("general-info");
  if (!status) return;
  const grid = el("div", "kv-grid");
  const cells = [
    ["工作台地址", `${location.host}（${exposureText()}）`, true],
    ["代理核心", `${status.core.name} ${status.core.version}`, true],
    ["状态目录", status.config.state_dir, true],
    ["端口区间", status.config.port_range, true],
  ];
  for (const [label, value, mono] of cells) grid.appendChild(kvCell(label, value, mono));
  host.replaceChildren(grid);
}

function renderAbout() {
  const status = state.status;
  const host = document.getElementById("about-info");
  if (!status) return;
  const grid = el("div", "kv-grid");
  grid.appendChild(kvCell("产品", "envboard —— 一个环境 = 一个实例 = 一个端口", false));
  grid.appendChild(kvCell("工作台版本", status.version || "—", true));
  grid.appendChild(kvCell("代理核心", `${status.core.name} ${status.core.version}`, true));
  host.replaceChildren(grid);
}

// ---- 根证书二维码弹窗 ----

function openQrModal() {
  // 二维码内容是"手机能直连的那个地址"：只有浏览器知道它用哪个 host 访问的
  // 工作台，所以由前端拼 URL、服务端只负责编码（qrcode.svg 是无状态端点）。
  const url = `${location.protocol}//${location.host}${caUrl("/api/ca.pem")}`;
  document.getElementById("qr-img").src = `/api/ca/qrcode.svg?data=${encodeURIComponent(url)}`;
  document.getElementById("qr-url").textContent = url;
  openModal("modal-qr", document.activeElement, "#qr-close");
}

function closeQrModal() {
  closeModal("modal-qr", { force: true });
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

// URL 携带的 token：捕获进 state，**保留在地址栏里**。
//
// 曾经的做法是捕获后立刻抹掉（历史/截图更干净），代价是硬刷新必死 ——
// document 请求本身要带 token（服务端对页面本体不豁免），抹掉 query 之后 F5
// 只能拿到 401 JSON 页。可用性优先于观感：凭据本就在启动横幅里，换 token
// 随时可重启。之后所有请求走 header；SSE 用 ?token=。
{
  const token = new URLSearchParams(location.search).get("token");
  if (token && token.trim()) state.token = token.trim();
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

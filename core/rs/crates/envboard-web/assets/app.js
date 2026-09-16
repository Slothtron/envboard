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

/// 健康状态 → 徽章样式。徽章**文本始终是 `env.health` 原值**（验收靠它断言），
/// 这里只决定配色：一份状态只能有一种视觉表达。
const HEALTH_STYLE = {
  running: "running",
  stopped: "stopped",
  unhealthy: "unhealthy",
  config_mismatch: "error",
  port_conflict: "error",
  failed: "error",
};

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
  /// 就地二次确认的目标："env:beta" / "rule:beta"；null = 没有待确认的破坏性操作。
  confirm: null,
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
  const response = await fetch(path, options);
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
  const node = el("span", `badge ${HEALTH_STYLE[health] || ""}`.trim());
  const dot = el("span", "dot");
  dot.setAttribute("aria-hidden", "true");
  node.appendChild(dot);
  node.appendChild(el("span", null, health));
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

function renderChrome() {
  const status = state.status;
  if (status) {
    document.getElementById("core-badge-text").textContent = `core: ${status.core.name} ${status.core.version}`;
    document.getElementById("sidebar-foot-text").textContent =
      `${status.core.name} ${status.core.version} · ${status.config.state_dir}`;
    document.getElementById("env-form-port-hint").textContent =
      `留空则从端口区间 ${status.config.port_range} 里自动分配`;
  }
  const connection = connectionState();
  const badge = document.getElementById("conn-badge");
  badge.className = `badge ${connection.kind}`;
  document.getElementById("conn-badge-text").textContent = connection.text;
  const dot = document.getElementById("conn-dot");
  dot.className = `conn-dot is-${connection.kind}`;
  renderStats();
}

function renderStats() {
  const list = state.environments;
  const running = list.filter((env) => env.health === "running").length;
  const issues = list.filter((env) => ISSUE_HEALTH.has(env.health)).length;
  document.getElementById("stat-total").textContent = String(list.length);
  document.getElementById("stat-running").textContent = String(running);
  document.getElementById("stat-issues").textContent = String(issues);
  document.getElementById("stat-rules").textContent = String(state.rules.length);
  document.getElementById("env-count").textContent = String(list.length);
  document.getElementById("rules-count").textContent = String(state.rules.length);
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
  return [env.name, String(env.listen.port), env.rules || "", env.description || ""].some((value) =>
    value.toLowerCase().includes(needle),
  );
}

const visibleEnvironments = () =>
  state.environments.filter((env) => matchesFilter(env) && matchesSearch(env));

let sidebarKey = "";

function renderSidebar(force) {
  const list = visibleEnvironments();
  const key = JSON.stringify([
    list.map((env) => [env.name, env.health, env.listen.port, env.rules, env.rules_count, env.desired]),
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
  meta.appendChild(
    el("span", "env-rules mono", env.rules ? `${env.rules} (${env.rules_count})` : "不覆盖"),
  );
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
    const active = card.dataset.filter === state.filter;
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
  badge.className = `badge ${HEALTH_STYLE[env.health] || ""}`.trim();
  document.getElementById("detail-badge-text").textContent = env.health;
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
      onClick: () => copyText(env.proxy_command, "代理命令已复制到剪贴板"),
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
  if (env.health === "port_conflict") {
    const key = `reallocate:${env.name}`;
    host.appendChild(
      button({
        label: "重分配端口",
        icon: "shuffle",
        className: "btn sm",
        key,
        onClick: () => runAction(key, () => mutate(`${environmentPath(env.name)}/reallocate`, "POST")),
      }),
    );
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
  document.getElementById("ov-health").textContent = env.health;
  document.getElementById("ov-port").textContent = `${env.listen.host}:${env.listen.port}`;
  document.getElementById("ov-desired").textContent = env.desired;
  document.getElementById("ov-rules").textContent = env.rules || "（不覆盖）";
  document.getElementById("ov-rules-count").textContent = env.rules ? String(env.rules_count) : "—";
  document.getElementById("ov-desc").textContent = env.description || "—";
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
          `读不到规则文件 ${name}：${entry.error}　建议：在「规则库」重新导入同名文件，或把绑定改成（不覆盖）。`,
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
      box.appendChild(el("span", "confirm-text", `删除规则文件 ${name}？`));
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
    ruleTextCache.clear();
    try {
      await refreshAll();
    } catch (error) {
      reportError(error);
    }
  }
}

async function copyText(text, message) {
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
  ruleTextCache.clear();
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
  field(form, "name").focus();
}

/// 运行中只允许改描述：名字 / 端口 / 规则绑定都会让运行中的实例与配置对不上
/// （实例启动时就固定了规则路径与监听端口），服务端也会拒。界面先把这条路封住，
/// 免得用户改了再吃一个红字。状态变了要重新同步 —— 表单还开着的时候环境可能被停掉。
function syncEditLock(env) {
  const form = document.getElementById("env-form");
  const locked = env.health === "running";
  for (const name of ["name", "port", "rules"]) {
    const input = field(form, name);
    input.disabled = locked;
    input.title = locked ? "运行中不能改：先点「停止」" : "";
  }
  document.getElementById("env-form-hint").textContent = locked
    ? `编辑 ${env.name}：实例在运行，只能改描述。改名 / 换端口 / 换绑定要先停止。` +
      `（规则文件的内容是热重载的 —— 去「规则库」改那份文件，实例会自己跟上。）`
    : `编辑 ${env.name}：改完点「保存」。规则绑定在下次启动时生效。`;
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
  for (const name of ["name", "port", "rules"]) {
    const input = field(form, name);
    input.disabled = false;
    input.title = "";
    input.removeAttribute("aria-invalid");
  }
  form.reset();
  document.getElementById("env-form-title").textContent = "新建环境";
  document.getElementById("env-form-submit-text").textContent = "创建";
  document.getElementById("env-form-cancel").hidden = true;
  document.getElementById("env-form-mode").hidden = true;
  document.getElementById("env-form-hint").textContent = "";
}

/// 表单 → 请求体。创建与编辑共用，差别只有两处（都写在下面，避免两份字段映射漂移）：
///
/// * 编辑时 `rules` **总是显式给值**（空选 = `null` = 不覆盖）：PATCH 里"没给这个字段"
///   才是"不动它"，所以想解绑就必须真的把 null 发出去；
/// * 编辑时端口栏留空 = 保持当前端口（不写 `listen`），创建时留空 = 自动分配。
function formPayload(form, editing) {
  const port = field(form, "port").value.trim();
  const rules = field(form, "rules").value;
  const payload = { name: field(form, "name").value.trim(), description: field(form, "description").value };
  if (editing) {
    payload.rules = rules || null;
    if (port) payload.listen = { port: Number(port) };
  } else {
    if (rules) payload.rules = rules;
    if (port) payload.listen = { port: Number(port) };
  }
  return payload;
}

/// 字段级错误：把出错的输入框标出来并聚焦。只丢一条 toast 的话，用户还得自己
/// 在一屏字段里找哪一个是它说的那个。
function markInvalid(form, path) {
  const name = String(path).split(".").pop();
  const input = field(form, name);
  if (!input) return;
  input.setAttribute("aria-invalid", "true");
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

function renderLogs(force) {
  const rows = state.logLines.map(parseLogLine);
  const counts = { req: 0, res: 0, conn: 0, run: 0, err: 0 };
  let total = 0;
  for (const row of rows) {
    if (row.kind === "mark") continue;
    total += 1;
    if (counts[row.kind] !== undefined) counts[row.kind] += 1;
  }
  renderLogFilters(counts, total);

  const needle = state.logSearch.trim().toLowerCase();
  const visible = rows.filter((row) => {
    if (row.kind === "mark") return true; // 分段标记是现场的时间锚点，任何过滤下都保留
    if (state.logKind !== "all" && row.kind !== state.logKind) return false;
    if (needle && !row.text.toLowerCase().includes(needle)) return false;
    return true;
  });
  document.getElementById("log-count").textContent =
    `${visible.filter((row) => row.kind !== "mark").length}/${total}`;

  const key = `${state.logKind}|${state.logSearch}|${state.logLines.join("\u0000")}`;
  if (!force && key === state.logRenderKey) return;
  state.logRenderKey = key;

  const view = document.getElementById("logs");
  const previousTop = view.scrollTop;
  const fragment = document.createDocumentFragment();
  for (const row of visible) fragment.appendChild(logLineNode(row, needle));
  view.replaceChildren(fragment);
  applyAutoscroll(previousTop);
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
  if (force) ruleTextCache.clear();
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

  document.getElementById("copy-cmd").addEventListener("click", () => {
    const env = currentEnvironment();
    if (env) copyText(env.proxy_command, "代理命令已复制到剪贴板");
  });

  document.getElementById("env-form-cancel").addEventListener("click", () => {
    cancelEdit();
    renderDetail(true);
  });

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
    for (const name of ["name", "port"]) field(form, name).removeAttribute("aria-invalid");
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
      if (error.field) markInvalid(form, error.field);
      hint.textContent = "";
      reportError(error);
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
      ruleTextCache.clear();
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
  const source = new EventSource("/api/events");
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

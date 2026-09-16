// envboard 工作台 —— 原生 JS，无框架、无构建链。
//
// 两条约束（前端与服务端必须一致）：
//   · 变更类请求必须带 `x-envboard-request: 1`（跨站简单请求带不了自定义头，
//         所以这一条就是 CSRF 防线）；配了 token 时还要带 `x-envboard-token`。
//   · SSE 推的是"当前全部环境的快照"，前端不需要维护增量状态。
//
// 刻意把"渲染结果"写成可断言的形状（表格行、状态徽章文本），这样浏览器验收
// 能直接断言 DOM，而不是靠截图猜。

const REQUEST_HEADER = "x-envboard-request";
const TOKEN_HEADER = "x-envboard-token";

const state = {
  environments: [],
  token: "",
  logsEnv: null,
  // 非空 = 表单处于编辑模式，值是**改名前的原名**（PATCH 要打在这个名字上）。
  editing: null,
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
  api(path, { method, headers: headers(payload !== undefined), body: payload === undefined ? undefined : JSON.stringify(payload) });

function toast(message, bad) {
  const existing = document.querySelector(".toast");
  if (existing) existing.remove();
  const node = document.createElement("div");
  node.className = bad ? "toast bad" : "toast";
  node.textContent = message;
  document.body.appendChild(node);
  setTimeout(() => node.remove(), bad ? 6000 : 2500);
}

function reportError(error) {
  toast(error.field ? `${error.message}（字段：${error.field}）` : error.message, true);
}

// --------------------------------------------------------------------------- //
// 渲染
// --------------------------------------------------------------------------- //

const HEALTH_CLASS = {
  running: "ok",
  stopped: "",
  unhealthy: "warn",
  config_mismatch: "bad",
  port_conflict: "bad",
  failed: "bad",
};

function badge(text, kind) {
  const span = document.createElement("span");
  span.className = `pill ${kind || ""}`.trim();
  span.textContent = text;
  return span;
}

function cell(text, className) {
  const td = document.createElement("td");
  td.textContent = text;
  if (className) td.className = className;
  return td;
}

function actionButton(label, handler, className) {
  const button = document.createElement("button");
  button.type = "button";
  button.textContent = label;
  if (className) button.className = className;
  button.addEventListener("click", async () => {
    try {
      await handler();
    } catch (error) {
      reportError(error);
    }
  });
  return button;
}

// 只在数据真的变了才重建 DOM。
//
// 不是微优化：SSE 每秒推一次快照，无脑重建会把按钮从用户鼠标底下换掉（点击丢失）、
// 让辅助技术的 ref 立刻失效，也白白烧 CPU。浏览器验收用真实点击时这一点会立刻暴露。
let lastSignature = "";

function renderEnvironments(list, force) {
  const signature = JSON.stringify(list);
  if (!force && signature === lastSignature) return;
  lastSignature = signature;
  state.environments = list;
  // 日志面板的目标必须一直存在：环境被改名后它还指着旧名字，之后每秒一次的尾部读取
  // 都是 404（实测：那个 404 会把"已保存"顶成一条红字）。所以这里跟着列表自愈。
  if (!state.logsEnv || !list.some((env) => env.name === state.logsEnv)) {
    state.logsEnv = list.length ? list[0].name : null;
    document.getElementById("logs-env").textContent = state.logsEnv || "—";
  }
  // 编辑表单跟着最新状态走：环境可能在表单开着的时候被停掉（解锁）或被删掉。
  if (state.editing) {
    const target = list.find((env) => env.name === state.editing);
    if (target) {
      syncEditLock(target);
    } else {
      cancelEdit();
      toast("正在编辑的环境已不存在，已退出编辑。", true);
    }
  }
  const body = document.getElementById("env-body");
  body.replaceChildren();

  if (!list.length) {
    const row = document.createElement("tr");
    const td = cell("还没有环境。用下面的「新建环境」加一个。", "empty");
    td.colSpan = 7;
    row.appendChild(td);
    body.appendChild(row);
    return;
  }

  for (const env of list) {
    const row = document.createElement("tr");
    row.dataset.env = env.name;

    row.appendChild(cell(env.name, "name"));
    const healthCell = document.createElement("td");
    healthCell.appendChild(badge(env.health, HEALTH_CLASS[env.health]));
    if (env.health_reason) healthCell.title = env.health_reason;
    row.appendChild(healthCell);
    row.appendChild(cell(String(env.listen.port), "mono"));
    row.appendChild(cell(env.rules ? `${env.rules} (${env.rules_count})` : "—", "mono"));
    row.appendChild(cell(env.desired, "mono"));
    row.appendChild(cell(env.proxy_command, "cmd"));

    const actions = document.createElement("td");
    actions.className = "actions";
    actions.appendChild(actionButton("启动", async () => {
      await mutate(`/api/environments/${env.name}/start`, "POST");
      await refresh();
    }));
    actions.appendChild(actionButton("停止", async () => {
      await mutate(`/api/environments/${env.name}/stop`, "POST");
      await refresh();
    }));
    actions.appendChild(actionButton("重启", async () => {
      await mutate(`/api/environments/${env.name}/restart`, "POST");
      await refresh();
    }));
    actions.appendChild(actionButton("编辑", async () => {
      startEdit(env);
    }));
    actions.appendChild(actionButton("日志", async () => {
      state.logsEnv = env.name;
      document.getElementById("logs-env").textContent = env.name;
      await loadLogs();
    }));
    if (env.health === "port_conflict") {
      actions.appendChild(actionButton("重分配端口", async () => {
        await mutate(`/api/environments/${env.name}/reallocate`, "POST");
        await refresh();
      }, "primary"));
    }
    actions.appendChild(actionButton("删除", async () => {
      await mutate(`/api/environments/${env.name}`, "DELETE");
      await refresh();
    }, "danger"));
    row.appendChild(actions);

    body.appendChild(row);
  }
}

function renderStatus(status) {
  document.getElementById("core-pill").textContent = `core: ${status.core.name} ${status.core.version}`;
  const upstream = status.capabilities.rewrite_upstream;
  const pill = document.getElementById("conn-pill");
  pill.textContent = upstream ? "真实改写已启用" : "仅监听（不改写）";
  pill.className = upstream ? "pill ok" : "pill warn";
}

// 「载入」按钮：把已导入的规则原文取回表单，改完再导入（覆盖同名文件）。
async function loadRule(name) {
  const data = await get(`/api/rules/${encodeURIComponent(name)}`);
  const form = document.getElementById("rules-form");
  form.name.value = data.name;
  form.text.value = data.text;
  document.getElementById("rules-hint").textContent =
    `已载入 ${data.name}（${data.text.length} 字符）：改完点「导入」覆盖同名文件。`;
}

let lastRulesSignature = "";

function renderRules(names) {
  // 环境可能绑着一个**已经被删掉**的规则名（那时 start 会响亮失败）。这个选项必须
  // 留在下拉里：否则打开编辑再保存，就会把"绑了个不存在的规则"悄悄改成"不覆盖"。
  const bound = new Set(
    state.environments.map((env) => env.rules).filter((name) => name && !names.includes(name)),
  );
  const options = [...names, ...bound];
  const signature = JSON.stringify(options);
  if (signature === lastRulesSignature) return;
  lastRulesSignature = signature;

  const list = document.getElementById("rules-list");
  list.replaceChildren();
  if (!names.length) {
    const item = document.createElement("li");
    item.textContent = "（空）";
    list.appendChild(item);
  }
  for (const name of names) {
    const item = document.createElement("li");
    const label = document.createElement("span");
    label.textContent = name;
    item.appendChild(label);
    item.appendChild(actionButton("载入", async () => {
      await loadRule(name);
    }));
    item.appendChild(actionButton("删除", async () => {
      await mutate(`/api/rules/${name}`, "DELETE");
      await refresh();
    }, "danger"));
    list.appendChild(item);
  }

  const select = document.getElementById("rules-select");
  const previous = select.value;
  select.replaceChildren();
  const empty = document.createElement("option");
  empty.value = "";
  empty.textContent = "（不覆盖）";
  select.appendChild(empty);
  for (const name of options) {
    const option = document.createElement("option");
    option.value = name;
    option.textContent = bound.has(name) ? `${name}（文件不存在）` : name;
    select.appendChild(option);
  }
  select.value = previous;
}

// --------------------------------------------------------------------------- //
// 动作
// --------------------------------------------------------------------------- //

async function refresh() {
  const [status, environments, rules] = await Promise.all([
    get("/api/status"),
    get("/api/environments"),
    get("/api/rules"),
  ]);
  renderStatus(status);
  // 显式刷新时 force：用户点了"刷新"就该看到重绘，而不是被签名判断吞掉
  renderEnvironments(environments, true);
  renderRules(rules.rules);
  // 日志面板的目标由 renderEnvironments 维护（它知道最新的列表），这里只负责读一次尾部。
  await loadLogs();
}

/// 读当前选中环境的日志尾部。`state.logsEnv` 为空时什么都不做。
///
/// 这里自己吞掉读失败：日志面板是旁路，它读不到不该把整次 `refresh()` 判成失败 ——
/// 否则保存成功的那句"已保存。"会被这条错误顶掉，用户以为自己的改动没生效
/// （改名之后就真的踩到过：面板还指着旧名字）。
async function loadLogs() {
  if (!state.logsEnv) return;
  const panel = document.getElementById("logs");
  let lines;
  try {
    const data = await get(`/api/environments/${state.logsEnv}/logs?lines=200`);
    lines = data.lines.length ? data.lines.join("\n") : "（还没有日志）";
  } catch (error) {
    lines = `（读不到 ${state.logsEnv} 的日志：${error.message}）`;
  }
  // 内容没变就别碰 DOM：否则每秒重设一次会把选区与滚动位置弄丢。
  if (panel.textContent !== lines) panel.textContent = lines;
}

// --------------------------------------------------------------------------- //
// 新建 / 编辑（同一个表单）
// --------------------------------------------------------------------------- //

/// 进入编辑模式：把行里的值填进表单，并在表单上记住**原名**。
///
/// 记住原名而不是新名：改名时请求要打在旧名字的 URL 上，改完才换成新名字。
function startEdit(env) {
  const form = document.getElementById("env-form");
  state.editing = env.name;
  form.dataset.editing = env.name;
  form.name.value = env.name;
  form.port.value = String(env.listen.port);
  form.description.value = env.description || "";
  // 下拉里没有这个选项时先补一个（绑定的规则文件可能已被删）
  ensureRuleOption(env.rules);
  form.rules.value = env.rules || "";
  document.getElementById("env-form-title").textContent = "编辑环境";
  document.getElementById("env-form-submit").textContent = "保存";
  document.getElementById("env-form-cancel").hidden = false;
  const mode = document.getElementById("env-form-mode");
  mode.hidden = false;
  mode.textContent = env.name;
  syncEditLock(env);
  form.name.focus();
}

/// 运行中只允许改描述：名字 / 端口 / 规则绑定都会让运行中的实例与配置对不上
/// （实例启动时就固定了规则路径与监听端口），服务端也会拒。界面先把这条路封住，
/// 免得用户改了再吃一个红字。状态变了要重新同步 —— 表单还开着的时候环境可能被停掉。
function syncEditLock(env) {
  const form = document.getElementById("env-form");
  const locked = env.health === "running";
  for (const input of [form.name, form.port, form.rules]) {
    input.disabled = locked;
    input.title = locked ? "运行中不能改：先点「停止」" : "";
  }
  document.getElementById("env-form-hint").textContent = locked
    ? `编辑 ${env.name}：实例在运行，只能改描述。改名/换端口/换绑定要先停止。` +
      `（规则文件的内容是热重载的 —— 去「规则库」改那份文件，实例会自己跟上。）`
    : `编辑 ${env.name}：改完点「保存」。规则绑定在下次启动时生效。`;
}

function ensureRuleOption(name) {
  if (!name) return;
  const select = document.getElementById("rules-select");
  if ([...select.options].some((option) => option.value === name)) return;
  const option = document.createElement("option");
  option.value = name;
  option.textContent = `${name}（文件不存在）`;
  select.appendChild(option);
}

function cancelEdit() {
  const form = document.getElementById("env-form");
  state.editing = null;
  form.dataset.editing = "";
  for (const input of [form.name, form.port, form.rules]) {
    input.disabled = false;
    input.title = "";
  }
  form.reset();
  document.getElementById("env-form-title").textContent = "新建环境";
  document.getElementById("env-form-submit").textContent = "创建";
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
  const port = form.port.value.trim();
  const rules = form.rules.value;
  const payload = { name: form.name.value.trim(), description: form.description.value };
  if (editing) {
    payload.rules = rules || null;
    if (port) payload.listen = { port: Number(port) };
  } else {
    if (rules) payload.rules = rules;
    if (port) payload.listen = { port: Number(port) };
  }
  return payload;
}

function wireEvents() {
  document.getElementById("refresh").addEventListener("click", () => {
    refresh().catch(reportError);
  });

  document.getElementById("env-form-cancel").addEventListener("click", () => {
    cancelEdit();
  });

  document.getElementById("env-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = event.target;
    const editing = state.editing;
    const hint = document.getElementById("env-form-hint");
    try {
      const payload = formPayload(form, editing);
      if (editing) {
        await mutate(`/api/environments/${encodeURIComponent(editing)}`, "PATCH", payload);
        cancelEdit();
        hint.textContent = "已保存。";
      } else {
        await mutate("/api/environments", "POST", payload);
        form.reset();
        hint.textContent = "已创建。";
      }
      await refresh();
    } catch (error) {
      hint.textContent = "";
      reportError(error);
    }
  });

  document.getElementById("compare-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const host = event.target.host.value.trim();
    try {
      const data = await get(`/api/compare?host=${encodeURIComponent(host)}`);
      const box = document.getElementById("compare-result");
      box.replaceChildren();
      for (const row of data.environments) {
        const line = document.createElement("div");
        line.className = "row";
        const name = document.createElement("span");
        name.textContent = row.env;
        const value = document.createElement("span");
        value.className = row.covered ? "covered" : "uncovered";
        value.textContent = row.covered
          ? `→ ${row.ip}  (端口 ${row.port})`
          : `未覆盖 (端口 ${row.port})`;
        line.appendChild(name);
        line.appendChild(value);
        box.appendChild(line);
      }
    } catch (error) {
      reportError(error);
    }
  });

  document.getElementById("rules-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = event.target;
    try {
      const name = form.name.value.trim();
      await mutate("/api/rules", "POST", { name, text: form.text.value });
      // 刻意**不**清空表单：规则大多是"载入 → 改几行 → 再导入"，清掉反而逼人重新载入。
      document.getElementById("rules-hint").textContent =
        `已导入 ${name}（覆盖同名文件）；绑定它的实例按 mtime 热重载。`;
      await refresh();
    } catch (error) {
      reportError(error);
    }
  });
}

function connectEvents() {
  const source = new EventSource("/api/events");
  source.addEventListener("snapshot", (event) => {
    const payload = JSON.parse(event.data);
    if (!payload.ok) return;
    renderEnvironments(payload.environments);
    // 日志面板跟着刷新：它只在点"日志"时取过一次的话，之后永远停在那一次的快照上
    // （崩溃现场、热重载规则都看不到）。快照本身就是每秒一次，顺势读一次尾部即可。
    loadLogs().catch(() => {});
    const pill = document.getElementById("conn-pill");
    if (!pill.classList.contains("ok")) pill.classList.add("ok");
  });
  source.addEventListener("error", () => {
    const pill = document.getElementById("conn-pill");
    pill.textContent = "事件流断开（界面仍在轮询）";
    pill.className = "pill warn";
  });
}

// 给浏览器验收用的显式标记：**JS 真的跑完了**才写它。
document.documentElement.dataset.envboardReady = "pending";

wireEvents();
connectEvents();
refresh()
  .then(() => {
    document.documentElement.dataset.envboardReady = "yes";
  })
  .catch((error) => {
    document.documentElement.dataset.envboardReady = "no";
    reportError(error);
  });

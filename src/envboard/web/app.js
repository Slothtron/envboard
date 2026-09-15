"use strict";
const PREFIX = location.pathname.replace(/\/+$/, "");
const API = PREFIX + "/api";

const $ = (id) => document.getElementById(id);
let state = { environments: [], active: "", editing: null, rules: [] };

function toast(msg, kind) {
  const el = document.createElement("div");
  el.className = "toast " + (kind || "ok");
  el.textContent = msg;
  document.body.appendChild(el);
  setTimeout(() => el.remove(), kind === "err" ? 6500 : 2800);
}

function xsrf() {
  const m = document.cookie.match(/(?:^|;\s*)_mitmproxy_xsrf=([^;]*)/);
  return m ? decodeURIComponent(m[1]) : "";
}

async function api(path, options) {
  const opts = Object.assign({ method: "GET", headers: {} }, options || {});
  // 注意：tornado 的 xsrf_cookies 对**所有**非安全方法生效，包括没有 body 的 DELETE。
  // 只在有 body 时才加 X-XSRFToken 是错的 —— 删除按钮会 403。
  const mutating = !["GET", "HEAD", "OPTIONS"].includes(String(opts.method).toUpperCase());
  if (opts.body !== undefined) {
    if (typeof opts.body !== "string") opts.body = JSON.stringify(opts.body);
    opts.headers["Content-Type"] = "application/json";
  }
  if (mutating) opts.headers["X-XSRFToken"] = xsrf();
  const res = await fetch(API + path, opts);
  const text = await res.text();
  let data = null;
  try { data = text ? JSON.parse(text) : null; } catch (e) { data = { raw: text }; }
  if (!res.ok) {
    const msg = data && data.error ? (data.error.code + ": " + data.error.message) : (res.status + " " + res.statusText);
    throw new Error(msg);
  }
  return data;
}

function hostsToText(hosts) {
  return Object.keys(hosts || {}).sort().map((k) => k + "=" + hosts[k]).join("\n");
}
function textToHosts(text) {
  const out = {};
  (text || "").split("\n").forEach((line) => {
    const s = line.trim();
    if (!s || s.startsWith("#")) return;
    const i = s.indexOf("=");
    if (i < 0) throw new Error("hosts 行缺少 '=': " + s);
    out[s.slice(0, i).trim()] = s.slice(i + 1).trim();
  });
  return out;
}

function renderEnvs() {
  const box = $("envList");
  box.innerHTML = "";
  state.environments.forEach((env) => {
    const div = document.createElement("div");
    div.className = "env" + (env.name === state.editing ? " sel" : "");
    const dot = document.createElement("span");
    dot.className = "dot";
    if (env.active) dot.style.background = "var(--ok)";
    else if (env.color) dot.style.background = env.color;
    const meta = document.createElement("div");
    meta.style.minWidth = "0";
    meta.style.flex = "1";
    const nm = document.createElement("div");
    nm.className = "nm";
    nm.textContent = env.name + (env.active ? "  ●  active" : "");
    const sub = document.createElement("div");
    sub.className = "sub";
    sub.textContent = (env.dns_servers.length ? env.dns_servers.join(", ") : "<system dns>") +
      (env.domain_suffix ? "  ·  " + env.domain_suffix : "") +
      "  ·  " + Object.keys(env.hosts || {}).length + " hosts" +
      (env.rules_file ? "  ·  rules: " + env.rules_file : "");
    meta.appendChild(nm); meta.appendChild(sub);
    div.appendChild(dot); div.appendChild(meta);
    div.onclick = () => selectEnv(env.name);
    box.appendChild(div);
  });
  if (!state.environments.length) box.innerHTML = '<div class="muted">还没有环境</div>';
}

function selectEnv(name) {
  const env = state.environments.find((e) => e.name === name);
  if (!env) return;
  state.editing = name;
  $("fName").value = env.name;
  $("fColor").value = env.color || "";
  $("fDns").value = (env.dns_servers || []).join(", ");
  $("fSuffix").value = env.domain_suffix || "";
  $("fHosts").value = hostsToText(env.hosts);
  $("fDesc").value = env.description || "";
  $("editMode").textContent = "编辑 " + env.name;
  renderEnvs();
  syncRuleBind(env.rules_file || "");
}

function blank() {
  state.editing = null;
  ["fName", "fColor", "fDns", "fSuffix", "fHosts", "fDesc"].forEach((id) => { $(id).value = ""; });
  $("editMode").textContent = "新建环境";
  renderEnvs();
  $("fName").focus();
}

// 映射表范围下拉：全部环境 + 每个环境（保留当前选中项）
function syncMapEnvOptions() {
  const sel = $("mapEnv");
  const previous = sel.value || "*";
  sel.innerHTML = "";
  const all = document.createElement("option");
  all.value = "*";
  all.textContent = "全部环境";
  sel.appendChild(all);
  state.environments.forEach((env) => {
    const opt = document.createElement("option");
    opt.value = env.name;
    opt.textContent = env.name + (env.active ? "  ●" : "");
    sel.appendChild(opt);
  });
  sel.value = Array.from(sel.options).some((o) => o.value === previous) ? previous : "*";
}

async function loadAll() {
  const data = await api("/environments");
  state.environments = data.environments || [];
  state.active = data.active || "";
  if (!state.editing || !state.environments.some((e) => e.name === state.editing)) {
    state.editing = state.active && state.environments.some((e) => e.name === state.active)
      ? state.active : (state.environments[0] || {}).name || null;
  }
  $("activeTag").textContent = "active: " + (state.active || "—");
  $("activeTag").className = "tag on";
  syncMapEnvOptions();
  if (state.editing) selectEnv(state.editing); else renderEnvs();
  try {
    const st = await api("/status");
    $("dnsTag").textContent = "dns: " + ((st.servers.effective || []).join(", ") || "—");
    $("cfgPath").textContent = st.config_path || "";
    $("idxStats").textContent = "host×env=" + st.index.hosts_per_env + " entries=" + st.index.entries +
      " hits=" + st.index.hits + " misses=" + st.index.misses;
  } catch (e) { /* status 失败不影响主流程 */ }
}

async function save() {
  let hosts;
  try { hosts = textToHosts($("fHosts").value); }
  catch (e) { toast(e.message, "err"); return; }
  const payload = {
    name: $("fName").value.trim(),
    dns_servers: $("fDns").value.split(",").map((s) => s.trim()).filter(Boolean),
    domain_suffix: $("fSuffix").value.trim(),
    color: $("fColor").value.trim(),
    description: $("fDesc").value.trim(),
    hosts: hosts
  };
  if (!payload.name) { toast("名称不能为空", "err"); return; }
  try {
    if (state.editing) {
      await api("/environments/" + encodeURIComponent(state.editing), { method: "PUT", body: payload });
      toast("已保存 " + payload.name);
    } else {
      await api("/environments", { method: "POST", body: payload });
      toast("已创建 " + payload.name);
    }
    state.editing = payload.name;
    await loadAll();
  } catch (e) { toast(e.message, "err"); }
}

async function activate() {
  if (!state.editing) { toast("先选择一个环境", "err"); return; }
  try {
    await api("/active", { method: "PUT", body: { name: state.editing } });
    toast("已切换到 " + state.editing);
    await loadAll();
  } catch (e) { toast(e.message, "err"); }
}

async function remove() {
  if (!state.editing) return;
  if (!confirm("删除环境 " + state.editing + " ?")) return;
  try {
    await api("/environments/" + encodeURIComponent(state.editing), { method: "DELETE" });
    toast("已删除 " + state.editing);
    state.editing = null;
    await loadAll();
  } catch (e) { toast(e.message, "err"); }
}

function renderMappings(mappings) {
  const body = $("mapBody");
  body.innerHTML = "";
  if (!mappings || !mappings.length) {
    body.innerHTML = '<tr><td colspan="5" class="muted">尚无数据</td></tr>';
    return;
  }
  mappings.forEach((m) => {
    const tr = document.createElement("tr");
    [m.env, m.host, m.ip, m.source, String(m.age)].forEach((v) => {
      const td = document.createElement("td");
      td.textContent = v;
      tr.appendChild(td);
    });
    body.appendChild(tr);
  });
}

async function loadMappings() {
  try {
    const q = encodeURIComponent($("qFilter").value.trim());
    const env = encodeURIComponent($("mapEnv").value || "*");
    const data = await api("/mappings?env=" + env + "&q=" + q);
    renderMappings(data.mappings);
  } catch (e) { toast(e.message, "err"); }
}

async function resolveHosts() {
  const hosts = $("qHosts").value.split(",").map((s) => s.trim()).filter(Boolean);
  if (!hosts.length) { toast("请输入至少一个 host", "err"); return; }
  const btn = $("btnResolve"); btn.disabled = true;
  try {
    const data = await api("/resolve", { method: "POST", body: { hosts: hosts, env: state.editing || "" } });
    const bad = data.results.filter((r) => !r.ips.length);
    toast("解析 " + (data.results.length - bad.length) + "/" + data.results.length + " 成功" +
      (bad.length ? "；未解析: " + bad.map((r) => r.host + "(" + r.rcode + ")").join(", ") : ""));
    await loadMappings();
    await loadStatus();
  } catch (e) { toast(e.message, "err"); }
  finally { btn.disabled = false; }
}

async function resolveAllEnvs() {
  const hosts = $("qHosts").value.split(",").map((s) => s.trim()).filter(Boolean);
  if (!hosts.length) { toast("请输入至少一个 host", "err"); return; }
  const btn = $("btnAllEnvs"); btn.disabled = true;
  try {
    const data = await api("/resolve", { method: "POST", body: { hosts: hosts, all_envs: true } });
    const groups = data.environments || {};
    let total = 0, hit = 0;
    Object.values(groups).forEach((rows) => rows.forEach((r) => { total++; if (r.ips.length) hit++; }));
    toast("已对 " + Object.keys(groups).length + " 套环境解析 " + total + " 条，命中 " + hit + " 条");
    $("mapEnv").value = "*";     // 对比视图默认看全部环境
    $("qFilter").value = "";
    await loadMappings();
    await loadStatus();
  } catch (e) { toast(e.message, "err"); }
  finally { btn.disabled = false; }
}

async function loadStatus() {
  try {
    const st = await api("/status");
    $("dnsTag").textContent = "dns: " + ((st.servers.effective || []).join(", ") || "—");
    $("idxStats").textContent = "host×env=" + st.index.hosts_per_env + " entries=" + st.index.entries +
      " hits=" + st.index.hits + " misses=" + st.index.misses;
  } catch (e) { /* ignore */ }
}

async function refreshWatchlist() {
  try {
    const data = await api("/refresh", { method: "POST", body: {} });
    toast("已刷新 " + data.refreshed + " 个 host（watchlist）");
    await loadMappings(); await loadStatus();
  } catch (e) { toast(e.message, "err"); }
}

// ---------------------------------------------------------------- 规则文件

function currentEnv() {
  return state.environments.find((e) => e.name === state.editing);
}

function renderRules(rules) {
  const box = $("ruleBody");
  box.innerHTML = "";
  if (!rules.length) {
    box.innerHTML = '<tr><td colspan="4" class="muted">还没有规则文件</td></tr>';
    return;
  }
  rules.forEach((r) => {
    const tr = document.createElement("tr");
    [r.name, String(r.entries), String(r.ips), (r.environments || []).join(", ") || "—"]
      .forEach((v) => {
        const td = document.createElement("td");
        td.textContent = v;
        tr.appendChild(td);
      });
    tr.style.cursor = "pointer";
    tr.onclick = () => { $("ruleName").value = r.name; syncRuleBind(r.name); };
    box.appendChild(tr);
  });
}

function syncRuleBind(selected) {
  const sel = $("ruleBind");
  const previous = selected !== undefined ? selected : sel.value;
  sel.innerHTML = "";
  const none = document.createElement("option");
  none.value = "";
  none.textContent = "（不绑定）";
  sel.appendChild(none);
  state.rules.forEach((r) => {
    const opt = document.createElement("option");
    opt.value = r.name;
    opt.textContent = r.name + "  (" + r.entries + " entries)";
    sel.appendChild(opt);
  });
  sel.value = Array.from(sel.options).some((o) => o.value === previous) ? previous : "";
}

async function loadRules() {
  try {
    const data = await api("/rules");
    state.rules = data.rules || [];
    $("rulesDir").textContent = data.dir || "";
    renderRules(state.rules);
    syncRuleBind(currentEnv() ? currentEnv().rules_file || "" : "");
  } catch (e) { toast(e.message, "err"); }
}

// 注意：跳过项的 detail 来自**未通过校验**的原始 token，可能含任意字符，
// 因此这里一律用 textContent 组装，不拼 innerHTML。
function renderRuleReport(payload) {
  const box = $("ruleReport");
  box.innerHTML = "";
  const s = payload.stats || {};
  const add = (text) => {
    const div = document.createElement("div");
    div.className = "hint";
    div.textContent = text;
    box.appendChild(div);
  };
  add("已导入 " + payload.name + "：accepted=" + s.accepted + " over " + payload.ips +
      " ip，skipped=" + s.skipped + "，conflicts=" + s.conflicts);
  if ((payload.conflicts || []).length) {
    add("冲突（后出现者胜）：" + payload.conflicts.slice(0, 6)
      .map((c) => c.host + " " + c.dropped + " → " + c.kept).join("；"));
  }
  if ((payload.skipped || []).length) {
    add("已忽略：" + payload.skipped.slice(0, 8)
      .map((i) => "L" + i.line + " " + i.reason + (i.detail ? " " + i.detail : "")).join("；") +
      (payload.skipped.length > 8 ? " …共 " + payload.skipped.length + " 条" : ""));
  }
}

async function importRules(body, okMsg) {
  try {
    const data = await api("/rules", { method: "POST", body: body });
    renderRuleReport(data.import);
    $("ruleName").value = data.import.name;
    toast(okMsg);
    await loadRules();
  } catch (e) { toast(e.message, "err"); }
}

async function importRulesText() {
  const name = $("ruleName").value.trim();
  if (!name) { toast("先填规则名", "err"); return; }
  await importRules({ name: name, text: $("ruleText").value }, "已导入规则 " + name);
}

async function importRulesPath() {
  const path = $("rulePath").value.trim();
  if (!path) { toast("先填文件路径", "err"); return; }
  await importRules({ name: $("ruleName").value.trim(), path: path }, "已从文件导入规则");
}

async function bindRules() {
  const env = currentEnv();
  if (!env) { toast("先选一个环境", "err"); return; }
  try {
    const data = await api("/environments/" + encodeURIComponent(env.name),
      { method: "PUT", body: { rules_file: $("ruleBind").value } });
    toast("环境 " + data.environment.name + " 现用规则 " +
      (data.environment.rules_file || "（无）"));
    await loadAll();
    await loadRules();
  } catch (e) { toast(e.message, "err"); }
}

async function deleteRule() {
  const name = $("ruleName").value.trim();
  if (!name) { toast("先填规则名", "err"); return; }
  try {
    await api("/rules/" + encodeURIComponent(name), { method: "DELETE" });
    toast("已删除规则 " + name);
    $("ruleReport").innerHTML = "";
    await loadRules();
  } catch (e) { toast(e.message, "err"); }
}

$("btnReload").onclick = () => loadAll().catch((e) => toast(e.message, "err"));
$("btnNew").onclick = blank;
$("btnSave").onclick = save;
$("btnActivate").onclick = activate;
$("btnDelete").onclick = remove;
$("btnMappings").onclick = loadMappings;
$("btnResolve").onclick = resolveHosts;
$("btnAllEnvs").onclick = resolveAllEnvs;
$("btnRefresh").onclick = refreshWatchlist;
$("qFilter").addEventListener("keydown", (e) => { if (e.key === "Enter") loadMappings(); });
$("mapEnv").addEventListener("change", loadMappings);
$("btnRulesReload").onclick = loadRules;
$("btnRuleImport").onclick = importRulesText;
$("btnRuleImportPath").onclick = importRulesPath;
$("btnRuleBind").onclick = bindRules;
$("btnRuleDelete").onclick = deleteRule;
$("ruleBind").addEventListener("change", bindRules);

loadAll().then(loadRules).then(loadMappings).catch((e) => toast("初始化失败: " + e.message, "err"));

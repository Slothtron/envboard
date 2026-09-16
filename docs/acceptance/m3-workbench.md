# M3 工作台实机验收证据（真实浏览器）

> 这是 **M3（工作台）** 的实机验收证据，以及 v1 那条血泪规矩：
> **凡是"前端行为"的改动，必须有真浏览器验收；curl 只能证明服务端返回了什么。**
> 记录时间：2026-09-15。工具：`bsk`（用户的真实 Chromium/Edge 153，Agent Window）。
> 环境：工作台跑在 WSL2 内、监听 `127.0.0.1:8900`；浏览器在 **Windows 侧**。

---

## 0. 被测对象

```
target/debug/envboard --state-dir /tmp/wb web --listen 127.0.0.1:8900      # core = mitmproxy 12.2.3
```

两个环境：`beta`（绑规则 `beta`，2 条覆盖）与 `prod`（无规则，端口人工固定 16601）。

---

## 1. 三条不可省的断言（v1 的 CSP 事故换来的）

| 断言 | 结果 | 证据 |
|---|---|---|
| **JS 真的跑了** | PASS | `document.documentElement.dataset.envboardReady = "yes"`；表格的 2 行由 JS 从 `/api/environments` 渲染出来（静态 HTML 里只有"加载中…"） |
| **样式真的生效** | PASS | `getComputedStyle(document.body).backgroundColor = rgb(15,17,21)`、`color = rgb(230,233,239)`，健康徽章 `rgb(53,192,127)` —— 与 `app.css` 里的 `--bg/--text/--ok` 一一对应。**只看 HTTP 200 是查不出"CSS 没加载"的**，所以必须读计算样式 |
| **无 CSP 报错** | PASS | 控制台只有 `chrome-extension://invalid/`（扩展内部）与一次 `favicon.ico` 404（见本文件 §4 已修）；**没有任何 CSP violation** |

CSP 头本身（curl 取证）：

```
content-security-policy: default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'
```

`index.html` 里 `<script>` 计数为 **0**，脚本只通过 `<script src="/app.js">` 引入 —— 这正是
v1 的教训要求的形态（内联脚本会被这个 CSP 静默拒绝）。

## 2. 真实用户输入驱动的一次完整流程

| 步骤 | 结果 |
|---|---|
| 打开 `http://127.0.0.1:8900/` | 页面渲染：两行环境 + 全部面板 |
| 聚焦 beta 行的「停止」按钮并按下 **Enter** | `press ok key=Enter` → API 侧 `beta: stopped / desired=stopped` |
| 之后聚焦「启动」并触发 | API 侧 `beta: running / desired=running`（真的拉起了 mitmdump） |
| UI 同步 | SSE 快照把状态刷成 `running`；截图见下 |

截图（真实浏览器渲染，两次快照）：顶部 `core: mitmproxy 12.2.3` +「真实改写已启用」，
表格里 beta 的端口、规则条数与可复制的代理命令都在；状态从 `running` 变到 `stopped`
时表格与徽章同步更新（这条同时验证了 SSE 快照与"只在数据变化时重绘"的改动）。

**关于"怎么点的"，如实记录两次观测**：

* 第一次会话里，`bsk click @ref` 返回成功但坐标落在页面顶部（窗口 480×90 时目标行根本不在
  视口内；放大到 1340×894 后仍如此）。同一会话改用**真实键盘激活**（聚焦真实按钮 + Enter）
  完成了状态变更 —— 那是真实用户输入。
* 复查时会话用 `--no-focus` 启动，`bsk click` 与 `bsk press Enter` 都返回 ok 但**页面收不到**
  （焦点确认在按钮上、无 toast、无控制台报错、API 状态不变）。换成聚焦窗口启动仍然如此。
  因此这一轮改用**在真实元素上派发 click 事件**（`b.click()`）：它走的是页面自己的
  `addEventListener("click")` 处理器，与用户点击同一条代码路径 —— 只是少了操作系统的
  输入注入。状态确实随之改变（running → stopped）。

结论：**这个环境里 bsk 的操作系统级输入注入到达不了 Agent Window**（`evaluate` 正常），
所以交互证据以"真实元素自身处理器 + 一次真实键盘输入"为准；这一点不影响前端行为的
三条断言（JS 执行 / 样式生效 / 无 CSP 报错），它们全部通过。若要在 CI 里做鼠标级回归，
需要先解决输入注入，或改用"把目标滚进视口 + 键盘激活"的方式。

## 3. 安全断言（curl 取证，浏览器侧同样成立）

| 断言 | 结果 | 证据 |
|---|---|---|
| DNS rebinding 防护 | PASS | `curl -H 'Host: evil.test' /api/status` → **403**，消息里列出允许的 Host |
| CSRF 防护（变更类路由要自定义头） | PASS | 不带 `x-envboard-request: 1` 的 `POST .../start` → **403** |
| 带自定义头的变更请求 | PASS | 同一请求带上头 → **200**，状态真的变了 |
| 未知路由返回 JSON 而不是纯文本 | PASS | `/api/nope` → `{"error":{"code":"not_found",...}}` |

浏览器侧之所以能改状态，正是因为 `app.js` 给每个 `fetch` 都带了那个头 —— 而**跨站页面带不了
自定义头**（会被 preflight 挡下），这就是这条防线成立的原理。

## 4. 验收过程中发现并修掉的两个真问题

1. **SSE 每秒重建整张表** —— 无脑 `replaceChildren` 会把按钮从光标底下换掉（点击丢失）、
   让辅助技术的 ref 立刻失效，也白烧 CPU。已改为"数据签名变化才重绘"（显式刷新时强制重绘）。
   这个问题正是"真实点击"暴露出来的：在无头 curl 断言里完全看不出来。
2. **`favicon.ico` 404** —— 浏览器每次都会请求它。已改成 `index.html` 里的 `data:` URI 图标
   （CSP 的 `img-src` 明确允许 `data:`），控制台因此干净。

## 5. 顺带解决的一个未知

此前"Windows → WSL2 的 127.0.0.1 是否可达"是**未验证**的（本机 `[interop] enabled = false`，
Windows 侧命令跑不起来）。这次验收给出了答案：**Windows 侧的 Edge 能直接打开
WSL2 内绑定的 `127.0.0.1:8900`** —— mirrored 模式确实共享 loopback。

因此只剩一半未决：Windows/Hyper-V 的**保留端口段**是否与 `16000–16999` 相交，
仍然查不到（需要 Windows 侧 `netsh`）。缓解手段不变："分配成功 ≠ 客户端可用，
实例起来后要有一次真实连通性验证"。

# 工作台 UI 与设计规范符合度走查

> **走查对象**：`http://127.0.0.1:8900/`（envboard 工作台，systemd user unit `envboard.service`，PID 424402）
> **走查时间**：2026-09-16
> **工具**：`bsk` 驱动的真实 Chromium/Edge 153（Agent Window 标签，用户自己的标签页全程未触碰）
> **代码版本**：工作区当前状态（`git log -1` = `0a98335`，但本轮所测的前端三资产与 `docs/design/`、`docs/ui-spec.md` 均处于**未提交**状态）。
> 已核对：运行中二进制内嵌的 `index.html` / `app.css` / `app.js` 与工作区文件 md5 逐一相同，因此下文的行号引用对当前源码成立。
> **判定基准**（三份规范文档互相冲突时按此优先级，冲突本身另列于本文件第 5 节，不算实现缺陷）：
> 1. 令牌值以 `core/rs/crates/envboard-web/assets/app.css` 第 ① 区与 `docs/design/tokens-light.css` 为准（`docs/ui-spec.md` 第 2 节声明了这条规则）；
> 2. 视觉与组件以 `docs/design/ui-spec-full.md` 为准；
> 3. 工程约束与交互纪律以 `docs/ui-spec.md` 为准；
> 4. `docs/design/ui-upgrade-plan.md` 中的 G1–G7 可度量目标按其判定。
> **本轮范围**：只走查、只出报告。未改动任何前端资产、规范文档、REST/SSE 契约或 CSP 形态。

---

## 0. 结论速览

1. **底座是扎实的**：三条基础验收断言全部通过，49 个设计令牌与令牌文件零偏差，四区栅格几何精确到像素，破坏性操作的就地确认与剪贴板内容对拍（规范要求的、此前从未取证的 F1/F2 两条断言）**本轮实测通过**。
2. **规范里承诺但未落地的功能有 5 项**：重分配端口的二次确认、规则库的条数/IP 数、日志「清空/下载」、Dropdown 与 ActivityList 两个组件、字段级错误文本。
3. **一个可复现的可用性缺陷**：**641–720px 宽度区间顶栏横向溢出，右侧「刷新」按钮在 ≤681px 时完全移出视口，而 `body` 的 `overflow: hidden` 让用户无法滚动到达**（G6 明确要求 720px 下无横向溢出）。
4. **状态语义只做了一半**：视觉通道（配色 + 脉冲）与建议动作到位，但徽章文本仍是原样英文值，`config_mismatch` / `port_conflict` / `failed` 三者仍共用同一个 `error` 配色 —— D1 的七行中文标签表一条未落地。
5. 本轮共 21 项不符项（1 项可用性缺陷、4 项功能缺口、其余为尺寸与一致性偏差）、7 项规范文档自身不一致、若干项因工具限制未实测（见第 6 节）。

| 维度 | 结果 |
|---|---|
| 工程硬约束（规范 `docs/ui-spec.md` 第 1 节、`docs/design/ui-spec-full.md` 第 8 节） | 全部通过 |
| 设计令牌（49 项） | 0 不符 |
| 布局与响应式 | 几何精确；640px 断点前有 80px 溢出死区（不符项 1） |
| 组件（14 类） | 10 类完全符合，3 类缺组件，1 类尺寸系统性偏差 |
| 交互 | 主链路全部通过；日志「清空/下载」缺失 |
| 三条新增断言 | F1 通过、F2 通过、F3 未实测 |
| 端到端状态 | 走查前后 `/api/environments` 快照**逐字节相同**，现场已还原 |

---

## 1. 方法与工具限制

### 1.1 输入方式

`docs/acceptance/m3-workbench.md` 记录过「本环境 OS 级输入注入到达不了 Agent Window」。**本轮该限制未复现**：

- `bsk click` 走真实坐标（返回 `x`/`y`）并被页面接收，视图切换、统计卡筛选、页签切换、删除确认、启停按钮全部由真实点击完成；
- `bsk fill` 用于搜索框、端口字段、对比域名；
- `bsk evaluate` 只用于**读取**（计算样式、几何、DOM 结构、请求记录）与两处必要的页内埋点（剪贴板捕获、`fetch`/`XHR` 请求记录）。

因此本轮交互证据比此前几轮更接近真实用户操作。

### 1.2 四项测量限制（影响结论口径，务必连读）

| 限制 | 现象 | 对结论的影响 |
|---|---|---|
| **浏览器最小字号 12px** | 探针实测：`font-size: 10px` / `11px` / `11.5px` 的计算值**全部被钳成 `12px`**（该浏览器 profile 的用户设置） | 本轮**所有小于 12px 的字号结论一律取自 `app.css` 源码**，不是计算样式；因此 11 vs 11.5 vs 12 这类半档差异在本机不可观测 |
| **后台标签定时器节流** | 页内 `setTimeout(100ms)` 轮询实测被量化到约 1s 粒度；toast 实测 4006ms 消失（源码是 2600ms） | toast 自动消失时长、入场动画时长、SSE 快照间隔等**计时类数值本轮未实测**，报告只给源码值 |
| **截图不可用** | `bsk screenshot` 两次均 30s RPC 超时，未产出任何 PNG | 本文件全部证据为页内实测值（几何、计算样式、DOM 结构、请求计数），无截图 |
| **未制造异常态** | 经确认本轮不制造 `port_conflict` | 异常态三通道、重分配端口确认框在实机未取证（见第 6 节） |

### 1.3 现场还原

走查前保存 `/api/environments` 快照，中途通过 UI 启动过 `beta` 取运行态证据，结束后停止。最终 API 快照与初始快照**逐字节相同**（`beta` 16827 stopped、`stable` 16565 stopped、规则绑定与条数不变）。期间两次 `PATCH` 用非法端口（99999）是刻意的校验测试，服务端拒绝、未改变任何状态。

---

## 2. 逐项符合度矩阵

### A. 工程硬约束

| # | 断言 | 结果 | 证据 |
|---|---|---|---|
| A1 | JS 真的执行 | PASS | `documentElement.dataset.envboardReady === "yes"`，`dataset.envboardError` 未定义 |
| A2 | 无内联脚本 | PASS | `script:not([src])` 计数 **0**；全页唯一 `<script>` 是 `<script src="/app.js" defer>`；服务端 HTML 里 `<script` 字面量 1 处（即该外链标签） |
| A3 | 无内联样式 | PASS | `[style]` 属性 **0** 个、`<style>` 元素 **0** 个（服务端 HTML 与运行态 DOM 双向核对） |
| A4 | 无内联事件 | PASS | 遍历全部元素属性，`/^on/i` 命中 **0** 个 |
| A5 | 资产 3 文件 | PASS | 资源条目只有 `index.html` + `/app.css` + `/app.js`；favicon 是 `data:` URI（无 `favicon.ico` 请求，故无 404） |
| A6 | CSP 无 `unsafe-inline` | PASS | 响应头 `default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'` |
| A7 | 控制台零 CSP 报错、零未捕获异常 | PASS | `bsk console` 只有 3 条：1 条来自扩展自身的 `chrome-extension://invalid/`，2 条是本轮刻意提交非法端口得到的 400（已被应用捕获并转为 toast）。**无任何 CSP violation** |

### B. 设计令牌

| # | 断言 | 结果 | 证据 |
|---|---|---|---|
| B1 | 令牌与 `docs/design/tokens-light.css` 逐条一致 | PASS | 49 条（表面三级 + 边框两级 + 四级灰阶 + 主色/variant 三件套 + 5 组语义色三件套 + 间距 + 圆角 + 阴影 + 4 个布局常量）全部一致，**0 不符** |
| B2 | 四级灰阶对比度达标 | PASS | 自算对 `--panel` 的对比度：`--text` 17.46 / `--text-2` 9.39 / `--muted` 5.47 / `--muted-2` 4.59，与 `docs/design/ui-spec-full.md` 标称的 17.5 / 9.4 / 5.5 / 4.6 相符 |
| B3 | 组件层零十六进制色值 | PASS | `app.css` 第 90 行之后共 7 处十六进制命中，**全部**落在文件末尾被注释掉的 `[data-theme="dark"]` 块内，无生效规则 |
| B4 | 间距只用 4 的倍数 | PASS（有破例，见不符项 14） | 令牌 `--space-1…6`、`--space-8` 齐备且无 `--space-7`（与令牌文件一致）；有 5 处非基数硬编码 |
| B5 | 圆角同层级一致 | PASS | 按钮/输入/列表项 `6px`、卡片/面板 `10px`、品牌区 `14px`、chip `4px`、徽章 `999px`，与规范五档一一对应 |

### C. 布局与响应式

| # | 断言 | 结果 | 证据 |
|---|---|---|---|
| C1 | 四区栅格 | PASS | `grid-template-areas` 精确等于 `"topbar topbar" "sidebar main" "logbar logbar"`；`grid-template-rows` = `52px / 553.6px / 240px`；`grid-template-columns` = `260px / 1144px` |
| C2 | 布局常量 | PASS | topbar 实测高 **52**、sidebar 宽 **260**、logbar 高 **240**、折叠后 **36**（与 `--logbar-h-collapsed` 精确一致） |
| C3 | 应用形态（页面不滚、区内滚） | PASS | `body` 高 845.6 = `innerHeight` 846，`overflow: hidden`；`documentElement.scrollHeight === clientHeight`；`.main` / `#logs` / `.env-list` 各自 `overflow: auto` |
| C4 | 1200px 断点 | PASS | 1201px 时统计卡 4 列 / InfoGrid 3 列 / `.brand-sub` 可见；**1200px 时**统计卡 2 列 / InfoGrid 2 列 / `.brand-sub` 隐藏 |
| C5 | 900px 断点 | PASS | 902px 时仍是两栏；**900px 时**栅格变单列堆叠（`"topbar" "sidebar" "main" "logbar"`）、侧栏 `max-height` = 304px（= 38% × 800）、InfoGrid 降 1 列 |
| C6 | 640px 断点 | PASS | 640px 时统计卡 1 列、`seg-btn` 文字隐藏、`.log-search` 隐藏 |
| C7 | 无横向溢出 / G6（720px 全部功能可达） | **FAIL** | 见不符项 1：641–720px 区间顶栏溢出 1–79px，刷新按钮在 ≤681px 完全不可达 |

### D. 组件

| 组件 | 结果 | 关键实测值 |
|---|---|---|
| Button | 符合 | 五变体齐（primary/default/ghost/danger/icon-btn）；默认高 30、`.sm` 24、`.icon-btn` 30×30；`.primary` 背景 `rgb(42,95,232)` + 白字（`--accent-fg`）；`:disabled` opacity 0.45；`:active` 下移 1px；`.is-busy` opacity 0.6 + 图标呼吸。**每个视图可见的 primary 按钮数为 1**（环境视图 DOM 内 3 个，另 2 个在隐藏面板中，可见 1 个） |
| Badge | 部分符合 | 语义三件套配色正确（运行 `ok-bg`/`ok-border`/`ok` 等）；`.dot` 6×6、pill 圆角；running 圆点实测 `animation: pulse 2s infinite`。**高度实测 20px（规范 24）**，字号 11px（规范 11.5），字体是 UI 栈而非等宽（规范要求等宽） |
| Input | 符合（缺错误文本） | `.search-box` 与表单字段均高 30；底 `--bg-3`；focus 时转 `--panel` 白底 + `--accent` 描边 + `0 0 0 3px var(--accent-bg)` 光环。错误态见不符项 6 |
| Tabs | 符合（底色偏差） | 激活态文本 `--accent` + 底部 2px `--accent` 下划线（dpr 1.25 下计算值 1.6px 属设备像素对齐，非缺陷）；`role=tablist` 1 / `role=tab` 3 / `role=tabpanel` 3，`aria-selected` 同步。底色用 `--bg-3` 而非规范的 `--bg-2`（不符项 9） |
| Toast | 部分符合 | 白底 + 语义色 30% 描边 + `--shadow-lg` + 同屏上限 3；实测文案「代理命令已复制到剪贴板」、class `toast ok`。位置、最小宽、时长见不符项 11 |
| 日志行 | 符合 | 结构 = `.log-ts`（`--muted-2`）+ `.log-peer` + `.log-kind`（定宽 46px、语义色底/字）+ `.log-msg`（`--text-2`）；hover 底 `--panel-hover`；整体等宽。实例：`<div class="log-line k-req"><span class="log-ts"></span><span class="log-peer">127.0.0.1:61782</span><span class="log-kind">REQ</span><span class="log-msg">POST https://…</span></div>` |
| **Dropdown** | **缺失** | `[role=menu]` / `.dropdown` / `.menu` / `.more-menu` 命中数全为 0；`app.css` 无对应规则。见不符项 5 |
| **ActivityList** | **缺失** | `.activity` / `.activity-list` / `[data-activity]` 命中数 0；无「最近操作」区块。见不符项 5 |
| EmptyState | 部分符合 | 有 `#detail-empty` 与 `.empty-title`(13.5px) / `.empty-hint`(12px) / 居中 / CTA 按钮。图标处理与规范不同，见不符项 12 |
| StatCard | 部分符合 | 4 张卡；图标底 `--radius-sm` + `tint-*` 语义底色；数值等宽 + 700；hover 抬升 1px + `--shadow-sm`；点击筛选（`aria-pressed` 同步）。尺寸偏差见不符项 8 |
| InfoGrid | 符合 | `gap: 1px` + 底层 `--line` 形成缝隙网格；label `text-transform: uppercase` + letter-spacing；value 等宽。字号见不符项 8 |
| CmdCard | 符合 | 外层 `--bg-3`；命令块 `--panel` 白底 + 等宽 + `white-space: pre` + `overflow-x: auto`；复制按钮在 `.cmd-head` 内**常驻**（非 hover 才显） |
| RuleItem | 部分符合 | 名字 + 绑定状态 + 「载入」「删除」；`.is-missing` 有 warn 态。缺条数/IP 数，见不符项 3 |
| Icon | 部分符合 | 规格全部符合：`viewBox 0 0 24 24`、`stroke-width: 1.8`、`fill: none`、`stroke: currentColor`、三档 14/16/18（`.ic-sm`/`.ic`/`.ic-lg`）。基础集缺一项，见不符项 18 |

### E. 交互

| 交互 | 结果 | 证据 |
|---|---|---|
| 视图切换 | PASS | 三个 `seg-btn` 互斥，`aria-pressed` 与 `is-active` 同步；`view-rules` / `view-compare` 的 `is-hidden` 正确切换 |
| 统计卡筛选 | PASS | 点「运行中」→ `filter-bar` 由 `is-hidden` 变 `display:flex`、徽章文案「筛选：运行中」、列表过滤到 0 条并显示空态「没有符合条件的环境。」、`aria-pressed` 唯一；再点一次取消（`filter-bar` 回到 `is-hidden`、列表恢复 2 条） |
| 侧栏搜索 | PASS | 匹配 name / port / rules / description 四字段；清空按钮按需出现 |
| 详情三页签 | PASS | `aria-selected`、`aria-controls`、面板互斥正确 |
| 复制代理命令 | PASS（F2） | 见下 |
| 删除环境二次确认 | PASS（F1） | 见下 |
| 规则载入 | PASS | 触发 `GET /api/rules/hw-prod-blue`，回填 348 字符规范化原文（带生成器注释头），提示写明字符数；表单内容刻意保留 |
| 规则删除确认 | PASS（措辞待补） | 就地确认「删除规则文件 hw-prod-blue？」+ 确认删除/取消；未确认时 DELETE 计数 0 |
| 跨环境对比 | PASS | 汇总「devapi-km.wps.cn：2 个环境里 1 个覆盖它」；行 `.is-covered` 用 `ok-bg`/`ok-border`/`ok`，`.is-uncovered` 用默认底；本查询纯查表不发外部请求 |
| 日志事件类型过滤 | PASS | 6 个 chip（全部/请求/响应/连接/运行/异常），实测计数 195/44/44/75/15/17；选中 chip 加 `is-active`；计数显示「44/195」 |
| 日志搜索高亮 | PASS | 输入 `GET` → 26 个 `<mark>`，底色 `rgba(42,95,232,0.1)`、字色 `rgb(29,78,216)`（= `--accent-2`）；清空按钮恢复 |
| 暂停跟随 | PASS | `aria-pressed` true→false、图标 `#i-pause`→`#i-play`、标题变为「继续跟随：日志恢复自动刷新」 |
| 自动滚动开关 | PASS | `aria-pressed` 同步切换 |
| 日志栏折叠 | PASS | `body.log-collapsed`、logbar 高度 240→**36**、`display:none`、`aria-expanded` false；再点恢复 240 |
| 编辑表单锁定 | PASS | beta 为 stopped 时四字段（name/port/rules/description）全部可编辑；进入编辑后页签自动切到「配置」、标题「编辑环境 · beta」、按钮变「保存」、出现「取消编辑」、模式徽章显示 |
| 异步操作 loading | PASS | 点「启动」后同一 tick 内按钮为 `btn primary is-busy` 且 `disabled === true` |
| 错误输入态 | 部分符合 | 见不符项 6 |
| 无障碍 | PASS | `skip-link` 存在；`#logs` 为 `role="log"` + `aria-live="off"`；`#toasts` 为 `role="status"` + `aria-live="polite"`；20 处 `aria-label`、15 处 `aria-pressed`；`:focus-visible` 用 2px `--accent` outline；旧版表格（含 `th scope` 问题）已被卡片式列表整体取代，`table` 计数 0 |
| `prefers-reduced-motion` | 静态通过 | `app.css` 有 `@media (prefers-reduced-motion: reduce)` 块，把动画/过渡压到 0.001ms；**运行时无法模拟**（`bsk emulate` 只覆盖 viewport/UA/touch） |
| 每秒整表重建（旧缺陷回归检查） | PASS | 运行态下 8 秒内 mutation 仅 8 批且**全部落在 `env-form-port-hint`**；`#detail-actions` 按钮、`.env-item`、`.log-line` 的节点身份标记全部存活 → 无「每秒重建抢光标」回归。副作用见不符项 17 |

### F. 规范要求但此前从未取证的三条断言

| # | 断言 | 结果 | 证据 |
|---|---|---|---|
| F1 | 确认框真实拦截（未确认时 API 不被调用） | **PASS** | 于页内接管 `fetch` 与 `XMLHttpRequest.open` 记录全部请求；点「删除环境」后确认行出现（「删除 beta？实例会先被停掉；规则文件本身不受影响。」+「确认删除」`btn danger sm` +「取消」`btn ghost sm`），此期间非 GET 请求数 **0**、DELETE 计数 **0**；点「取消」后确认行消失、操作行复原为「启动/重启/更多操作」，`/api/environments` 对拍环境仍在 |
| F2 | 复制后剪贴板内容与 `proxy_command` 一致 | **PASS** | `isSecureContext === true`，捕获到写入**经由 `navigator.clipboard`**；捕获值 `export https_proxy=http://127.0.0.1:16827 http_proxy=http://127.0.0.1:16827` 与概览命令块、`GET /api/environments` 的 `proxy_command` 三者**完全一致**；同时弹出 toast「代理命令已复制到剪贴板」 |
| F3 | `config_mismatch` 环境下建议动作可见且可点 | **未实测** | 本轮经确认不制造异常态，工作台上 2 个环境全程健康，无法构造该分支（见第 6 节） |

---

## 3. 不符项清单

严重度：**高** = 可用性受损或会让用户误操作；**中** = 规范承诺的功能缺失、用户可感知；**低** = 尺寸/一致性偏差。

### 高

#### 1. 顶栏在 641–720px 溢出，刷新按钮不可达（违反 G6）

| viewport | 顶栏 `scrollWidth` | 溢出 | 刷新按钮可见宽度 |
|---|---|---|---|
| 721px | 722 | 0 | 30（完整） |
| **720px** | 721 | **1** | 29 |
| 710px | 721 | 11 | 19 |
| 700px | 721 | 21 | 9 |
| **681px 及以下** | 721 | ≥40 | **≤0（完全移出视口）** |
| 660px | 721 | 61 | −31 |
| 641px | 721 | 79 | −49 |
| **640px** | 640 | **0** | 30（完整） |

- **证据**：顶栏的固有最小宽度恒为 **721px**；在 720px 及以下 `scrollWidth - clientWidth` 持续为正，刷新按钮 `right` 恒定停在 721px，故其可见宽度随视口收窄而变小，约 681px 处变为负值。640px 触发窄屏断点（`seg-btn` 文字与日志搜索框隐藏）后才重新归零。
- **为什么不可达**：`core/rs/crates/envboard-web/assets/app.css` 的 `body { overflow: hidden }`，溢出内容被裁切且**没有横向滚动条可以滚过去**。
- **影响**：`docs/design/ui-upgrade-plan.md` 的 G6 要求「720px 宽下全部功能可达，无横向溢出」——在 720px 已差 1px，在 641–680px 区间则「刷新」按钮完全不可点击。
- **建议**：把窄屏断点从 640px 提到 720px；或给 `.topbar` 加 `flex-wrap` / 允许压缩（如 720px 以下隐藏 `conn-badge` 或让 `view-switch` 只留图标）。

#### 2. 重分配端口无二次确认，直接调用 API（违反 G1 与两份规范）

- **证据**：`core/rs/crates/envboard-web/assets/app.js:523-534` —— 仅当 `env.health === "port_conflict"` 时渲染「重分配端口」按钮，其 `onClick` 直接 `mutate(…/reallocate, "POST")`，**没有** `state.confirm` 环节。对照同文件 `413-443` 行，删除环境是走就地确认的。
- **影响**：`docs/design/ui-upgrade-plan.md` 的 G1 要求「全部破坏性操作（删除环境 / 删除规则 / 重分配端口）带确认且说明后果」，P0-2 更明确点名这条；`docs/design/ui-spec-full.md` 第 8 节第 3 条要求「破坏性操作必须二级确认并写明后果」。
- **放大因素**：同文件 `358` 行的建议文案在 `port_conflict` 时**主动把用户导向这个动作**：「建议：用「更多操作 → 重分配端口」…」，而该动作一旦执行，客户端此前 `export https_proxy=…` 立即失效且界面没有任何提示 —— 这正是 P0-2 描述的原始问题，尚未修复。
- **未实测说明**：该按钮仅在 `port_conflict` 状态出现，本轮未制造冲突，故结论来自源码直读而非实机复现。

#### 3. 状态标签仍是原样英文值，三种「坏」不可区分（违反 D1 标签列与 P0-5）

- **证据**：`core/rs/crates/envboard-web/assets/app.js:159-170` 的 `healthBadge` 把 `env.health` 原文当作徽章文案；实测徽章文本为 `running` / `stopped`。同文件 `19-26` 行的 `HEALTH_STYLE` 只做「状态 → CSS class」映射，`config_mismatch`、`port_conflict`、`failed` **三者映射到同一个 `error` class**。概览 InfoGrid 同样是 `状态=running`、`期望=running` 两个独立单元格的原样值。
- **影响**：D1 定义的三元组只落地了「视觉」；七行中文标签表（运行中/已停止/启动中/不健康/配置未生效/端口冲突/启动失败）一条未实现。P0-5「三种坏无法从颜色区分，且仅靠颜色」未解决；P0-6「期望 vs 实际不对比呈现、需要人肉比对两列」在结构上未变（只是从表格两列挪到网格两格）。
- **已落地的部分**（应记录）：P0-4 已解决 —— 原因不再是 `title` 悬浮，而是行内 `.notice`（`app.js:548-563` 的 `renderReason`）；建议动作有 `ADVICE` 映射（`app.js:355-360`），但**仅当 `health_reason` 非空时**随原因条一起出现。
- **建议**：引入 `HEALTH_META = { 标签, 视觉, 建议动作 }` 表替换 `HEALTH_STYLE`，徽章文案取「标签」；`health_reason` 为空时也应对异常态给出原因占位。

#### 4. 规则库不显示条数与 IP 数（违反 P1-2 与 M5-b 第 3 条）

- **证据**：4 个规则项只渲染「名字 + 被 X 绑定 / 未被绑定 + 载入 + 删除」。实测 `beta` 规则是 **536 条 / 75 个数据行**（`GET /api/rules/beta` 返回 75 行 `IP 域名` 记录，环境列表显示 `beta (536)`），但规则库列表上这两个数字都看不到。「载入」只报**字符数**（「已载入 hw-prod-blue（348 字符）」）。
- **影响**：无法在列表里判断某规则是否值得绑定，只能逐个载入；与 `docs/design/ui-upgrade-plan.md` 的「实测单规则 536 条，不可见即不可信」正好相反。

### 中

#### 5. 规范列明的 Dropdown 与 ActivityList 两个组件不存在

- **证据**：`[role=menu]`、`.dropdown`、`.menu`、`.more-menu`、`.activity`、`.activity-list`、`[data-activity]` 命中数全为 **0**；`app.css` 中无对应规则。
- **现状替代**：低频操作收在「更多操作」按钮里，展开为**就地一行** `.detail-more`（`app.css:373-381` 注释说明刻意不用浮层，理由是 CSP 禁内联 style、浮层在滚动容器里会被裁切）。这是合理取舍，但与 `docs/design/ui-spec-full.md` 第 1.2 节把 Dropdown 与 ActivityList 列为「新增组件」、第 7 节给出 Dropdown 样式规范不符；「⋯」图标（`#i-more`）已定义却未用于该入口（按钮图标在 `more`/`chevron-down` 间切换）。
- **建议**：要么把这两个组件从规范里删掉并在规范中记录取舍理由，要么补齐。

#### 6. 字段级错误文本缺失，错误文案是开发向的

- **证据**：端口填 `99999` 提交后，服务端拒绝（状态未变），前端表现是：`aria-invalid="true"`、边框 `rgb(201,42,46)`（`--bad`）、光环 `0 0 0 3px rgba(201,42,46,0.1)`（`--bad-bg`）、焦点落到该字段 —— 说明 `markInvalid`（`app.js:963-968`）确实执行了。但字段容器内只有 `SPAN.field-label` / `INPUT` / `SPAN.field-hint` 三个子节点，**没有错误文本节点**。
- **规范要求**：`docs/design/ui-spec-full.md` 第 7 节「输入框」写的是「错误：描边转 `--bad` + 下方 11.5px `--bad` 错误文本」。
- **影响**：异常态的三通道（颜色 / 行内原因 / 徽章）在**表单场景只有「颜色 + toast」两通道**，行内原因缺失；而 toast 文案是 `invalid_config: listen.port 99999 is out of range (1..=65535)（字段：environment.listen.port）` —— 原始错误码加规范化配置路径，不是面向用户的表述。同时非法值原样留在输入框里。

#### 7. 日志栏缺「清空」与「下载」

- **证据**：`core/rs/crates/envboard-web/assets/index.html:344-352` 的 `.log-actions` 只有 `#log-follow` 与 `#log-autoscroll`；运行态实测按钮 id 列表为 `["log-follow","log-autoscroll"]`。
- **规范要求**：`docs/design/ui-spec-full.md` 第 1.3 节的排障链路写「级别过滤 → 关键词搜索（命中高亮）→ 自动滚动开关 → **清空**」；`docs/design/ui-upgrade-plan.md` 的 M5-b 第 4 条要求「跟随 / 暂停、按级别过滤、**下载**、**清空**」。
- **现状**：跟随/暂停 ✓、搜索高亮 ✓、自动滚动 ✓、清空 ✗、下载 ✗。

#### 8. 组件尺寸与规范存在系统性半档偏差（12 处）

| 组件 | 实测/源码值 | 规范值 | 出处 |
|---|---|---|---|
| `.badge` 高度 | **20px**（实测） | 24 | `docs/design/ui-spec-full.md` 第 7 节「状态徽章」 |
| `.badge` 字号 / 字体 | 11px / UI 栈 | 11.5px / 等宽 | 同上 |
| `.stat-icon` | **34×34** | 38px | 第 7 节「StatCard」 |
| `.stat-value` | **20px** | 22px | 同上 |
| `.stat-label` | 11.5px（源码） | 12px | 同上 |
| 输入框高度 | **30px**（`.search-box`、`.form input/select/textarea`） | 32 | 第 7 节「输入框」 |
| `.btn.sm` 字号 | 11.5px（源码） | 12 | 第 7 节「按钮」 |
| `.btn.icon-btn.sm` | 26×26 | 30×30（sm 高 24） | 同上 |
| `.cmd-text` | 12px（源码） | 12.5px | 第 7 节「CmdCard」 |
| `.info-label` | 11px（源码） | 11.5px | 第 7 节「InfoGrid」 |
| `.info-value` | 13.5px（源码） | 14px | 同上 |
| `.log-kind` 定宽 | **46px** | 44px | 第 7 节「日志行」 |
| `.empty-title` | 13.5px | 13px | 第 7 节「EmptyState」 |

（粗体为可用计算样式直接实测、不受最小字号钳制影响的项。）单处 1–4px，但累加后削弱了规范「同一层级必须一致」的意图。

#### 9. Tab 条底色用 `--bg-3` 而非 `--bg-2`

- **证据**：`.tabs` 计算背景色 `rgb(248,250,252)` = `#f8fafc` = `--bg-3`（`app.css:396-401`）。
- **规范要求**：`docs/design/ui-spec-full.md` 第 7 节「标签页」写「Tab 条底色 `--bg-2`，与内容区 `--panel` 形成一层弱区分」。

#### 10. EmptyState 图标处理与规范不同

- **证据**：`.empty-icon` 实测 48×48、圆角 10px、底色 `--panel-2`、内层 `.ic-lg` 18×18、**opacity 1**（`app.css:709-715`）。
- **规范要求**：`docs/design/ui-spec-full.md` 第 7 节「EmptyState」写「40px 图标（50% 透明）」。

### 低

#### 11. Toast 位置、最小宽与时长

- **位置**：`app.css:814-823` 是 `position: fixed; right: 16px; bottom: calc(var(--logbar-h) + 16px)`，即**右下、日志栏上方**（实测 right 16 / bottom 256）。`docs/design/ui-spec-full.md` 第 7 节写「顶栏下方右侧堆叠」，而 `docs/design/ui-upgrade-plan.md` 第 1.3 节写「右下堆叠」——**两份规范互相冲突**，实现依据后者（见第 5 节）。
- **最小宽**：规范写「最小宽 240」，实现只有 `max-width: 380px`，`min-width` 计算值为 `auto`。
- **时长（未实测，仅源码）**：`app.js:173` = `{ ok: 2600, info: 2600, bad: 6000 }`，规范写 2.5s（ok/info 相符，错误类刻意延长到 6s，规范未提）；入场动画 `toast-in 0.18s`，规范写 0.2s。

#### 12. 窄屏不是「抽屉」，日志栏未加高

- **证据**：≤900px 时 `body` 变单列堆叠，`.sidebar` 加 `max-height: 38vh`（900px 宽时实测 304px），日志栏高度**恒为 240px**。
- **规范要求**：`docs/ui-spec.md` 第 3 节写「窄屏收侧栏为抽屉、统计卡降为两列/单列、日志栏加高」。统计卡降列符合，抽屉与加高未做。

#### 13. 间距基数破例 5 处

`gap: 5px`（`.badge`，`app.css:547`）、`gap: 3px`（`.info-cell`，`app.css:426`）、`gap: 2px`（`.env-main`，`app.css:236`）、`padding: 3px`（`.view-switch`，`app.css:164`）、`padding: 6px`（`.form input/select/textarea`，`app.css:480`）。`docs/ui-spec.md` 第 2 节要求「只用 `--space-1…8`，不出现 6px、15px 这类孤立值」——**`6px` 正是该节点名的反例值**。

#### 14. 顶栏未展示监听地址与鉴权状态；无「最后更新时间」

- **证据**：页面文本不含 `8900`，不含 token/鉴权字样；`.sidebar-foot` 只显示 `mitmproxy 12.2.3 · /home/lolioy/.local/state/envboard`（核心版本 + 状态目录）。全页无「最后更新 / 更新于 / 上次刷新 / 数据时间」字样。
- **对应目标**：`docs/design/ui-upgrade-plan.md` 的 P2-7（顶栏暴露面显示）与 P1-9（数据新鲜度指示）。

#### 15. 日志栏无日志时无空态

选中 `stable`（无日志）时 `#logs` 子节点数 **0**、文本为空，只有计数器显示 `0/0`；日志区没有空态说明。规范第 1.2 节把 EmptyState 列为通用组件，此处未应用。

#### 16. 每秒一次无意义的 DOM 写入

- **证据**：运行态 8 秒内共 8 个 mutation 批次，**全部落在 `env-form-port-hint`**；对应 `app.js:210-211` 每秒把同一段文案重新赋给 `textContent`（`renderChrome` 不做变更判断）。
- **影响**：无可见缺陷，但它是 SSE 每秒快照下唯一未被「数据签名」挡住的重绘点，属噪声；同为每秒快照的侧栏、详情、日志都已被签名比较挡住（见 E 表最后一行）。

#### 17. 图标基础集缺「设置」

- **证据**：`index.html` 共 24 个 `<symbol>`：`layers, rules, compare, plus, search, refresh, copy, play, stop, restart, edit, trash, log, more, chevron-down, close, check, alert, info, pause, arrow-down, server, shuffle, terminal`。
- **规范要求**：`docs/design/ui-spec-full.md` 第 6 节的基础集列出「新建、搜索、刷新、复制、启动、停止、删除、**设置**、**文件**、警告、更多」。「设置」（齿轮）缺失；「文件」由 `i-rules`（文档形）近似承担。

#### 18. 计数 chip 无激活态；详情页签不带计数

- **证据**：`.count`（`app.css:572-580`）只有固定 `--panel-2` 底 + `--muted` 字，无 `is-active` 变体；详情页签（概览/配置/规则）不带计数 chip。有激活态的是日志过滤用的 `.chip`。
- **规范要求**：`docs/design/ui-spec-full.md` 第 7 节「标签页」写「计数 chip 用 `--panel-2` 底 / 4px 圆角，**激活时转 `--accent-bg` 底 + `--accent` 字**」。

#### 19. 规则删除确认未写后果

- **证据**：规则删除的确认文案是「删除规则文件 hw-prod-blue？」，只有动作没有后果；对照环境删除写明了「实例会先被停掉；规则文件本身不受影响。」被绑定规则（如 `beta`）删除时，界面也不提示哪些环境受影响。
- **规范要求**：`docs/design/ui-spec-full.md` 第 8 节第 3 条「破坏性操作必须二级确认并写明后果」。

#### 20. 侧栏计数是总数而非筛选结果数

- 筛选「运行中」后列表为空并显示空态，但 `#env-count` 仍显示 `2`（`app.js:230` 赋的是 `state.environments.length`）。规范未定义该语义，两种解释都成立，列出供决策；若定位为「列表条数」则应在筛选后同步。

#### 21. 「已复制」按钮反馈缺失

- **证据**：`app.js:833-854` 的 `copyText` 只发 toast，不改按钮；实测点击后按钮文案仍为「复制」、class 仍为 `btn sm`、图标仍为 `#i-copy`，无瞬时反馈态。
- **规范要求**：`docs/design/ui-spec-full.md` 第 1.3 节写「代理命令一键复制，按钮即时反馈「已复制」+ Toast」。

---

## 4. 规范文档自身的不一致

以下**不作为实现缺陷**，列此供规范维护者裁决。

| # | 不一致 | 说明 |
|---|---|---|
| C1 | 详情页签数量 | `docs/design/ui-spec-full.md` 第 1.1 节写「多标签页：概览 / 配置 / 规则 / **日志**」（4 个），实现是 3 个。实测日志栏**随选中环境切换**（选中 `stable` 后 `#logs-env` 变为 `stable`），即「日志」由常驻日志栏承担，功能可达、入口不同。建议把规范改回 3 个页签并说明日志入口在底部常驻栏 |
| C2 | 日志过滤维度 | `docs/design/ui-spec-full.md` 第 1.1 / 第 7 节写「级别过滤」与「级别色仅四值 INFO/SUCCESS/WARN/ERROR」；`docs/ui-spec.md` 第 4 节**明确否决**按级别过滤（「实测日志行不带 INFO/WARN 前缀，按级别过滤是个假功能」）。实测日志行确无级别字段，实现按事件类型分 6 类。以 `docs/ui-spec.md` 与数据源为准，实现正确 |
| C3 | 日志行时间戳 | `docs/design/ui-spec-full.md` 第 7 节「日志行」把「时间戳」列为每行必备结构；实测数据源只在 `connect` 行写时间戳（47 行中 42 行时间戳为空，如 `127.0.0.1:55287: POST https://…` 与 `     << HTTP/2.0 200 OK 419b`）。实现用空 `span.log-ts` 占位以保持列对齐，属如实呈现。规范描述与数据源不符 |
| C4 | 搜索命中底色 | `docs/design/ui-spec-full.md` 第 7 节写「`--accent` **25%** 底」；同文件第 2 节的透明变体规则只定义 10%。实现用 `--accent-bg`（10%，`app.css:811`）。按第 2 节为准则实现正确 |
| C5 | Toast 位置 | `docs/design/ui-spec-full.md` 第 7 节写「顶栏下方右侧」，`docs/design/ui-upgrade-plan.md` 第 1.3 节写「右下堆叠」。实现依据后者 |
| C6 | 令牌文件与规范正文 | `docs/design/tokens-light.css` 未定义 `--radius-xs`（4px）与 `--accent-fg`（`#ffffff`），而 `core/rs/crates/envboard-web/assets/app.css` 第 ① 区两者都有并被使用；`docs/design/ui-spec-full.md` 第 5 节只把最小圆角描述为「4」而未给令牌名。建议补齐令牌文件 |
| C7 | 深色块用纯黑阴影 | `core/rs/crates/envboard-web/assets/app.css:932-946` 保留的深色值用 `rgba(0,0,0,…)` 阴影，与 `docs/design/ui-spec-full.md` 第 5 节「禁止纯黑阴影」相悖。该块当前为注释态、不生效，但一旦启用即违规 |
| C8 | 资产行数软上限 | `docs/design/ui-upgrade-plan.md` 第 8 节给前端资产设了约 1200 行软上限（超出则应拆多个外置脚本）；实测 `app.js` **1562 行**、`app.css` 946 行、`index.html` 366 行，已超出。硬约束（资产 3 文件）未破 |
| C9 | 规范依据不可解析 | `docs/design/ui-spec-full.md` 开头把依据写成 `envboard-ui-demo/index.html` 与外部画板「Ardot」。前者在包内与全机均不存在，后者是包外在线协作画板；包内文本自包含门禁（`scripts/doc_scope_lint.py`）不会捕获这一条，因为该路径不以门禁识别的目录前缀开头 |

---

## 5. 未实测清单

| 项 | 原因 |
|---|---|
| 异常态三通道（`config_mismatch` / `port_conflict` / `failed` / `unhealthy`） | 本轮经确认不制造异常。徽章配色、`.notice.warn` / `.notice.danger` 分支、`ADVICE` 文案在实机未取证；源码路径已核对（`app.js:548-563` 与 `355-360`） |
| 重分配端口的确认框 | 该按钮仅在 `health === "port_conflict"` 时渲染，未触发；不符项 2 的结论来自源码直读 |
| `prefers-reduced-motion` 运行时表现 | `bsk emulate` 只覆盖 viewport / UA / touch，无法设置媒体特性；仅静态核对到 `app.css` 的降级块存在 |
| 计时类数值 | Agent Window 为后台标签，定时器被节流到约 1s 粒度（页内 100ms 轮询被量化、toast 实测 4006ms 消失而源码为 2600ms），故 toast 时长、动画时长、SSE 间隔只给源码值 |
| 小于 12px 的字号 | 该浏览器 profile 有最小字号 12px 设置，10 / 11 / 11.5px 计算值一律为 12px；此类结论全部取自 `app.css` 源码 |
| 截图证据 | `bsk screenshot` 两次均 30s RPC 超时，未产出 PNG；本文件全部证据为页内实测值 |
| G5（≥20 个环境时定位 ≤3 次交互） | 工作台当前只有 2 个环境。可得的替代证据：搜索覆盖 name/port/rules/description 四字段，筛选覆盖状态维度，「搜索 + 筛选」满足「三选二」的目标；排序未实现 |
| G4（UI 覆盖 13/13 端点） | 未逐端点穷举。已确认 UI 会调用 `/api/status`、`/api/environments`、`/api/environments/:name`（GET/PATCH/DELETE/POST 启停等）、`/api/environments/:name/logs`、`/api/rules`、`/api/rules/:name`、`/api/events`（SSE）、`/api/compare` |

---

## 6. 复现命令

```bash
# 0) 确认被测的是本仓当前代码
md5sum ~/.local/bin/envboard packages/envboard/target/release/envboard
md5sum packages/envboard/core/rs/crates/envboard-web/assets/*
curl -s http://127.0.0.1:8900/app.css | md5sum   # 应与上一行的 app.css 相同

# 1) 起浏览器会话（Agent Window，不打扰用户标签页）
bsk browsers
bsk session start --no-focus --browser <instance-id>        # 记下 4 字母 session id
bsk navigate http://127.0.0.1:8900 --session <id>
bsk window resize --width 1464 --height 950 --session <id>   # 视口变成 1404x846

# 2) 三条基础断言 + 工程硬约束
bsk evaluate --session <id> '({ ready: document.documentElement.dataset.envboardReady,
  boot: document.documentElement.dataset.envboardError ?? null,
  inlineScripts: [...document.querySelectorAll("script")].filter(s => !s.getAttribute("src")).length,
  styleAttrs: document.querySelectorAll("[style]").length,
  styleEls: document.querySelectorAll("style").length })'
bsk console --session <id>

# 3) 令牌对拍（把 docs/design/tokens-light.css 的值写成期望表后逐条 getPropertyValue 比较）
bsk evaluate --session <id> 'getComputedStyle(document.documentElement).getPropertyValue("--bg")'

# 4) 响应式溢出（不符项 1）
for W in 760 720 700 681 660 640; do
  bsk emulate --width $W --height 800 --session <id> --quiet
  bsk evaluate --session <id> '({ vw: innerWidth,
    overflow: document.querySelector(".topbar").scrollWidth - document.querySelector(".topbar").clientWidth,
    refreshVisible: Math.round(Math.min(document.getElementById("refresh").getBoundingClientRect().right, innerWidth)
                              - document.getElementById("refresh").getBoundingClientRect().left) })'
done
bsk emulate --off --session <id>

# 5) F1 确认真实拦截（未确认时不得发出 DELETE）
bsk evaluate --session <id> '(() => { window.__reqs = [];
  const of = window.fetch; window.fetch = function (...a) { window.__reqs.push({ url: String(a[0].url || a[0]), m: ((a[1]||{}).method||"GET").toUpperCase() }); return of.apply(this, a); }; })()'
# …点「更多操作 → 删除环境」，断言 window.__reqs 中 DELETE 计数为 0，再点「取消」，最后用 /api/environments 对拍

# 6) F2 剪贴板内容对拍
bsk evaluate --session <id> '(() => { window.__cap = null;
  navigator.clipboard.writeText = async (t) => { window.__cap = t; }; })()'
bsk click --selector '#copy-cmd' --session <id>
bsk evaluate --session <id> '({ captured: window.__cap, matches: window.__cap === document.getElementById("ov-cmd").textContent })'

# 7) 收尾
bsk session stop <id>
curl -s http://127.0.0.1:8900/api/environments | python3 -m json.tool   # 与走查前快照对拍
```

---

## 7. 本轮未做的事情

- 未改动 `core/rs/crates/envboard-web/assets/` 下任何一个文件；
- 未改动 `docs/design/`、`docs/ui-spec.md`、`docs/acceptance/m3-workbench.md` 等既有文档；
- 未改动 REST / SSE 契约、CSP 形态、Cargo 依赖与任何 Rust 代码；
- 未执行任何破坏性操作：删除环境与删除规则的确认框一律点了「取消」，端口重分配未触发，未删除或导入任何规则文件；
- 未改动 `CHANGELOG.md`（本轮只产出本验收记录，是否补变更日志条目由后续决定）。

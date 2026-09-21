# bsk 验收轮 2 · 真实 API 全路径回归结论（2026-09-21）

对象：`http://127.0.0.1:5199/`（vite dev，`ENVBOARD_PROXY=http://127.0.0.1:8901` 指向**临时联调实例**，
state 目录独立于用户真实 `~/.envboard`，零污染）。
数据：规则集 beta（4 条）、上游代理 corp（带鉴权）、环境 dev（运行）/ stage（运行，绑 corp）。
截图：`shots/round2/`。

## 走查结果（全绿）

| 检查 | 结果 | 证据 |
|---|---|---|
| 环境列表真实渲染 | ✅ | dev/stage 两行，健康六态、监听端口 16988/16135 真实值 |
| SSE 快照流实时性 | ✅ | 外部 `POST /start` 启动 stage，UI 自动变「运行中 2」（无手动刷新） |
| 抓包端到端 | ✅ | 开启抓包 → curl 经代理 2 请求 → 实时会话计数 2、GET 200/POST 201、真实大小（364B/417B） |
| 抓包详情真实头部 | ✅ | 请求头 4 条（host/user-agent/accept/proxy-connection），非派生 mock |
| 新建环境联动 | ✅ | 探针环境 probe 即时出现在目标下拉与列表（SSE 驱动） |
| 规则库被引用 | ✅ | beta 被 dev、stage 引用（实时来自环境绑定，非静态账） |
| 上游代理真实态 | ✅ | corp `proxy.corp.example.com:3128 · 鉴权已配置`、被 stage 引用 → 删除禁用 |
| 跨环境对比 | ✅ | api.example.com → dev/stage 解析 10.0.0.11「是」、probe「不覆盖/否」 |
| 活动审计 | ✅ | 12 条真实事件（导入规则集/创建环境/启动实例…），今天分组 |
| 设置 CA 摘要 | ✅ | 真实 ECDSA with SHA-256、not_before 2026-09-20、序列号、下载地址（含 token 逻辑） |
| 概览代理命令 | ✅ | 服务端回显 `export https_proxy=http://127.0.0.1:16988 …`（前端不重算） |
| console 页面 error | ✅ | 0（chrome-extension:// 噪音除外） |

## 本轮修复

- **DebugView 会话计数 bug**：events 帧的 `captured/dropped` 未合并进状态（chip 显示「已捕获 0」）；
  `mergeRecords` 现同步两计数。

## 判定

**轮 2 通过**。C2 完成，进入 B（后端 Admin 重构）。

# envboard

多环境代理管理器：**一个环境 = 一个进程内引擎实例 + 一个端口**。客户端把代理指到
`127.0.0.1:<环境端口>` 就是在用那个环境，所以多个环境可以**同时活着**、直接对比。

数据面是**纯 Rust 进程内引擎**（`envboard-engine`）：没有子进程、没有轮询、没有第二种
语言 —— 配置生效塌缩成一次**同步装配**，PATCH 返回后的第一个请求就是新配置。

规则是 hosts 风格的静态覆盖表，在**建连时改写上连目标** —— 客户端看到的一切
（Host 头、SNI 基准）都不变，只有"出网连到哪"变了。

---

## 现在能用什么

| 能力 | 状态 |
|---|---|
| 环境 CRUD / 编辑 / 启停 / 显式重分配端口 / 规则绑定与热重载 | ✅ |
| **热应用同步生效**：PATCH 返回后的第一个请求就是新配置（无轮询、无收敛窗） | ✅ |
| 按域名放宽上游证书校验（`insecure_hosts`：精确匹配，运行中改立即生效） | ✅ |
| 端口自动分配（区间内随机试绑，新建冲突自动重试一次） | ✅ |
| **上游代理（二级代理）**：按名管理的代理账本 + 环境按名绑定（热字段）；TLS 目标走 CONNECT 隧道（内层 TLS 与 `insecure_hosts` 语义不变），明文 http 按 absolute-URI 转发；删除被引用的代理拒绝并点名 | ✅ |
| 按 host 改写上连目标（hosts 规则，只改 `ConnectTarget.resolved_addr`） | ✅ |
| 请求轨迹（每请求一条 append-only 事件流，SSE 实时跟随） | ✅ |
| 抓包（请求/响应详情，实例内存会话：默认关、热开关、停止/重启即丢弃、HAR/JSONL 导出） | ✅ |
| 调试页（工作区级调试会话单例：切换环境即停旧+清旧+开新）+ HAR 导入会话（多个并存，只读）+ 抓包实时推送（`/api/debug/stream`） | ✅ |
| 控制面审计事件（权威仍在 state.json；`GET /api/history` + 工作台「活动」） | ✅ |
| CONNECT 隧道 + MITM 按 SNI 现签 + HTTP/1.1 缓冲转发；absolute-URI 正向代理；101 透传 | ✅ |
| 代理访问鉴权（`proxy_user` / `proxy_password` → 407 门；**凭据只在内存**，不进 argv / 视图 / SSE） | ✅ |
| 共享 CA（所有实例一张；兼容加载 mitmproxy 形状的同名 CA 文件，已装证书的客户端零感知） | ✅ |
| 规则库账本（`state.json` 的 `rules[]` 是唯一真相；物化文件可再生；启动对账回填） | ✅ |
| 期望状态 reconcile（引擎线程 panic → `failed` → 按 desired 自动重启 = 崩溃自愈） | ✅ |
| 工作台（环境 / 规则库 / 跨环境对比 / 活动 / 设置；详情含轨迹页签、可折叠日志栏、SSE 每秒快照） | ✅ |
| 工作台鉴权档位（回环默认免鉴权；非回环必须显式 `--token`；header 与 `?token=` 等效） | ✅ |
| 对外服务开关（环境监听 `0.0.0.0`，默认 `127.0.0.1`） | ✅ |
| 单实例锁、状态原子写 + 0600 | ✅ |
| HTTP/2 客户端（gRPC 等） | ❌ 数据面只实现 HTTP/1.1，见「已知限制」 |
| PAC / 透明代理 / SOCKS | ❌ 未做 |

一切管理动作的入口 = 工作台 UI 或本地 HTTP API（见「HTTP 端点」）。判定只在服务端做
一次，界面与脚本是两个翻译面，语义严格同一份。

## 架构

**引擎进程模型**：一个环境实例 = 一个 OS 线程 + 一个 current-thread tokio runtime +
一个监听端口，全部活在 envboard 这一个进程里。绑定即真相（`EADDRINUSE` →
`port_conflict`，不试绑、不换端口）。崩溃隔离是**任务级**的：单条请求任务 panic 只死
那一条连接；引擎线程整体 panic 才让实例进入 `failed`，由 reconcile 按期望状态拉回。
这要求 release 构建**保持 unwind**（`panic = "abort"` 会让任务级隔离失效）—— 属构建契约。

**内置能力**：监听、407 门、TLS 策略、共享 CA、协议面是引擎原生快路径；hosts 规则
改写（connect 路径直调修订 `resolved_addr`，规则脏数据 fail-closed 502）与请求终局
日志（有界总线投递，写失败丢行不阻断流量）是引擎内置步骤，不是可插拔接口。
逐项语义以 spec/capabilities.md 的「引擎能力矩阵」为准。

**ConnectTarget 是唯一接缝**：`authority / sni / resolved_addr / tls_policy /
chained_proxy`（恒 None 的扩展位）。改写语义只有一件事：不动请求内容、不动 Host 头、
不动 SNI 基准，只改"连到哪"。

**同步热装配**：管理器把归一化环境字段 + 规则账本 rendered 编译成 `EngineSpec`，
`ProxyEngine::apply` 经 **ArcSwap 原子换入**新快照并回执 `config_hash` 与 `epoch` ——
装配即生效，没有轮询间隔、没有收敛窗口；运行中的请求按进入时取到的旧快照跑完
（每请求 `load_full`，单请求内配置一致）。**失败整套拒绝**：旧快照继续服务，
管理器留 `invalid_config` 标记，健康视图以 `unhealthy` + "configuration rejected,
previous snapshot still serving" 呈现。

**有界日志总线**：数据面写日志走 `try_send` 即返回，队列满则丢弃并计数
（`EngineReport.log_drops` 上报告）—— **数据面永不阻塞在写日志上**（实测纪律）。
落盘是 `<log_dir>/<env>.log`（管理器侧单一写者）、copytruncate 轮转、有界尾读。

```
spec/            语言中立契约：能力清单、错误码、协议（UI 边界词汇与推送流帧）、
                 Admin API 端点契约、UI 契约与设计规范、规则语法 BNF、golden fixture
crates/
  protocol            UI 边界词汇：视图 DTO、类型化请求 DTO、推送流帧形状、游标、
                      错误信封（纯 serde，零传输依赖；未来桌面形态与 web 共用）   ← 叶子
  engine              ProxyEngine 接缝（EngineSpec / EngineHandle / EngineReport /
                      InstanceState）、错误码、端口（时钟 / 日志 LineWriter）
  │ domain             环境校验 / 合并、端口选择、reconcile 决策                   ← 纯逻辑
  │ rules              hosts 解析 + 确定性渲染（全仓唯一一份解析实现）              ← 纯逻辑
  │ 引擎：共享 CA、rustls 接线（ring + rcgen）、引擎实例、
                      hosts 改写、请求终局日志、有界日志总线
  engine-fake         ProxyEngine 的生命周期替身（测试 dev-dep，真绑端口）
  events              事件词汇：控制面审计 / 数据面轨迹 / JSONL 存取（见 spec/events.md）
  envboard-manager    环境 CRUD、端口分配、账本持久化、锁、健康判定、reconcile、
                      规则账本、EngineSpec 编译与热装配接线、视图 DTO 构造
                      （单一 Manager 类型，impl 按域拆模块文件：lifecycle /
                       rules_store / proxies / reconcile_health / ledger /
                       observability / debug_capture / projection）
  admin               Admin 管理 API 门面（spec/admin-api.md）：类型化入口校验 +
                      用例编排，传输无关（无 axum、无引擎装配）
  web                 web 形态：axum HTTP/SSE 传输绑定 + 安全中间件 + 内嵌前端
                      构建产物（include_dir! 嵌 frontend/dist）；只认识 Admin 门面
  server              envboard 二进制（唯一发布产物）：组合根在 src/main.rs
                      （引擎装配 + 管理器 → AdminService → web 的 serve 编排）
  envboard-contract-tests   消费全部契约 fixture（只有测试目标）
  envboard-policy-tests     工程门禁本身（只有测试目标，不进发布物）
frontend/               工作台前端：Vite 7 + React 19 + HeroUI v3 + Tailwind v4
                      （pnpm 管理；dist/ 是内嵌源，提交入库，src↔dist 成对判定）
scripts/systemd/      部署工件（用户级 unit）
```

依赖方向由工程门禁强制（`cargo test -p envboard-policy-tests --test deps`）：
protocol 是叶子；domain / rules 是纯逻辑（不得依赖 tokio / libc）；UI 面（web 与
admin 全部源码、宿主除 main.rs 外）只认识管理器的公开 API、不认识引擎装配（引擎装配
只允许出现在 src/main.rs 组合根，源文件面判据钉住）—— 换引擎实现不动界面，
加界面形态（桌面 GUI 等）只接 Admin 门面、不动协议。

```
<state_dir>/
├── state.json        环境账本 + 规则账本 + 期望状态（原子写 + 0600）
├── lock              单实例锁（flock）
├── rules/<name>.rules    规则物化文件（可再生；给人看，运行时输入是账本 rendered）
├── logs/<env>.log    实例日志（copytruncate 轮转，保留一份 .1）
├── events.jsonl      控制面审计事件（权威仍是 state.json；8 MiB 轮转，见 spec/events.md）
├── trajectories/<env>.jsonl  请求轨迹（append-only 事件流，见 spec/events.md）
└── shared/confdir/   共享 CA（mitmproxy-ca.pem / mitmproxy-ca-cert.pem）
```

## 构建与快速开始

工具链只有一条：**cargo**（`rust-toolchain.toml` 钉死 1.98.0）。没有 Python、没有 Node。

```bash
cargo build --release --locked --offline      # 产物：target/release/envboard
export PATH="$PWD/target/release:$PATH"

# 1) 起工作台 —— 二进制的唯一行为（管理器 + HTTP API + 内嵌前端 + 进程内引擎）
envboard --state-dir ~/.envboard
#    默认监听 127.0.0.1:8900 且回环免鉴权：浏览器直接打开 http://127.0.0.1:8900
#    要给别的机器用：--listen 0.0.0.0:8900 --token <T>（非回环不给 token 拒绝启动）

# 2) 导入规则 + 建环境：工作台「规则库 / 环境」两个视图各点一下；脚本化走同一个 API
curl -s -X POST localhost:8900/api/rules \
     -H 'content-type: application/json' -H 'x-envboard-request: 1' \
     -d '{"name":"beta","text":"127.0.0.1 api.example.com\n"}'
curl -s -X POST localhost:8900/api/environments \
     -H 'content-type: application/json' -H 'x-envboard-request: 1' \
     -d '{"name":"beta","rules":"beta"}'

# 3) 客户端按端口选环境（工作台每行都有可复制的那一行）
export https_proxy=http://127.0.0.1:16301 http_proxy=http://127.0.0.1:16301
curl https://api.example.com/
```

日常巡检同样走本地 HTTP API（端点清单见「HTTP 端点」）：概况、实例日志尾部、
跨环境静态对比在界面上是顶栏 / 详情面板 / 独立视图，在 API 面各有一个 GET。

### 改一个已经建好的环境

建的时候没绑规则、端口想换一个、描述写错了 —— 都不必删了重建。工作台每行有「编辑」，
脚本化则走 PATCH（未提及不动，`null` 表示清空）：

```bash
curl -s -X PATCH localhost:8900/api/environments/beta \
     -H 'content-type: application/json' -H 'x-envboard-request: 1' \
     -d '{"rules":"beta"}'          # 补绑 / 换绑规则（热的）
#     {"port":16302}    换端口      —— 需先停止（POST .../stop）
#     {"name":"gamma"}  改名        —— 期望状态、端口归属一起搬过去；需先停止
#     {"rules":null}    解绑        —— 回到「不覆盖」
#     {"upstream":"corp"}   绑上游代理  —— 热的（先在 /api/proxies 建好 corp）
#     {"upstream":null}     改回直连    —— 热的
#     {"description":"灰度"}        —— 只改描述（热的）
curl -s localhost:8900/api/rules/beta          # 改规则原文前先取回，免得盲覆盖
```

热 / 停机矩阵（服务端裁决，工作台与 HTTP API 是同一份语义的两个面）：

| 改动 | 运行中 | 生效方式 |
|---|---|---|
| `description` | ✅ | 纯展示字段 |
| `insecure_hosts` | ✅ | 一次同步 apply，返回后第一个请求就是新配置 |
| `rules` **绑定** | ✅ | 账本 rendered 重编译进快照，不重启实例 |
| 规则文件的**内容** | ✅ | import 同名覆盖即对绑定且在跑的环境逐个热应用 |
| `upstream` **绑定** | ✅ | 代理账本定义重编译进快照；改代理实体（host/port/凭据）对引用环境同样热应用 |
| `name` / `listen` | ❌ `conflict` | 它们是环境的对外身份 |
| `proxy_user` / `proxy_password` | ❌ `conflict` | 鉴权门在实例启动时装配 |

- 绑定一个不存在的规则名在**写入时就被拒绝**（`environment.rules`）—— 账本 rendered
  直供引擎，不存在"链断了所以不覆盖"的形态，宁可不接受静默失效。
- 改停机字段时服务端返回 `conflict`，消息说清"先停止"并列出哪些字段是热的。
- 改了 `listen`、规则绑定、`insecure_hosts` 或凭据即作废该环境旧的失败标记
  （`port_conflict` / `invalid_config` 标记）并重置收敛基准 —— 不会拿新配置报旧冲突。

## 配置

### 启动参数

| 参数 | 默认 | 说明 |
|---|---|---|
| `--state-dir <DIR>` | `$ENVBOARD_STATE_DIR`，其次 `~/.envboard` | 账本、锁、规则库、日志、共享 CA 都在它下面 |
| `--port-range <MIN>-<MAX>` | `16000-16999` | 随机分配区间（避开常见服务端口与系统临时端口段） |
| `--listen <HOST:PORT>` | `127.0.0.1:8900` | 工作台监听。**回环默认免鉴权；非回环必须配 `--token`，否则拒绝启动** |
| `--token [T]` | 回环可省、非回环必填 | 启用鉴权：带值用给定 token；裸给（不带值）自动生成 128 bit 随机值并在启动日志打印可点链接 |
| `--log-dir <DIR>` | `<state_dir>/logs` | 实例日志目录 |
| `--no-log-file` | 关 | 实例日志不落盘（只在有界内存缓冲），日志面板读不到尾部；丢弃量经 `log_drops` 上报告 |
| `--max-log-bytes <N>` | 8 MiB | 单环境日志上限，超过 copytruncate 轮转（保留一份 `.1`）；`0` = 不轮转；下限 64 KiB |

按域名放宽上游证书校验（`insecure_hosts`）与代理凭据的唯一入口是工作台的「配置」表单
—— 它们是安全控制，做成"随便填个键值对"的文本框会让"配置说开了、引擎其实没收到"
这种分叉无从排除。

启动错误输出为 `envboard: <错误码>: <消息>`（带字段路径时附 `(field: …)`）；
不可重试错误退出码 2，其余 1。非法配置**加载即失败**并给出字段路径。

### 工作台鉴权与对外暴露

- **token 档位**：回环监听（127.0.0.1 / ::1）**默认免鉴权** —— 本机即本机用户，token
  挡不住同机进程，只添摩擦。`--token <T>` 在任何监听上显式启用；`--token` 裸给（不带
  值）= 自动生成 128 bit 随机 token，启动日志打印
  `dashboard: http://<host>:<port>/?token=<值>`（通配监听用 127.0.0.1 展示）供点击直达。
  **非回环监听必须显式给 `--token`，否则启动即失败**（`invalid_config`，字段
  `web.token`）—— 对外暴露是知情动作，不给"忘了开就裸奔"留路径。服务端接受 header
  `x-envboard-token`（优先）与 URL `?token=`（EventSource 带不了自定义头）两条等效
  通道；前端捕获 URL token 后**保留在地址栏**（reload 不 401），后续请求走 header。
  `/assets/*`（hashed 前端构建产物）豁免（浏览器子资源请求带不了凭据）。
- **与 token 无关、任何档位都不豁免的两道门**：`Host` 必须等于监听地址（DNS rebinding
  防线）；变更类请求必须带 `x-envboard-request: 1`（CSRF 防线）—— 回环免鉴权时，
  后者就是挡住恶意网页打本机端口的闸。**多用户共享的主机请在回环上也给 `--token`**：
  免鉴权档下本机任何进程都能读工作台数据（环境名、端口、日志尾部）。
- **代理访问鉴权**：环境字段 `proxy_user` / `proxy_password` **同生共死**（只有一边 →
  `invalid_config`，错误指向缺失的那一边）。每段非空、不含 `:`、空白或控制字符，
  `proxy_user` ≤64、`proxy_password` ≤128。407 门在引擎内存里定长时间比对 ——
  凭据不进任何进程命令行（live 断言含 /proc 全量 cmdline 审计），明文只落 0600 的
  状态账本；视图 / 日志 / SSE 只暴露「是否启用」布尔。工作台表单不回显已保存凭据
  （看不到 = 不覆盖，要改就填两个新的）。
- **对外服务**：环境监听默认 `127.0.0.1`；表单「对外服务」开关把 `listen.host` 换成
  `0.0.0.0`。勾选而未填代理凭据时表单给警示。创建时勾选对外服务则端口必填
  （自动分配只支持默认监听地址）。

### HTTP 端点

启用 token 后，除两个静态资产外每个端点都要求 token（回环默认档无凭据直接放行）；
变更类（非 GET/HEAD）在任何档位都要求 `x-envboard-request: 1`（CSRF 防线），
且 `Host` 必须等于监听地址（DNS rebinding 防线）。
未匹配路径返回 JSON 形状的 404（`{error:{code,message}}`）。

| 方法 | 路径 | 用途 |
|---|---|---|
| GET | `/` | 工作台页面（内嵌 HTML 外壳） |
| GET | `/assets/<hashed>` | 内嵌前端构建产物（唯一豁免 token 的路径前缀；immutable 缓存） |
| GET | `/api/status` | 管理器与引擎概况、`port_range` 等配置回显、`events_dropped` |
| GET/POST | `/api/environments` | 列表 / 建环境（`name` 必填；端口缺省 = 自动分配） |
| GET/PATCH/DELETE | `/api/environments/:name` | 详情 / 改环境（PATCH 未提及不动、`null` 清空） / 删除（须先停止） |
| POST | `/api/environments/:name/{start,stop,restart,reallocate}` | 启停 / 重启 / 显式重分配端口 |
| GET | `/api/environments/:name/logs?lines=N` | 实例日志尾部 |
| GET | `/api/environments/:name/trajectory?limit=N` | 请求轨迹尾部（拉取式） |
| GET | `/api/environments/:name/trajectory/stream` | SSE 实时轨迹（`baseline` 尾部窗口 + `events` 增量；断线带 `?cursor=` 续传） |
| GET | `/api/history?name=<env>&limit=N` | 控制面审计事件（只读；`name` 缺省 = 全部） |
| GET | `/api/environments/:name/captures?limit=N` | 抓包会话尾部（易失，会话 = 实例生命周期） |
| GET | `/api/environments/:name/captures/:request_id` | 单条抓包详情 |
| POST | `/api/environments/:name/capture/clear` | 手动清空抓包会话（会话延续） |
| GET | `/api/environments/:name/captures/export?format=har\|jsonl` | 导出当前抓包会话（HAR 1.2 / JSONL 下载） |
| POST/GET | `/api/debug`、`POST /api/debug/stop` | 开启/切换调试会话（单例，切换即停旧+清旧+开新）/ 停止并清空 / 当前视图 |
| POST | `/api/har/import?name=<file>` | 导入 HAR 会话（HAR 1.2；多个并存只读，最多 8 个） |
| GET/DELETE | `/api/har`、`GET/DELETE /api/har/:id` | 导入会话列表 / 条目窗口 / 删除 |
| GET/POST | `/api/rules` | 规则账本列表（名字 + 条数） / 导入（覆盖同名 = 对绑定环境热应用） |
| GET/DELETE | `/api/rules/:name` | 规则原文 / 删除（仍被绑定时 `conflict`） |
| GET/POST | `/api/proxies` | 上游代理账本清单（含 `references[]`） / 保存（同名整体替换，凭据只写不读） |
| GET/DELETE | `/api/proxies/:name` | 上游代理详情（凭据永不回显，只有 `has_auth`） / 删除（被环境引用时 `conflict` 并点名） |
| GET | `/api/compare?host=<域名>` | 跨环境静态对比：该域名在各环境被覆盖成什么（不发请求） |
| GET | `/api/events` | SSE 快照（每秒一次全量环境视图，附 `generation`：未变更可跳过重渲） |
| GET | `/api/ca` | 共享 CA 证书只读摘要（版本 / 序列号 / 有效期 / 指纹 / 颁发者 / SAN） |
| GET | `/api/ca.pem` | 下载根证书（只含证书，不带私钥；`Content-Disposition: attachment`） |
| GET | `/api/ca/qrcode.svg?data=<url>&token=<t>` | 把传入 URL（≤512 字节）编码成二维码 SVG，供手机扫码下载证书 |
| POST | `/api/_fault` | 故障注入旋钮（live 断言"实例崩溃"组的面；转给引擎的注入接缝） |

## 验证

单一入口（托管方无关，CI 直接调它即可）：

```bash
bash ci/verify.sh            # 默认层：policy + rust + contract + artifact
bash ci/verify.sh rust       # 纯 Rust 最小子集：policy + rust
bash ci/verify.sh live       # 实机层：真宿主（只需 openssl 与 curl 两个命令行工具）
bash ci/verify.sh policy     # 只跑仓库纪律那一层
```

`--locked --offline` 全程在位：`Cargo.lock` 入库，离线可构建是验收项。

| 层 | 手段 | 关键断言 |
|---|---|---|
| `policy` | `cargo test -p envboard-policy-tests` | 工具链收敛（toolchain）、命名（naming）、文本自包含（doc_scope）、依赖方向（deps）、注册表三方一致（registry）—— 一文件一门禁，可单跑 |
| `rust` | `cargo fmt --check` / `clippy -D warnings` / `check` / `build` / `test --workspace` | 编译、lint、单测 + 冒烟（鉴权档位、非回环拒启、单写者；端口分配、reconcile、锁、健康判定、日志尾部与轮转、环境编辑热/停机） |
| `contract` | `cargo test -p envboard-contract-tests` | 66 个 fixture 的形状与语义都由实现消费；失败用例钉住错误码，成功用例钉归一化字段 |
| `artifact` | `cargo test -p envboard-server --test artifact` | 声明的发布工件逐项在位、内嵌前端资产完整（行数 + 关键符号）、二进制里真的带着进程内引擎 |
| `live` | `cargo test -p envboard-server --test live_workbench -- --ignored` | 29 条实机断言逐条点名打印：双环境对照、规则热重载与热生效时延、`insecure_hosts` 三态对照、既有 CA 零感知、凭据 argv 审计、407/200、鉴权四档（含非回环拒启负向）、CSP、320 连打、注入 failed 与端口释放、崩溃自愈、编辑热生效 |
| `live` | `cargo test -p envboard-engine --test live_manager -- --ignored` | 引擎直驱三组：预放 CA 加载复用 + 回执如实 + curl 验链；注入 failed 可见、端口释放、重拉回 running；insecure 名单外 502 → 热 apply 第一次请求即 200 |

常用的引擎侧单跑（全部 hermetic，不需要网络与宿主）：

```bash
cargo test -p envboard-engine                  # 单测 + data_plane / backend / mitm
cargo test -p envboard-engine --test data_plane  # 规则命中、502、407、insecure 热翻转、port_conflict、隧道保活
cargo test -p envboard-policy-tests --test registry   # 注册表 ↔ 内置实现 ↔ 契约表 三方一致
```

### 工具链纪律

**默认验证路径只有一条工具链：`cargo`。** 第二种工具链（Node/TS/Vite/pnpm）被
**圈禁在 `frontend/` 边界内**：清单文件、TS 源码、`node_modules` 只允许出现在该目录；
`ci/*.sh` 与 `*.service` 的可执行面仍然零前端命令 —— cargo 构建**永不**调用前端工具链，
`frontend/dist/` 作为内嵌源提交入库（src↔dist 成对判定 + drift 校验由
`bash ci/verify.sh frontend` 层负责，该层是显式动作、不在默认 all 里）。
这条约束由门禁自己证明：文件面（边界判定）、调用面（禁出现的命令形态）、
成对判定（src 与 dist 同存同缺）、白名单双向判定（条目失效同样判红）。
`bash ci/verify.sh rust` 是这条纪律的可执行形式：在没有 Python、没有 Node 的机器上全绿。

前端开发流两步式：`pnpm dev`（5199，`/api` 代理到本机 8900 实跑实例联调）→
`pnpm build` 后把 `frontend/dist` 与 `src` 一起提交。

要加一条新门禁，就加一个新测试：纯文本 / 结构门禁放 `envboard-policy-tests`
（一门禁一文件），需要已构建二进制的放所属 crate 的 `tests/`（用 `CARGO_BIN_EXE_<bin>`），
需要真宿主的标 `#[ignore]` 由 `ci/verify.sh live` 显式触发。禁止在门禁里嵌套
`cargo fmt|clippy|check|build`；`ci/verify.sh` 只做编排、不含判据。

### 文本自包含（doc_scope 门禁）

入库的一切面向读者的文本必须自包含：零命中不在仓库里的文档的指称（路径、链接、点名、
章节号都算）；指向本仓的路径必须真实存在；任何含 `§` 的行必须同时写明本仓文件或「本文件」。
可复核的外部事实坐标不在禁列（上游源码位置、实测命令与输出、版本号、协议编号）。
判据由 `cargo test -p envboard-policy-tests --test doc_scope` 机械执行。

## 运维（systemd）

`scripts/systemd/envboard.service` 是**用户级** unit（只监听回环、状态目录在 `$HOME` 下、
没有任何子进程 —— 没有一处需要 root）：

```bash
install -Dm755 target/release/envboard ~/.local/bin/envboard
install -Dm644 scripts/systemd/envboard.service ~/.config/systemd/user/envboard.service
systemctl --user daemon-reload
systemctl --user enable --now envboard
systemctl --user status envboard
curl -s localhost:8900/api/status | head -c 400     # 或直接在浏览器看工作台
```

关键取舍：

- **状态目录交给 systemd 建**：`StateDirectory=envboard` → `~/.local/state/envboard`
  （0700 —— 里面有共享 CA 的**私钥**）。不用 `ReadWritePaths=`，因为它要求路径在挂载
  命名空间建立时已存在，首次安装必失败（实测 `status=226/NAMESPACE`）。
- **`ExecStart` 用绝对路径**：systemd 的 PATH 不含 `~/.local/bin`。引擎就在这个
  二进制里，`envboard --state-dir %S/envboard` 就是全部（默认回环、免鉴权档）。
- **`Restart=on-failure` 是安全的**：期望状态（`desired=running`）已持久化，重启后
  reconcile 会把环境重新拉起；主动 `stop` 不会被当成失败再拉起。
- 实例活在进程内：停服务 = 停工作台 = 停所有实例（端口即刻释放，不存在孤儿代理）。
  想让服务在没有登录会话时也活着：`loginctl enable-linger $USER`。
  改配置别动这个文件，用 `systemctl --user edit envboard` 写 drop-in。
- 没有"只跑环境不跑工作台"的形态，也不会收养 / 清理任何外部进程 —— 端口上蹲着
  别人的进程时，对应环境以 `port_conflict` 如实呈现。

工作台的设计语言以 `spec/design.md` 为准（Token 纪律、布局骨架、按钮分级、
反馈契约、bsk 走查检查表）；机器可判的纪律以 `spec/ui.md` 为准
（机检条目 UI-1…UI-6 由 policy 门禁执行，违反即 `verify` 红）。
控制面端点契约见 `spec/admin-api.md`。

## 健康与错误码

环境实际状态全部来自**内存报告**（契约见 spec/errors.md 的「健康状态」）：

| 状态 | 含义 |
|---|---|
| `stopped` | 无登记实例 |
| `starting` | 已下发启动，绑定结果未定态 |
| `running` | 引擎线程存活且 listener 已绑定；配置装配即生效 |
| `port_conflict` | 绑定失败（EADDRINUSE）；**不换端口**，等一次显式动作 |
| `unhealthy` | 线程存活但异常：accept 持续失败，或配置被 apply 拒绝（旧快照仍在服务） |
| `failed` | 引擎线程终止（panic）→ reconcile 按 desired 自动重启 |

判定次序：**标记 → desired → 引擎内存报告**；视图层与权威判定是同一个函数。
错误码：`invalid_config`（含热更被拒）/ `not_found` / `conflict` / `port_conflict` /
`port_range_exhausted` / `store_failure` / `internal_error` —— 语义、HTTP 映射与
可重试性以 spec/errors.md 为准。

## 已知限制

这一节**是契约的一部分**，不是免责声明：

- **数据面只实现 HTTP/1.1**。MITM 与直连两侧的 ALPN 都只协商 `http/1.1`（声明 h2
  会诱导客户端走我们不支持的协议面）。浏览器会正常回退；**强制 h2 的客户端（gRPC 等）
  会失败** —— 这是写明的非目标，不是静默降级。101 升级请求走透传旁路。
- **上游信任库来自系统库**（rustls-native-certs）："哪些域名免名单就能通过严格校验"
  的集合随宿主机系统的信任库而变。某域名要 502 时，把它加进 `insecure_hosts`
  或修系统信任库。
- **`insecure_hosts` 是精确域名**：归一化后**完全相等**才放宽（不做子域继承、不做后缀
  匹配），写入时拒绝 `*` / `?` 通配符。无 SNI 时才退回按上连地址判定；SNI 存在但没
  命中**不**退回。放宽只降校验档，不改写 SNI —— 那台机器根本没有该域名的证书时，
  进名单也救不了。**没有"整个环境全关校验"的开关**。
- **上游连接不复用**（一问一答）：热改规则 / 名单不会被连接池里的旧策略穿越。代价是
  被覆盖域名每个请求新建一次上连（多一次 TLS 握手）。"只改连到哪"的固有代价，正确性优先。
- **请求 / 响应体默认缓冲**，`max_buffered_body` 上限 8 MiB（快照字段）：超限返回
  413/502 而不是静默流式。大文件穿透不是当前形态。
- **客户端要改代理配置**（换端口即换环境）。工作台给出的那行 `export https_proxy=…`
  就是为此。
- **PAC / 透明代理 / SOCKS 未做**。上游代理只做 HTTP 形态（CONNECT 隧道 +
  absolute-URI + Basic 鉴权）：`https://` 代理（与代理本身的 TLS）、按域名分流、
  代理链都是写明的非目标，语义见 spec/capabilities.md 的「上游代理」。

## 版本与发布

发布产物是**单一 `envboard` 二进制**（引擎、管理器、工作台全在其中；前端构建产物
`include_dir!` 内嵌，运行时零 Node、零 CDN）。全部 crate 显式 `publish = false` ——
"不发布 crate"是 manifest 事实，由 `deps` 门禁钉住；发布物内容由 `artifact` 层做
声明式清单校验（必需文件逐项在位）。当前版本：**0.3.0**，与 git tag 一致是发布前置条件。

# envboard

多环境代理管理器：**一个环境 = 一个进程内引擎实例 + 一个端口**。客户端把代理指到
`127.0.0.1:<环境端口>` 就是在用那个环境，所以多个环境可以**同时活着**、直接对比。

v3 是一次彻底的形态重构：数据面从"spawn 出去的 mitmproxy 子进程 + 注入器"换成
**纯 Rust 进程内引擎**（`envboard-core`）。没有子进程、没有轮询、没有第二种语言 ——
整条 `config.json` / 固定名软链 / 状态文件 TTL / 收敛窗口的进程间通道全部退场，
配置生效塌缩成一次**同步装配**。

规则仍是 hosts 风格的静态覆盖表，在**建连时改写上连目标** —— 客户端看到的一切
（Host 头、SNI 基准）都不变，只有"出网连到哪"变了。

---

## 现在能用什么

| 能力 | 状态 |
|---|---|
| 环境 CRUD / 编辑 / 启停 / 显式重分配端口 / 规则绑定与热重载 | ✅ |
| **热应用同步生效**：PATCH 返回后的第一个请求就是新配置（无轮询、无收敛窗） | ✅ |
| 按域名放宽上游证书校验（`insecure_hosts`：精确匹配，运行中改立即生效） | ✅ |
| 端口自动分配（区间内随机试绑，新建冲突自动重试一次） | ✅ |
| 按 host 改写上连目标（内置插件 `hosts-rules`，只改 `ConnectTarget.resolved_addr`） | ✅ |
| CONNECT 隧道 + MITM 按 SNI 现签 + HTTP/1.1 缓冲转发；absolute-URI 正向代理；101 透传 | ✅ |
| 代理访问鉴权（`proxy_user` / `proxy_password` → 407 门；**凭据只在内存**，不进 argv / 视图 / SSE） | ✅ |
| 共享 CA（所有实例一张；兼容加载既有 mitmproxy CA，已装证书的客户端零感知） | ✅ |
| 规则库账本（`state.json` 的 `rules[]` 是唯一真相；物化文件可再生；启动对账回填） | ✅ |
| 期望状态 reconcile（引擎线程 panic → `failed` → 按 desired 自动重启 = 崩溃自愈） | ✅ |
| 工作台（三视图：环境 / 规则库 / 跨环境对比；详情、可折叠日志栏、SSE 每秒快照） | ✅ |
| 工作台鉴权档位（回环默认免鉴权；非回环必须显式 `--token`；header 与 `?token=` 等效） | ✅ |
| 对外服务开关（环境监听 `0.0.0.0`，默认 `127.0.0.1`） | ✅ |
| CLI 子命令面 | ➖ 已退役：一切管理动作 = 工作台 UI / 本地 HTTP API（见「HTTP 端点」） |
| 单实例锁、状态原子写 + 0600 | ✅ |
| HTTP/2 客户端（gRPC 等） | ❌ 数据面只实现 HTTP/1.1，见「已知限制」 |
| PAC / 透明代理 / SOCKS | ❌ 未做 |

## 架构

**引擎进程模型**：一个环境实例 = 一个 OS 线程 + 一个 current-thread tokio runtime +
一个监听端口，全部活在 envboard 这一个进程里。绑定即真相（`EADDRINUSE` →
`port_conflict`，不试绑、不换端口）。崩溃隔离是**任务级**的：单条请求任务 panic 只死
那一条连接；引擎线程整体 panic 才让实例进入 `failed`，由 reconcile 按期望状态拉回。
这要求 release 构建**保持 unwind**（`panic = "abort"` 会让任务级隔离失效）—— 属构建契约。

**三层能力模型**（注册表 = `core/rs/crates/envboard-core/src/plugin.rs` 的静态只读
`CAPABILITIES`，契约表在 core/spec/capabilities.md 的「v3 插件与能力注册表」，三方一致性
由 policy 门禁 `cargo test -p envboard-policy-tests --test registry` 判定）：

| id | 层 | 阶段 | 语义 |
|---|---|---|---|
| `kernel:listen` | 内核 | startup | 监听 `listen.host:port`，绑定即 `running` |
| `kernel:proxy-auth` | 内核 | startup | 407 门：CONNECT 与 absolute-URI 同一条门；凭据只在内存、定长时间比对 |
| `kernel:tls-policy` | 内核 | connect | `insecure_hosts` → `ConnectTarget.tls_policy` 两档；无全局关校验 |
| `kernel:mitm-ca` | 内核 | startup | confdir 共享 CA 的加载 / 物化（兼容既有 mitmproxy CA） |
| `kernel:protocol` | 内核 | request | HTTP/1.1 协议面与引擎侧超时常量；101 透传 |
| `hosts-rules` | 内置插件 | connect | 消费环境的规则文本，`on_connect` 只修订 `resolved_addr` |
| `request-log` | 内置插件 | log | 默认启用；终局记录写日志通道；错误档 = bypass |
| `debug-inject` | 扩展插件 | request | 测试 / 故障注入旋钮（live 断言用它模拟崩溃）；产品装配路径恒为空 |

内核能力是原生快路径，注册表登记只为可见性 —— 插件链里出现内核 id 是装配错误。
内置插件走与扩展插件**完全相同**的接口（产品功能自举验证接口）。

**阶段管道**写死：`connect → request_head → request_body →（上游）→ response_head →
response_body → log`。插件执行序 = 配置声明序，依赖约束 > 声明序。装配期校验：id 未注册 /
内核冒充插件 / 缺依赖 / 依赖环 → `invalid_config` 点名双方，**没有"静默等待依赖"**。
错误两档：connect / 改写钩子 Err → fail-closed 502（带插件名）；显式 bypass 的插件跳过并
强制 WARN + 逐插件计数；每钩子超时（connect/head 1s、body 5s）按 Err；`on_log` 在类型上
就不可外溢。

**ConnectTarget 是唯一接缝**：`authority / sni / resolved_addr / tls_policy /
chained_proxy`（恒 None 的扩展位）。基础配置与后续插件管道之间只有这一个改写上
连目标的入口 —— 改写语义与 v2 逐项一致：不动请求内容、不动 Host 头、不动 SNI 基准。

**同步热装配**：管理器把归一化环境字段 + 规则账本 rendered 编译成 `EngineSpec`，
`ProxyEngine::apply` 经 **ArcSwap 原子换入**新快照并回执 `config_hash` 与 `epoch` ——
装配即生效，没有轮询间隔、没有收敛窗口；运行中的请求按进入时取到的旧快照跑完
（每请求 `load_full`，单请求内配置一致）。**失败整套拒绝**：旧快照继续服务，
管理器留 `invalid_config` 标记，健康视图以 `unhealthy` + "configuration rejected,
previous snapshot still serving" 呈现。v2 的三层配置回显比对（状态文件 / 规则条数 /
options_echo）整体删除 —— 同步装配不存在"宿主静默忽略一个选项"的介质。

**有界日志总线**：数据面写日志走 `try_send` 即返回，队列满则丢弃并计数
（`EngineReport.log_drops` 上报告）—— v2 实测过"213 个请求写满 64 KiB 管道、
全体客户端挂死"，那条纪律在 v3 以新形态继续成立：**数据面永不阻塞在写日志上**。
落盘仍是 `<log_dir>/<env>.log`（管理器侧单一写者）、copytruncate 轮转、有界尾读。

```
core/spec/            语言中立契约：能力清单、错误码、规则语法 BNF、66 个 golden fixture
core/rs/crates/
  envboard-core-api   ProxyEngine 接缝（EngineSpec / EngineHandle / EngineReport /
                      InstanceState）、错误码、端口（时钟 / 日志 LineWriter）      ← 根
  envboard-domain     环境校验 / 合并、端口选择、reconcile 决策                   ← 纯逻辑
  envboard-rules      hosts 解析 + 确定性渲染（v3 只有这一份解析实现）            ← 纯逻辑
  envboard-core       v3 引擎：共享 CA、rustls 接线（ring + rcgen）、引擎实例、
                      插件管道与能力注册表、有界日志总线
  envboard-core-fake  ProxyEngine 的生命周期替身（真绑定端口、报告可注入）
  envboard-manager    环境 CRUD、端口分配、账本持久化、锁、健康判定、reconcile、
                      规则账本、EngineSpec 编译与热装配接线
  envboard-web        axum API + 内嵌前端（index.html / app.css / app.js）+ envboard
                      二进制（唯一发布产物；组合根在本 crate 的 src/main.rs）
  envboard-contract-tests   消费全部 66 个契约 fixture（只有测试目标）
  envboard-policy-tests     工程门禁本身（只有测试目标，不进发布物）
scripts/systemd/      部署工件（用户级 unit）
```

依赖方向由工程门禁强制（`cargo test -p envboard-policy-tests --test deps`）：
core-api 是根；domain / rules 是纯逻辑（不得依赖 tokio / libc）；web 只认识管理器的公开
API，不认识引擎实现（未来换实现不必动它）。

```
<state_dir>/
├── state.json        环境账本 + 规则账本 + 期望状态（原子写 + 0600）
├── lock              单实例锁（flock）
├── rules/<name>.rules    规则物化文件（可再生；给人看，运行时输入是账本 rendered）
├── logs/<env>.log    实例日志（copytruncate 轮转，保留一份 .1）
├── shared/confdir/   共享 CA（mitmproxy-ca.pem / mitmproxy-ca-cert.pem）——
│                     读的就是 mitmproxy 形状的同名文件，兼容 v2 与已装证书的客户端
└── agent/            v2 遗留（注入器目录）：环境删除 / 改名搬迁时被清理，v3 不写入
```

## 构建与快速开始

工具链只有一条：**cargo**（`rust-toolchain.toml` 钉死 1.98.0）。没有 Python、没有 Node。

```bash
cargo build --release --locked --offline      # 产物：target/release/envboard
export PATH="$PWD/target/release:$PATH"

# 1) 起工作台 —— 同一个二进制、唯一行为（管理器 + HTTP API + 内嵌前端 + 进程内引擎）
envboard --state-dir ~/.envboard
#    默认监听 127.0.0.1:8900 且回环免鉴权：浏览器直接打开 http://127.0.0.1:8900
#    要给别的机器用：--listen 0.0.0.0:8900 --token <T>（非回环不给 token 拒绝启动）

# 2) 导入规则 + 建环境：工作台「规则库 / 环境」两个视图各点一下即可；
#    脚本化走同一个本地 HTTP API（变更类请求必须带 CSRF 头）：
curl -s -X POST localhost:8900/api/rules \
     -H 'content-type: application/json' -H 'x-envboard-request: 1' \
     -d '{"name":"beta","text":"127.0.0.1 api.example.com\n"}'
curl -s -X POST localhost:8900/api/environments \
     -H 'content-type: application/json' -H 'x-envboard-request: 1' \
     -d '{"name":"beta","rules":"beta"}'

# 4) 客户端按端口选环境（工作台每行都有可复制的那一行）
export https_proxy=http://127.0.0.1:16301 http_proxy=http://127.0.0.1:16301
curl https://api.example.com/
```

日常巡检也走同一个本地 HTTP API（工作台界面本就把这些面都摆了出来：概况在顶栏、
日志在详情面板、对比是一个独立视图）：

```bash
curl -s localhost:8900/api/status | head -c 400
curl -s "localhost:8900/api/environments/beta/logs?lines=50"
curl -s "localhost:8900/api/compare?host=api.example.com"   # 跨环境静态对比（不发请求）
```

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
#     {"description":"灰度 v2"}     —— 只改描述（热的）
curl -s localhost:8900/api/rules/beta          # 改规则原文前先取回，免得盲覆盖
```

热 / 停机矩阵（服务端裁决，工作台与 HTTP API 是同一份语义的两个面）：

| 改动 | 运行中 | 生效方式 |
|---|---|---|
| `description` | ✅ | 纯展示字段 |
| `insecure_hosts` | ✅ | 一次同步 apply，返回后第一个请求就是新配置 |
| `rules` **绑定** | ✅ | 账本 rendered 重编译进快照，不重启实例 |
| 规则文件的**内容** | ✅ | import 同名覆盖即对绑定且在跑的环境逐个热应用 |
| `name` / `listen` | ❌ `conflict` | 它们是环境的对外身份 |
| `proxy_user` / `proxy_password` | ❌ `conflict` | 鉴权门在实例启动时装配 |

- 绑定一个不存在的规则名在**写入时就被拒绝**（`environment.rules`）—— 账本 rendered
  直供引擎，不存在"链断了所以不覆盖"的形态，宁可不接受静默失效。
- 改停机字段时服务端返回 `conflict`，消息说清"先停止"并列出哪些字段是热的。
- 改了 `listen`、规则绑定、`insecure_hosts` 或凭据即作废该环境旧的失败标记
  （`port_conflict` / `invalid_config` 标记）并重置收敛基准 —— 不会拿新配置报旧冲突。

## 配置

### 启动参数（唯一入口 = 工作台）

| 参数 | 默认 | 说明 |
|---|---|---|
| `--state-dir <DIR>` | `$ENVBOARD_STATE_DIR`，其次 `~/.envboard` | 账本、锁、规则库、日志、共享 CA 都在它下面 |
| `--port-range <MIN>-<MAX>` | `16000-16999` | 随机分配区间（避开常见服务端口与系统临时端口段） |
| `--listen <HOST:PORT>` | `127.0.0.1:8900` | 工作台监听。**回环默认免鉴权；非回环必须配 `--token`，否则拒绝启动** |
| `--token [T]` | 回环可省、非回环必填 | 启用鉴权：带值用给定 token；裸给（不带值）自动生成 128 bit 随机值并在启动日志打印可点链接 |
| `--log-dir <DIR>` | `<state_dir>/logs` | 实例日志目录 |
| `--no-log-file` | 关 | 实例日志不落盘：没有文件出口，`env logs` 与日志面板读不到尾部；总线的丢弃量经 `log_drops` 上报告 |
| `--max-log-bytes <N>` | 8 MiB | 单环境日志上限，超过 copytruncate 轮转（保留一份 `.1`）；`0` = 不轮转；下限 64 KiB |

### 命令面

没有子命令，也没有直写状态的旁路：`envboard` 就是工作台启动器。建 / 改 / 删环境、
导入规则、看日志、跨环境对比，全部走工作台 UI 或下面的本地 HTTP API —— 判定只在
服务端做一次，界面与脚本看到的语义严格同一份（这正是退役 CLI 命令面后仍成立的理由）。
`smoke` 测试钉着"命令面不得再长出子命令"。

按域名放宽上游证书校验（`insecure_hosts`）与代理凭据的唯一入口是工作台的「配置」表单。它们是安全控制，做成"随便填个键值对"的文本框会让"配置说开了、
引擎其实没收到"这种分叉无从排除（任意选项透传通道已经删除）。

启动错误输出为 `envboard: <错误码>: <消息>`（带字段路径时附 `(field: …)`）；
不可重试错误退出码 2，其余 1。非法配置**加载即失败**并给出字段路径。

### 工作台鉴权与对外暴露

- **token 档位**：回环监听（127.0.0.1 / ::1）**默认免鉴权** —— 本机即本机用户，token
  挡不住同机进程，只添摩擦。`--token <T>` 在任何监听上显式启用；`--token` 裸给（不带
  值）= 自动生成 128 bit 随机 token，启动日志打印
  `dashboard: http://<host>:<port>/?token=<值>`（通配监听用 127.0.0.1 展示）供点击直达。
  **非回环监听必须显式给 `--token`，否则启动即失败**（`invalid_config`，字段
  `web.token`）—— 对外暴露是知情动作，不给"忘了开就裸奔"留路径；旧 `--without-token`
  已随新默认退役（它的语义成了默认档）。服务端接受 header `x-envboard-token`（优先）
  与 URL `?token=`（EventSource 带不了自定义头）两条等效通道；前端捕获 URL token 后
  **保留在地址栏**（reload 不 401），后续请求走 header。`/app.css` / `/app.js` 两个
  内嵌静态资产豁免（浏览器子资源请求带不了凭据）。
- **与 token 无关、任何档位都不豁免的两道门**：`Host` 必须等于监听地址（DNS rebinding
  防线）；变更类请求必须带 `x-envboard-request: 1`（CSRF 防线）—— 回环免鉴权时，
  后者就是挡住恶意网页打本机端口的闸。**多用户共享的主机请在回环上也给 `--token`**：
  免鉴权档下本机任何进程都能读工作台数据（环境名、端口、日志尾部）。
- **代理访问鉴权**：环境字段 `proxy_user` / `proxy_password` **同生共死**（只有一边 →
  `invalid_config`，错误指向缺失的那一边）。每段非空、不含 `:`、空白或控制字符，
  `proxy_user` ≤64、`proxy_password` ≤128。v3 的 407 门在引擎内存里定长时间比对 ——
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
| GET | `/app.css` / `/app.js` | 内嵌静态资产（唯一豁免 token 的两个路径） |
| GET | `/api/status` | 管理器与引擎概况、`port_range` 等配置回显 |
| GET/POST | `/api/environments` | 列表 / 建环境（`name` 必填；端口缺省 = 自动分配） |
| GET/PATCH/DELETE | `/api/environments/:name` | 详情 / 改环境（PATCH 未提及不动、`null` 清空） / 删除（须先停止） |
| POST | `/api/environments/:name/{start,stop,restart,reallocate}` | 启停 / 重启 / 显式重分配端口 |
| GET | `/api/environments/:name/logs?lines=N` | 实例日志尾部 |
| GET/POST | `/api/rules` | 规则账本列表（名字 + 条数） / 导入（覆盖同名 = 对绑定环境热应用） |
| GET/DELETE | `/api/rules/:name` | 规则原文 / 删除（仍被绑定时 `conflict`） |
| GET | `/api/compare?host=<域名>` | 跨环境静态对比：该域名在各环境被覆盖成什么（不发请求） |
| GET | `/api/events` | SSE 快照（每秒一次全量环境视图） |
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
| `rust` | `cargo fmt --check` / `clippy -D warnings` / `check` / `build` / `test --workspace` | 编译、lint、单测 + 冒烟（鉴权档位、非回环拒启、单写者、命令面不得有子命令；端口分配、reconcile、锁、健康判定、日志尾部与轮转、环境编辑热/停机） |
| `contract` | `cargo test -p envboard-contract-tests` | 66 个 fixture 的形状与语义都由实现消费；失败用例钉住错误码，成功用例钉归一化字段 |
| `artifact` | `cargo test -p envboard-web --test artifact` | 发布工件清单逐项在位、旧代残留为零、二进制里真的带着进程内引擎 |
| `live` | `cargo test -p envboard-web --test live_workbench -- --ignored` | 两个环境同时可用且结果不同、规则热重载、热生效时延（PATCH 返回后第一个请求即新配置）、`insecure_hosts` 对照（名单外 502、热加名单后 200、未点名域名仍 502）、既有 CA 零感知（预放 mitmproxy 形状 CA 逐字节不变加载 + 真 curl 验链）、凭据 argv 审计（/proc 全量不得出现明文密码）、407/200 对照、安全（Host / CSRF / 鉴权四档：回环默认免鉴权 200、裸 --token 自动生成、显式 --token 下 header 与 ?token= 双通道 200 + 端面逐个无凭据 401 清点、非回环无 token 拒启）、CSP、连打 320 个请求不卡死、注入 failed 后端口真释放、崩溃自愈、编辑后按新配置真的生效 |
| `live` | `cargo test -p envboard-core --test live_manager -- --ignored` | 引擎直驱三组：预放 CA 加载复用 + 回执如实 + curl 验链；注入 failed 可见、端口释放、重拉回 running；insecure 名单外 502 → 热 apply 第一次请求即 200 |

常用的引擎侧单跑（全部 hermetic，不需要网络与宿主）：

```bash
cargo test -p envboard-core                    # 单测 + data_plane / plugins / backend / mitm
cargo test -p envboard-core --test data_plane  # 规则命中、502、407、insecure 热翻转、port_conflict、隧道保活
cargo test -p envboard-core --test plugins     # fail-closed 带名、bypass+计数、钩子超时、Early 不触上游
cargo test -p envboard-policy-tests --test registry   # 注册表 ↔ 内置实现 ↔ 契约表 三方一致
```

### 工具链纪律

**这个仓库只有一条工具链：`cargo`。** 没有第二种语言的源码、清单文件、包管理器或门禁脚本
（v2 时代的 `adapters/` 单文件注入器已随引擎重构整体退场）。这条约束由门禁自己证明：
文件面（禁出现的清单与源码后缀）、调用面（`ci/*.sh` 与 `*.service` 里禁出现的命令形态）、
迁移白名单双向判定（条目失效同样判红；表已清空 = 收敛完成）。
`bash ci/verify.sh rust` 是这条纪律的可执行形式：在没有 Python、没有 Node 的机器上全绿。

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
# 看一眼状态：工作台页面，或本地 API
curl -s localhost:8900/api/status | head -c 400
```

关键取舍：

- **状态目录交给 systemd 建**：`StateDirectory=envboard` → `~/.local/state/envboard`
  （0700 —— 里面有共享 CA 的**私钥**）。不用 `ReadWritePaths=`，因为它要求路径在挂载
  命名空间建立时已存在，首次安装必失败（实测 `status=226/NAMESPACE`）。
- **`ExecStart` 用绝对路径**：systemd 的 PATH 不含 `~/.local/bin`。v3 起不需要
  `--core-bin` —— 引擎就在这个二进制里，
  `envboard --state-dir %S/envboard` 就是全部（默认回环监听、免鉴权档）。
- **`Restart=always` 是安全的**：期望状态（`desired=running`）已持久化，重启后 reconcile
  会把环境重新拉起；主动 `stop` 不会被当成失败再拉起。
- 工作台就是唯一的常驻形态（v0.2 起没有无界面的 `run` 变体）：实例活在进程内，
  停服务 = 停工作台 = 停所有实例。想让服务在没有登录会话时也活着：
  `loginctl enable-linger $USER`。
  改配置别动这个文件，用 `systemctl --user edit envboard` 写 drop-in。

工作台界面的视觉令牌、CSP 约束与交互纪律以
`core/rs/crates/envboard-web/assets/app.css` 第 ① 区为准（那里是机器可读的唯一来源）。

## 从 v2 升级

v2（mitmproxy 子进程核心）到 v3（进程内引擎）**数据契约兼容，无需迁移工具**：

- **`state.json` 直接可读**。缺失的段由默认值补齐；v2 写下的 `records` /
  `config_seals`（进程身份与收敛封印）加载时被接受，但 v3 没有任何判定消费它们 ——
  进程时代结束了。
- **已装的 CA 零感知**。共享 CA 的路径与文件名不变：`<state_dir>/shared/confdir/` 里
  既有的 `mitmproxy-ca.pem`（PKCS#1 私钥 + 证书拼接）与 `mitmproxy-ca-cert.pem` 被引擎
  **原样加载**（逐字节不改写），装过该证书的客户端什么都不用做。实机断言（live 层的 CA
  兼容组）预放一份 mitmproxy 形状的 CA，验的就是"不重新生成 + 真 curl 凭它验 MITM 链成功"。
- **坏 CA 响亮重做**。若既有 confdir 里的 CA 配对不上 / 不是 CA / 过期，引擎会删除并
  重新物化，并在 stderr 打两行警告：`shared CA was regenerated …` 与
  `clients must reinstall the CA from …`。这时**客户端需要重装证书** —— 带病服务的
  症状（随机证书错误）比一次重装糟糕得多。
- **v2 遗留的 `<state_dir>/agent/`**（注入器目录）不主动扫描；环境删除与改名搬迁时
  对应目录会被顺手清理。想立刻腾空直接删整个 `agent/` 即可，v3 不读它。
- **升级前手动停掉还在跑的 v2 实例**。v3 没有外部进程账本（不 spawn、不收养、不清理
  孤儿），还在监听的 mitmdump 会一直占着环境端口：

  ```bash
  pkill -f mitmdump        # 确认没有 v2 实例残留再升级
  ```
- **CLI 面整体退役**：v2 的 `--core mitmproxy` / `--core-bin` / `--core-python`，以及
  本版本此前的全部子命令（`status` / `run` / `env` / `rules` / `compare` / `web`）与
  `--core`、`--once`、`--json`、`--without-token` 全部删除 —— `envboard` 只剩一个行为：
  启动工作台。旧形态命令行会**响亮失败**（未知参数/子命令，退出码 2），不会静默改道。
- **行为差异**：热改（`insecure_hosts` / 规则绑定 / 规则内容 / 描述）从"按轮询间隔
  收敛"变成**同步装配、返回即生效**；`config_mismatch` 状态随之从契约删除
  （被拒配置以 `invalid_config` 标记 + `unhealthy` 表达，旧快照继续服务）。
  代理凭据不再进子进程命令行（v2 的 `--set proxyauth=…` 通道随子进程一起消失）。
- 规则库账本（`rules[]` 含 rendered）在 v2 后期就已就位，v3 原样沿用；
  `<rules_dir>` 里有而账本没有的 `*.rules` 在启动对账时回填。固定名软链不再维护。

## 健康与错误码

环境实际状态全部来自**内存报告**（契约见 core/spec/errors.md 的「健康状态」）：

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
可重试性以 core/spec/errors.md 为准。

## 已知限制

这一节**是契约的一部分**，不是免责声明：

- **数据面 v1 只实现 HTTP/1.1**。MITM 与直连两侧的 ALPN 都只协商 `http/1.1`（声明 h2
  会诱导客户端走我们不支持的协议面）。浏览器会正常回退；**强制 h2 的客户端（gRPC 等）
  会失败** —— 这是写明的非目标，不是静默降级。101 升级请求走透传旁路。
- **上游信任库来自系统库**（rustls-native-certs），不再是 mitmproxy 自带的 certifi。
  后果："哪些域名免名单就能通过严格校验"的集合随宿主机系统的信任库而变，与 v2 的
  certifi 集合**不保证一致**。某域名从"能过"变成"要 502"时，把它加进 `insecure_hosts`
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
- **PAC / 透明代理 / SOCKS 未做**。`ConnectTarget.chained_proxy` 是留好的扩展位
  （当前恒 None）。

## 版本与发布

发布产物是**单一 `envboard` 二进制**（引擎、管理器、工作台全在其中；前端资产
`include_str!` 内嵌，零构建步骤、零 CDN）。全部 crate 显式 `publish = false` ——
"不发布 crate"是 manifest 事实，由 `deps` 门禁钉住；发布物内容由 `artifact` 层做
声明式清单校验（必需文件逐项在位）。当前版本：**0.2.0**。

版本与 git tag 一致是发布前置条件；`git tag v0.1.0` 保留的是 v1 全貌，
v2 与 v3 的条目都在 CHANGELOG.md 的 `[Unreleased]` 段里。

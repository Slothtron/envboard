# envboard

多环境代理管理器：**一个环境 = 一个进程内引擎实例 + 一个端口**。客户端把代理指到
`127.0.0.1:<环境端口>` 就是在用那个环境，所以多个环境可以**同时活着**、直接对比。

数据面是**纯 Rust 进程内引擎**（`envboard-engine`）：没有子进程、没有轮询、没有第二种
语言 —— 配置生效塌缩成一次**同步装配**，改配置返回后的第一个请求就是新配置。

规则是 hosts 风格的静态覆盖表，在**建连时改写上连目标** —— 客户端看到的一切
（Host 头、SNI 基准）都不变，只有"出网连到哪"变了。

界面是**原生桌面工作台**（GPUI）：一个二进制，起起来就是界面，不经过浏览器、
不需要第二条工具链。曾经的 web 形态（HTTP+SSE + 内嵌 SPA）已整体下线，
理由、代价与复活条件见 docs/adr-003-retire-web-frontend.md。

---

## 现在能用什么

| 能力 | 状态 |
|---|---|
| 环境 CRUD / 编辑 / 启停 / 显式重分配端口 / 规则绑定与热重载 | ✅ |
| **热应用同步生效**：写操作返回后的第一个请求就是新配置（无轮询、无收敛窗） | ✅ |
| 按域名放宽上游证书校验（`insecure_hosts`：精确匹配，运行中改立即生效） | ✅ |
| 端口自动分配（区间内随机试绑，新建冲突自动重试一次） | ✅ |
| **上游代理（二级代理）**：按名管理的代理账本 + 环境按名绑定（热字段）；TLS 目标走 CONNECT 隧道（内层 TLS 与 `insecure_hosts` 语义不变），明文 http 按 absolute-URI 转发；删除被引用的代理拒绝并点名 | ✅ |
| 按 host 改写上连目标（hosts 规则，只改 `ConnectTarget.resolved_addr`） | ✅ |
| CONNECT 隧道 + MITM 按 SNI 现签 + HTTP/1.1 缓冲转发；absolute-URI 正向代理；101 透传 | ✅ |
| 代理访问鉴权（`proxy_user` / `proxy_password` → 407 门；**凭据只在内存**，不进 argv / 视图 / 日志） | ✅ |
| 共享 CA（所有实例一张；兼容加载 mitmproxy 形状的同名 CA 文件，已装证书的客户端零感知） | ✅ |
| 规则库账本（`state.json` 的 `rules[]` 是唯一真相；物化文件可再生；启动对账回填） | ✅ |
| 期望状态 reconcile（引擎线程 panic → `failed` → 按 desired 自动重启 = 崩溃自愈） | ✅ |
| 单实例锁、状态原子写 + 0600 | ✅ |
| **桌面工作台**：环境列表（搜索 / 状态过滤 / 排序 / 启停 / 删除 / 新建）、详情面板（概览 / 配置 / 规则三页签）、规则集目录（搜索 / 引用反查 / 查看原文 / 导入 / 删除）、上游代理目录（引用反查 / 新建 / 编辑 / 删除） | ✅ |
| 请求轨迹、抓包与调试页、HAR 导入会话、活动审计、设置（CA 与装证书）、跨环境对比 | ⏳ 引擎与管理器侧**已实现**，界面属于 P2（web 形态下线后这几屏暂无界面） |
| 环境监听 `0.0.0.0`（对外服务） | ✅ 引擎侧能力，工作台「配置」里有开关 |
| HTTP/2 客户端（gRPC 等） | ❌ 数据面只实现 HTTP/1.1，见「已知限制」 |
| PAC / 透明代理 / SOCKS | ❌ 未做 |
| 远程访问、脚本化管理面（HTTP API） | ❌ 随 web 形态出库，见 ADR-003「接受的损失」 |

一切管理动作的入口 = 桌面工作台。判定只在服务端做一次，界面只是它的一个翻译面。

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
spec/            语言中立契约：能力清单、错误码、界面边界词汇（视图 DTO / 帧 / 游标）、
                 Admin 门面契约、规则语法 BNF、golden fixture、桌面与 web 的行为对账
crates/
  protocol            UI 边界词汇：视图 DTO、类型化请求 DTO、快照帧形状、游标、
                      错误信封（纯 serde，零传输依赖）                      ← 叶子
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
  admin               Admin 管理门面（spec/admin-api.md）：类型化入口校验 +
                      用例编排，传输无关（无 axum、无引擎装配）
  host                唯一组合根（lib）：引擎装配 + 共享 CA 物化 + 管理器构造（含单实例
                      锁）+ 宿主编排（reconcile、日志照看）。引擎装配符号只允许出现在
                      这里；界面只拿到 Admin 门面，管理器类型不出 crate。
                      测试经 prepare_with 注入假引擎，走的就是生产那条构造路径
                      （见 docs/adr-002-composition-root-in-envboard-host.md）
  desktop             桌面形态（GPUI）= 唯一的界面与唯一的发布物 `envboard`：
                      CLI 翻译 + tokio 运行时归属 + 进程内直连 Admin 门面。
                      跨界只有一条通道（`hop`），框架只认 gpui-kit 一个包
scripts/verify.py     验证编排（仅标准库，跨平台）
mise.toml             工具版本锁定（python）+ 任务入口（mise run <task>）
```

依赖方向由工程门禁强制（`cargo test -p envboard-policy-tests --test deps`）：
protocol 是叶子；domain / rules 是纯逻辑（不得依赖 tokio / libc）；界面面（admin 与
desktop 的全部源码）只认识管理器的公开 API、不认识引擎装配 —— 引擎装配**只允许出现在
`crates/host`**（唯一组合根），判据有三层：装配符号必须真的在 host 里（搬回去就红）、
盲区表里的 crate 出现它就红、新界面形态消费门面却没登记进盲区表也红。
于是换引擎实现不动界面，加界面形态只接 Admin 门面、不动协议
（见 docs/adr-002-composition-root-in-envboard-host.md）。
GUI 框架另有体积守卫：`gpui-kit` 只允许出现在 `crates/desktop`（第 11 条）。

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

工具链只有一条：**cargo**（`rust-toolchain.toml` 钉死 1.98.0）。clone 完直接构建，
不需要 Node/pnpm，也没有"先前端后 Rust"的顺序要求。装了 mise 的话
`mise install` 拿齐 Python（验证编排用），`mise run` 可用全部任务。

```bash
cargo build --release --locked --offline      # 产物：target/release/envboard(.exe)
export PATH="$PWD/target/release:$PATH"

# 1) 起工作台 —— 二进制的唯一行为（界面 + 管理器 + 进程内引擎，全在一个进程里）
envboard --state-dir ~/.envboard
#    界面直接出来，没有"再开浏览器"这一步；状态目录被另一个实例占用时响亮失败。

# 2) 导入规则 + 建环境：侧栏「规则」→「导入规则集」，再回「环境」→「新建环境」，
#    在详情面板的「配置」页签里把规则绑上（绑定是热字段，不用重启环境）

# 3) 客户端按端口选环境（环境列表每行的详情面板里都有可复制的那一行）
export https_proxy=http://127.0.0.1:16301 http_proxy=http://127.0.0.1:16301
curl https://api.example.com/
```

### 改一个已经建好的环境

建的时候没绑规则、端口想换一个、描述写错了 —— 都不必删了重建。选中环境即开详情面板，
「配置」页签逐项标注**热生效**还是**需停机**；未改动的字段不会被提交。

热 / 停机矩阵（服务端裁决，界面只是把同一份语义写清楚）：

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
- 热应用被拒时环境标 `failed`/`unhealthy` 并给原因，**界面在每次写操作后都补取一次
  快照**，所以"改了但没生效"看得见（`crates/desktop` 的取数纪律）。

## 配置

### 启动参数

| 参数 | 默认 | 说明 |
|---|---|---|
| `--state-dir <DIR>` | `$ENVBOARD_STATE_DIR`，其次 `~/.envboard` | 账本、锁、规则库、日志、共享 CA 都在它下面 |
| `--port-range <MIN>-<MAX>` | `16000-16999` | 随机分配区间（避开常见服务端口与系统临时端口段） |
| `--capture-budget <N>` | 256 MiB | 每环境抓包缓冲预算（`0` = 引擎默认） |
| `--log-dir <DIR>` | `<state_dir>/logs` | 实例日志目录 |
| `--no-log-file` | 关 | 实例日志不落盘（只在有界内存缓冲），日志面板读不到尾部；丢弃量经 `log_drops` 上报告 |
| `--max-log-bytes <N>` | 8 MiB | 单环境日志上限，超过 copytruncate 轮转（保留一份 `.1`）；`0` = 不轮转；下限 64 KiB |

曾经的 `--listen` / `--token` 随 web 形态出库：桌面形态不起 HTTP 服务，
也就没有"监听哪儿、要不要 token"这两件事。

按域名放宽上游证书校验（`insecure_hosts`）与代理凭据的唯一入口是工作台的「配置」表单
—— 它们是安全控制，做成"随便填个键值对"的文本框会让"配置说开了、引擎其实没收到"
这种分叉无从排除。

启动错误输出为 `envboard: <错误码>: <消息>`（带字段路径时附 `(field: …)`）；
不可重试错误退出码 2，其余 1。非法配置**加载即失败**并给出字段路径。

### 凭据与对外服务

- **凭据只进不出**：代理鉴权与上游凭据在任何视图里都只有布尔（`has_auth` /
  `proxy_auth_enabled`）。表单不回显已保存凭据 —— 但要看清一件事：**"留空"不是
  "保持不变"**，`proxy_put` 是整条覆盖，凭据两栏缺席就是不配置鉴权。所以编辑一条
  已启用鉴权的代理时，保存按钮直接写成「保存（将清除鉴权）」，而不是让用户猜。
- **代理访问鉴权**：环境字段 `proxy_user` / `proxy_password` **同生共死**（只有一边 →
  `invalid_config`，错误指向缺失的那一边）。每段非空、不含 `:`、空白或控制字符，
  `proxy_user` ≤64、`proxy_password` ≤128。407 门在引擎内存里定长时间比对 ——
  凭据不进任何进程命令行（live 断言含 /proc 全量 cmdline 审计），明文只落 0600 的
  状态账本；视图 / 日志只暴露「是否启用」布尔。
- **对外服务**：环境监听默认 `127.0.0.1`；表单「对外服务」开关把 `listen.host` 换成
  `0.0.0.0`。勾选而未填代理凭据时表单给警示。创建时勾选对外服务则端口必填
  （自动分配只支持默认监听地址）。这是**代理端口**对外，不是工作台对外 ——
  工作台没有网络面可暴露了。
- **密码输入打码**：桌面没有浏览器的 `type=password`，密码框靠显式声明
  `InputState::masked(true)`（打码同时把值挡在剪贴板选择之外）。

### 控制面入口

门面上有哪些方法、吃什么吐什么、什么时候生效，见 **spec/admin-api.md**
（v2.0 起它是 `AdminService` 的方法契约，不再是端点表）。界面、契约测试与验收
共同依赖那份契约；曾经的 `curl` 用法随 HTTP 面一起出库。

## 验证

单一入口（托管方无关，CI 直接调它即可；`mise run verify` 等价于默认层）：

```bash
python scripts/verify.py            # 默认层：policy + rust + contract
python scripts/verify.py rust       # 纯 Rust 最小子集：policy + rust
python scripts/verify.py live       # 实机层：真环境（只需 openssl 与 curl 两个命令行工具）
python scripts/verify.py policy     # 只跑仓库纪律那一层
```

`--locked --offline` 全程在位：`Cargo.lock` 入库，离线可构建是验收项。

| 层 | 手段 | 关键断言 |
|---|---|---|
| `policy` | `cargo test -p envboard-policy-tests` | 依赖方向（deps）、文本自包含（doc_scope）、命名（naming）、工具链收敛（toolchain）、发布物纪律（artifacts）—— 一文件一门禁，可单跑 |
| `rust` | `cargo fmt --check` / `clippy -D warnings` / `check` / `build` / `test --workspace` | 编译、lint、单测 + 冒烟（单实例锁、端口分配、reconcile、健康判定、日志尾部与轮转、环境编辑热/停机） |
| `contract` | `cargo test -p envboard-contract-tests` | 66 个 fixture 的形状与语义都由实现消费；失败用例钉住错误码，成功用例钉归一化字段；能力注册表 ↔ 内置实现 ↔ 契约表三方一致也在这层判 |
| `live` | `cargo test -p envboard-engine --test live_manager -- --ignored` | 引擎直驱三组：预放 CA 加载复用 + 回执如实 + curl 验链；注入 failed 可见、端口释放、重拉回 running；insecure 名单外 502 → 热 apply 第一次请求即 200 |

发布物内容另有两处判据：仓库文件面（必需文件、历史残留为零、`target/` 被忽略、
四件 CA 测试资产必须在库）在 `--test artifacts`；二进制里真的带着进程内引擎
在 `cargo test -p envboard-desktop --test artifact`。

界面这一侧的验收边界与逐屏对账见 **spec/desktop-parity.md** —— 本机 agent 会话被
OS 拒了输入注入、框架的无头 UI 测试又不可解析，所以证据是"纯逻辑单测 + 实体级测试 +
`PrintWindow` 截图"三件拼起来的，缺哪件都不算过。

常用的引擎侧单跑（全部 hermetic，不需要网络）：

```bash
cargo test -p envboard-engine                  # 单测 + data_plane / backend / mitm
cargo test -p envboard-engine --test data_plane  # 规则命中、502、407、insecure 热翻转、port_conflict、隧道保活
cargo test -p envboard-contract-tests          # fixture 形状 + 能力注册表三方一致
```

### 工具链与任务入口

**一条工具链**：Rust（`rust-toolchain.toml` 钉死）+ 一个 Python 编排位
（`scripts/verify.py`，只用标准库，Linux / macOS / Windows 原生通用）。
任务入口是 mise（根目录 `mise.toml` 锁 python 版本；rust 不在它手里，避免双头）：
`mise run` 列出全部任务，`mise run desktop` / `test` / `verify` / `verify-desktop` /
`package` 即用。

`toolchain` 门禁现在的作用是**回归守卫**：仓库里已经没有前端工具链的合法容身处，
任何 Node/TS 清单、TS 源码或 `node_modules` 出现即红，可执行面里调用 pnpm/node
同样红。想把它带回来，先读 docs/adr-003-retire-web-frontend.md 的「复核触发条件」。

要加一条新门禁，就加一个新测试：纯文本 / 结构门禁放 `envboard-policy-tests`
（一门禁一文件），需要已构建二进制的放所属 crate 的 `tests/`（用 `CARGO_BIN_EXE_<bin>`），
需要真环境的标 `#[ignore]` 由 `scripts/verify.py live` 显式触发。禁止在门禁里嵌套
`cargo fmt|clippy|check|build`；`scripts/verify.py` 只做编排、不含判据。

### 文本自包含（doc_scope 门禁）

入库的一切面向读者的文本必须自包含：零命中不在仓库里的文档的指称（路径、链接、点名、
章节号都算）；指向本仓的路径必须真实存在；任何含 `§` 的行必须同时写明本仓文件或「本文件」。
可复核的外部事实坐标不在禁列（上游源码位置、实测命令与输出、版本号、协议编号）。
判据由 `cargo test -p envboard-policy-tests --test doc_scope` 机械执行。

### 界面契约放在哪

桌面形态的界面纪律不是"设计规范"，而是**行为对账**：spec/desktop-parity.md 逐屏写明
web 形态原来有什么、桌面现在有什么、差在哪、差的那部分怎么处置。视觉语汇由 GPUI 主题
令牌承担（`cx.theme()`，界面不许写死色值），不再有机检规范 —— 曾经服务 SPA 的
`spec/ui.md` / `spec/design.md` 与 `ui_style` 门禁随形态一起出库。 <!-- doc-scope-lint: allow -->

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

判定次序：**标记 → desired → 引擎内存报告**；视图层与权威判定是同一个函数
（`crates/desktop/src/display.rs` 逐字移植自原 SPA 的派生逻辑，带单测）。
错误码：`invalid_config`（含热更被拒）/ `not_found` / `conflict` / `port_conflict` /
`port_range_exhausted` / `store_failure` / `internal_error` —— 语义与
可重试性以 spec/errors.md 为准（HTTP 状态映射随传输面出库）。

## 已知限制

这一节**是契约的一部分**，不是免责声明：

- **数据面只实现 HTTP/1.1**。MITM 与直连两侧的 ALPN 都只协商 `http/1.1`（声明 h2
  会诱导客户端走我们不支持的协议面）。浏览器会正常回退；**强制 h2 的客户端（gRPC 等）
  会失败** —— 这是写明的非目标，不是静默降级。101 升级请求走透传旁路。
- **只有桌面形态**：没有远程访问、没有 HTTP API 可脚本化、没有无头常驻服务
  （systemd 用户服务那种跑法）。这是 ADR-003 明确接受的损失，不是待办。
- **P2 屏暂无界面**：抓包/调试、活动审计、设置（CA 与装证书）、跨环境对比 ——
  引擎与管理器侧能力都在，界面还没补上（清单见 spec/desktop-parity.md）。
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

发布产物是**单一 `envboard` 桌面二进制**（引擎、管理器、界面全在其中）。
全部 crate 显式 `publish = false` —— "不发布 crate"是 manifest 事实，由 `deps` 门禁
第 9 条钉住；发布物纪律由 `artifacts`（仓库文件面）与 `crates/desktop/tests/artifact.rs`
（二进制内容）两处判据钉。当前版本：**0.3.0**，与 git tag 一致是发布前置条件。

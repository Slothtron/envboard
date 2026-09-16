# envboard

多环境代理管理器：**一个环境 = 一个独立代理实例 + 一个端口**。客户端把代理指到
`127.0.0.1:<环境端口>` 就是在用那个环境，所以多个环境可以**同时活着**、直接对比。

规则是 hosts 风格的静态覆盖表，在**建连时改写上连目标** —— 客户端看到的一切
（Host 头、SNI 基准）都不变，只有"出网连到哪"变了。

> 上游参考 `opensource/mitmproxy/` 仅作参考，**从未修改**。

---

## 现在能用什么

| 能力 | 状态 |
|---|---|
| 环境 CRUD / **编辑** / 启停 / 显式重分配端口 / 规则绑定与热重载 | ✅ |
| **按域名放宽上游证书校验**（`insecure_hosts`：精确匹配、运行中改也**热生效**，其余域名一律严格校验） | ✅ |
| 端口自动分配（区间内随机试绑，撞了就重试一次） | ✅ |
| 按 host 改写上连目标（真 mitmproxy `server_connect`） | ✅ |
| 共享 CA（所有实例一张，客户端只装一次） | ✅ |
| 工作台（环境列表 / 启停 / 编辑 / 规则导入与载入 / 跨环境对比 / 日志 / SSE） | ✅ |
| 工作台 URL token 鉴权（默认启用、自动生成；header 优先，`?token=` 等效） | ✅ |
| 代理访问鉴权（环境 `proxy_user` / `proxy_password` → mitmproxy `proxyauth`，Basic 认证） | ✅ |
| 对外服务开关（环境监听 `0.0.0.0`，默认 `127.0.0.1`） | ✅ |
| CLI（本地 API 的瘦客户端；无常驻实例时自己驱动管理器） | ✅ |
| 期望状态 reconcile（崩溃/重启后自动恢复） | ✅ |
| 单实例锁、状态原子写 + 0600 | ✅ |
| PAC / 透明代理 / SOCKS | ❌ 未做（属 core 的可选能力） |

## 构建与快速开始

```bash
cargo build --release --locked --offline      # 产物：target/release/envboard
export PATH="$PWD/target/release:$PATH"

# 1) 导入一份 hosts 风格规则（会被解析成确定性的规范文件）
envboard --state-dir ~/.envboard rules import beta --file examples/hosts.sample.txt

# 2) 建环境（不给 --port 就在 16000–16999 里随机分配一个空闲端口）
envboard --state-dir ~/.envboard env add beta --rules beta

# 3) 起工作台（管理器 + HTTP API + 内嵌前端，同一个二进制）
envboard --state-dir ~/.envboard web --listen 127.0.0.1:8900
#    打开 http://127.0.0.1:8900

# 4) 客户端按端口选环境（工作台每行都有可复制的那一行）
export https_proxy=http://127.0.0.1:16301 http_proxy=http://127.0.0.1:16301
curl https://api.example.com/
```

CLI 在工作台跑着的时候会**自动走本地 HTTP API**（不再抢状态锁），所以两种用法不冲突：

```bash
envboard --state-dir ~/.envboard env list                  # 有常驻实例 → thin client
envboard --state-dir ~/.envboard env logs beta             # 实例日志尾部
envboard --state-dir ~/.envboard compare api.example.com   # 跨环境静态对比（不发请求）
```

### 改一个已经建好的环境

建的时候没绑规则、端口想换一个、描述写错了 —— 都不必删了重建。工作台每行有「编辑」
（同一个表单切到编辑模式），命令行是 `env edit`：

```bash
envboard env edit beta --rules beta            # 补绑规则（停不停机都行：绑定是热的）
envboard env edit beta --port 16302            # 换端口（需停止）
envboard env edit beta --rename gamma          # 改名（期望状态、端口归属一起搬过去）
envboard env edit beta --no-rules              # 解绑，回到「不覆盖」
envboard env edit beta --description "灰度 v2" # 只改描述
envboard rules show beta                        # 改规则文件前先取回原文，免得盲覆盖
```

**没有**「任意 core 选项」的开关：环境只认一等字段。按域名放宽上游证书校验
（`insecure_hosts`）与代理凭据的入口都在工作台的「配置」表单里 —— 它们是安全控制，
做成「随便填个键值对」的文本框会让「配置说开了、实例其实没收到」这种分叉无从排除。

四条规则（服务端裁决，工作台与 CLI 只是同一份语义的两个面）：

- **描述、`insecure_hosts`、规则绑定随时可改** —— 都是热的：`insecure_hosts` 与绑定写进
  每个环境自己的 `config.json` / 固定名软链，注入器按轮询间隔（默认 5s）重读，
  所以运行中改**不需要重启**；
- **规则文件的内容热重载** —— 导入同名规则覆盖即可，注入器按目标文件的
  `(mtime, size)` 重读；
- **改名 / 换端口 / 换代理凭据必须先停止**。实例在启动时才固定监听端口与
  `--set proxyauth=…`，运行中改这几项会让「配置说换了、实例还按旧的干活」，
  所以服务端返回 `conflict` 并在消息里说明原因；
- **绑定一个不存在的规则名会被拒绝**（`environment.rules`）。在新语义下「规则缺失」不再让
  启动失败，而是**不覆盖任何域名** —— 于是绑定一个不存在的名字会变成一次静默失效，
  宁可在写入时就拒绝。

没有 mitmproxy 的机器可以用 `--core fake` 跑通管理器与全部测试（只监听端口、不改写）。

## 配置

| 键（CLI 参数） | 默认 | 说明 |
|---|---|---|
| `--state-dir` | `~/.envboard` | 环境账本、运行时、规则库、物化产物、共享 CA 都在它下面 |
| `--port-range` | `16000-16999` | 随机分配区间（避开常见服务端口与系统临时端口段） |
| `--core` | `mitmproxy` | `fake` = 只监听端口，用于没有 mitmproxy 的环境 |
| `--core-bin` | `mitmdump`（PATH） | core 可执行文件 |
| `--core-python` | 从 `core.bin` 推导 | CA 预物化用解释器。**默认不用环境里的 `python3`**：本机实测默认 python3 没装 mitmproxy，所以走"shebang → uv-tool 布局 → 真跑一次 import 自检"的探测，并要求版本与 `core.bin` 一致 |
| `--log-dir` | `<state_dir>/logs` | 实例日志目录。子进程的 stdout/stderr **直接写** `<env>.log`（没有管道、没有读线程），所以"没人读管道 → 管道写满 → 子进程阻塞 → 代理挂住"这条路径不存在；日志跨重启留存 |
| `--no-log-file` | 关 | 不落盘：日志只留在内存的有界环形缓冲里（随实例结束消失）。磁盘零写入，代价是回到"必须持续把管道读走"的形态 |
| `--max-log-bytes` | 8 MiB | 单环境日志文件上限，超过就 copytruncate 轮转（保留一份 `.1`）。`0` = 不轮转 |
| `--reload-interval` | 5s | 注入器轮询 `config.json` 与规则目标的间隔（写进每个环境的 `config.json`；收敛窗口也按它算） |
| `--api` / `--token` | 自动发现 | 本地 API 地址与令牌。发现顺序：`--api` → `<state_dir>/runtime/api.json`（含工作台写下的 token）→ 默认端口，**最后这一步只在没显式给 `--state-dir` 时才走** —— 否则 `--state-dir /tmp/x` 会被另一个状态目录的常驻实例接管 |
| `web --listen` | `127.0.0.1:8900` | 工作台监听地址 |
| `web --token` | 自动生成 | 显式指定工作台访问令牌；不指定时**默认启用鉴权并自动生成**（128 bit 随机值）。与 `--without-token` 互斥 |
| `web --without-token` | 关 | 显式关闭 token 鉴权（与 `--token` 互斥）。**仅限回环监听**：非回环拒绝关闭 —— 无鉴权对外不允许 |

非法配置**加载即失败**并给出字段路径（如 `environment.listen.port`）。

### 工作台鉴权与对外暴露

- **token**：鉴权默认启用 —— 未给 `--token` 时自动生成随机 token，启动日志打印
  `dashboard: http://<host>:<port>/?token=<值>`，点击即可在浏览器直接打开
  （浏览器与 SSE 的 EventSource 带不了自定义头，所以服务端同样接受 `?token=`；
  header `x-envboard-token` 优先）。前端拿到 URL 上的 token 后会立刻从地址栏抹掉，
  之后所有请求走 header。生成的 token 同时写入
  `<state_dir>/runtime/api.json`（0600），CLI 瘦客户端自动带上，无需手抄。
  注意 `?token=` 会出现在访问日志与浏览器历史里，API 调用请优先用 header。`/app.css` 与 `/app.js` 是内嵌静态资产（不含数据），豁免 token 检查 —— 浏览器拉取子资源时带不了凭据，不豁免开 token 必白屏。
- **代理访问鉴权**：环境有两个字段 `proxy_user` / `proxy_password`，**同生共死**
  （只有一边 → `invalid_config`，错误指向缺失的那一边）。每段非空、不含 `:`、空白或
  控制字符（`:` 会让 mitmproxy `proxyauth` 的 `split(":")` 歧义），`proxy_user` ≤64、
  `proxy_password` ≤128。启用后实例以 `--set proxyauth=<user>:<password>` 启动，
  客户端凭据才能连代理（否则 407）。凭据明文只落在两处：0600 的状态文件，以及实例的
  启动参数 —— **视图 / 日志 / SSE 只暴露「是否启用」布尔**；记录进程身份时 cmdline 里的
  凭据值先脱敏成 `***`。运行中修改要先停止实例；工作台表单**不回显**已保存的凭据
  （看不到 = 不覆盖，要改就填两个新的）。
- **对外服务**：环境监听默认 `127.0.0.1`；工作台表单的「对外服务」开关把
  `listen.host` 换成 `0.0.0.0`（也可 PATCH `listen` 显式指定）。勾选而未填代理凭据时
  表单会给警示 —— 代理暴露给局域网后任何能连通的机器都能借它发请求。

### HTTP 端点

工作台就是这些端点的全部对外面。**鉴权规则**：除 `/app.css` `/app.js` 两个内嵌静态资产
（不含任何数据，浏览器子资源请求带不了凭据）外，**每个端点都要求 token**（header 或
`?token=`，SSE 用后者）；变更类（非 GET/HEAD）还要求 `x-envboard-request: 1`（CSRF 防线），
且 `Host` 必须等于配置的监听地址（DNS rebinding 防线）。

| 方法 | 路径 | 用途 |
|---|---|---|
| GET | `/` | 工作台页面（内嵌 HTML 外壳） |
| GET | `/app.css` / `/app.js` | 内嵌静态资产（唯一豁免 token 的两个路径） |
| GET | `/api/status` | 管理器与 core 概况、`port_range` 等配置回显 |
| GET | `/api/environments` | 环境列表（工作台视图对象数组） |
| POST | `/api/environments` | 建环境（`name` 必填；`listen.port` 缺省 = 自动分配） |
| GET | `/api/environments/:name` | 单环境详情 |
| PATCH | `/api/environments/:name` | 改环境（未提及字段不动；`null` 清空；运行中允许描述 / `insecure_hosts` / 规则绑定） |
| DELETE | `/api/environments/:name` | 删环境（先停止） |
| POST | `/api/environments/:name/start` | 启动实例（期望状态置 running） |
| POST | `/api/environments/:name/stop` | 停止实例 |
| POST | `/api/environments/:name/restart` | 重启实例 |
| POST | `/api/environments/:name/reallocate` | 显式重分配端口（`port_conflict` 的解法） |
| GET | `/api/environments/:name/logs?lines=N` | 实例日志尾部（读 `<state_dir>/logs/<env>.log`） |
| GET | `/api/rules` | 规则库列表（名字 + 条数） |
| POST | `/api/rules` | 导入规则（`{name, text}`，覆盖同名） |
| GET | `/api/rules/:name` | 规则文件原文 |
| DELETE | `/api/rules/:name` | 删规则（仍被环境绑定时 `conflict`） |
| GET | `/api/compare?host=<域名>` | 跨环境静态对比：该域名在各环境被覆盖成什么（不发请求） |
| GET | `/api/events` | SSE 快照（每秒一次全量环境视图；EventSource 只能靠 `?token=` 鉴权） |

未匹配的路径返回 JSON 形状的 404（`{error:{code,message}}`），前端据此提示而不是"响应不是 JSON"。

## 架构

```
core/spec/            语言中立契约：能力清单、错误码、规则语法 BNF、68 个 golden fixture
core/rs/crates/
  envboard-core-api   ProxyCore trait、实例契约、错误码、端口（时钟/日志）  ← 根，无内部依赖
  envboard-domain     环境校验/合并、端口选择、reconcile 决策              ← 纯逻辑
  envboard-rules      hosts 解析 + 确定性渲染                              ← 纯逻辑
  envboard-core-fake  只监听端口的测试替身 core
  envboard-core-mitmproxy  ProxyCore 的 mitmproxy 实现（spawn / CA / 监督 / 探活）
  envboard-manager    环境 CRUD、端口分配、状态持久化、锁、健康检查、reconcile
  envboard-web        axum API + 内嵌前端（index.html / app.css / app.js）
  envboard-cli        envboard 二进制
  envboard-contract-tests  消费全部 68 个契约 fixture（只有测试目标）
  envboard-policy-tests    工程门禁本身（只有测试目标，不进发布物）
adapters/mitmproxy/   单文件注入器（只用标准库与宿主自带的依赖；被二进制 include_str! 内嵌）
scripts/systemd/      部署工件（用户级 unit）
```

依赖方向由工程门禁强制（`cargo test -p envboard-policy-tests --test deps`）：core-api 是根；domain / rules / core-api
是**纯逻辑**（不得依赖 tokio / libc）；web 只认识管理器的公开 API，不认识具体 core
（所以换 core 不必动它）。`adapters/` 只做宿主接线，不含业务逻辑。
`envboard-policy-tests` 是例外的一类 —— 它是**开发工具**，谁都不许依赖它，也不进发布物。

每个环境有自己的 agent 目录，注入器、配置与规则软链都在里面：

```
<state_dir>/agent/<env>/
├── envboard_mitmproxy.py   注入器（构建期内嵌进二进制，启动时物化）
├── config.json             ★ 管理器 → 注入器的唯一配置通道（含 insecure_hosts，热重载）
└── envboard.rules          固定名软链 → <state_dir>/rules/<name>.rules（不存在 = 不覆盖）
```

注入器只做四件事：读 `config.json`、读规则软链、在 `server_connect` 里改写上连目标、
按域名放宽上游证书校验（`tls_start_server`），外加回写状态文件。它只认**自己目录旁边的
固定路径**，所以不需要任何「配置在哪」的参数 —— 也因此配置与绑定都能靠「换文件/换软链 +
轮询」热生效。`config.json` 的字节是环境定义的确定性函数（不含时间戳）：管理器据此做
「要不要重写」与「期望哈希」的判定，而实例回执的 `config_hash` 就是「这份配置到底生效了
没有」的权威判据（不等但在收敛窗口内算收敛中，超窗才算 `config_mismatch`）。

## 验证

单一入口（托管方无关，CI 直接调它即可）：

```bash
bash ci/verify.sh            # 默认层：policy + rust + contract + artifact + adapter
bash ci/verify.sh rust       # 纯 Rust 子集：policy + rust（不需要 Python / Node）
bash ci/verify.sh live       # 实机层：真 mitmdump + 真改写 + 真管理器（需要宿主）
bash ci/verify.sh policy     # 只跑仓库纪律那一层
```

层、手段与外部依赖（脚本本身**只做编排**，判据全在测试里）：

| 层 | 手段 | 关键断言 | 需要什么 |
|---|---|---|---|
| `policy` | `cargo test -p envboard-policy-tests` | 见下面「工具链纪律」与「文本自包含」两节 | 只有 cargo |
| `rust` | `cargo fmt --check` / `clippy -D warnings` / `check` / `build` / `test --workspace` | 编译、lint、单测（端口分配、reconcile、锁、健康判定含僵尸判活、日志尾部与轮转、参数拼装、CLI 瘦客户端、**环境编辑：热改 vs 必须停机**） | 只有 cargo |
| `contract` | `cargo test -p envboard-contract-tests` | 68 个 fixture 的形状与语义都由实现消费（含 `rules.parse` 与 `insecure.hosts`）；失败用例必须钉住错误码、成功用例至少钉一个归一化字段 | 只有 cargo |
| `artifact` | `cargo test -p envboard-cli --test artifact` | 发布工件清单齐全、v1 残留为零、二进制里真的内嵌了注入器 | 只有 cargo |
| `adapter` | `cargo test -p envboard-rules --test dual_impl -- --ignored` | Rust 与注入器的解析/渲染输出**逐字节**一致（63 个用例；已知分歧必须显式声明，声明过期同样失败） | 适配器宿主的解释器 |
| `live` | `cargo test -p envboard-cli --test live_workbench -- --ignored` | 两个环境**同时**可用且结果不同、规则热重载、**按域名放宽上游校验（含运行中热改，以及名单外域名仍然 502 的对照）**、安全（Host / CSRF）、CSP 形态、日志、崩溃恢复、连打 320 个请求不卡死、实例崩溃可见且不留僵尸、**编辑后按新配置真的生效** | mitmdump + 网络 |
| `live` | 人工走查浏览器层（真浏览器 + `getComputedStyle` 对齐令牌） | JS 真的跑了 + 样式真的生效 + 无 CSP 报错（v1 的 CSP 教训）；转录是本机工作材料，不入库 | 真浏览器 |

解释器缺失时默认层**响亮失败**并给出两条出路（装适配器宿主，或跑 `verify.sh rust`），
不静默跳过 —— 跳过等于那一层的护栏消失。

实机与验收的**转录**是本机工作材料，不入库；可重跑的断言在 `bash ci/verify.sh live` 那一层。

工作台界面的视觉令牌、CSP 约束与交互纪律以 `core/rs/crates/envboard-web/assets/app.css`
第 ① 区为准（那里是机器可读的唯一来源）。

### 工具链纪律

**这个仓库只有一条工具链：`cargo`。** 除 `adapters/` 下的宿主适配器脚本外，不许有第二种
语言的工具链痕迹 —— 门禁自己也不许用别的语言写。这条约束由门禁自己证明，不是靠自觉：

| 判据 | 内容 |
|---|---|
| 文件面 | 不许有 Python / Node / TS 的源码与清单文件（`*.py`、`pyproject.toml`、`package.json`、`node_modules/`、`*.ts`、打包器配置…）；`node_modules/` 与 `__pycache__/` 单独判存在性 |
| 调用面 | 可执行面（`ci/*.sh`、`*.service`、CI 描述文件）不许出现 `npm` / `pip` / `mypy` / `pytest` 等命令，也不许 `<解释器> -m <工具>`；被执行的仓库内脚本只允许是适配器脚本 |
| 反向守卫 | `adapters/` 下仍是**单文件**注入器，import 只用标准库与宿主自带依赖的白名单 |

为什么这么较真：门禁必须与产品同一条工具链、同一个类型系统，否则就是"用另一套语言维护
这个仓库的工程纪律" —— 而那套语言的类型错误没人检查，门禁自身也会漂移。
`bash ci/verify.sh rust` 就是这条纪律的可执行形式：它在一个没有 Python、没有 Node 的
环境里也全绿。

**禁止**为了"写起来快"重新引入脚本语言写门禁。要加一条新门禁，就加一个新测试：
纯文本 / 结构门禁放 `envboard-policy-tests`（一门禁一文件），需要已构建二进制的门禁
放它所属 crate 的 `tests/`（用 `CARGO_BIN_EXE_<bin>` 取二进制），需要真宿主的门禁标
`#[ignore]` 并由 `ci/verify.sh` 的对应层用 `--ignored` 触发。`ci/verify.sh` 只做编排。

### 文本自包含（`doc-scope-lint`）

**入库的一切面向读者的文本必须自包含** —— `README.md`、`CHANGELOG.md`、`core/spec/**`
与代码注释都不例外。判据有三条，由 `cargo test -p envboard-policy-tests --test doc_scope`
机械执行：

1. **零命中不在仓库里的文档的指称**：不给路径、不给链接、不给章节号、不点名字。
   设计文档、开发计划、实机验收转录、工作区规范文件同在此列 —— 它们在本机 `.agents/`
   目录下或更外面，**不入库**，对 clone 本仓的人不可解析；
2. **指向本仓的路径必须真实存在**（写错的、或写完没建的一律失败）；
3. **`§` 引用必须带本仓锚点**：写明是哪份本仓文件（`core/spec/rules.md` §3.1），
   引用本文件自己的章节则写"本文件"。

该写什么：把结论**直接写在这里**，或指向本仓内**真实存在**的文件与标题。
**不在禁列**的是可复核的外部事实坐标 —— 宿主源码位置（`mitmproxy/addons/tlsconfig.py:291`）、
实测命令与输出、版本号、协议编号、外部 URL；它们是证据，不是"另一份只在本机存在的文档"。
仅 `CHANGELOG.md` 的历史条目豁免第 2 条：它记录的是当时存在、如今已删的文件。

## 运维（systemd）

`scripts/systemd/envboard.service` 是**用户级** unit（agent 只监听回环、状态目录在 `$HOME`
下、实例是当前用户的 mitmdump 子进程 —— 没有一处需要 root）：

```bash
install -Dm755 target/release/envboard ~/.local/bin/envboard
install -Dm644 scripts/systemd/envboard.service ~/.config/systemd/user/envboard.service
systemctl --user daemon-reload
systemctl --user enable --now envboard
systemctl --user status envboard
export ENVBOARD_STATE_DIR=$HOME/.local/state/envboard   # 命令行操作同一批环境
envboard env list
```

四条关键取舍（unit 文件里都有逐条注释）：

- **状态目录交给 systemd 建**：`StateDirectory=envboard` → `~/.local/state/envboard`（0700）。
  不用 `ReadWritePaths=` 是因为它要求路径在**挂载命名空间建立时**已存在，首次安装必失败
  （实测 `status=226/NAMESPACE`）。
- **`ExecStart` 全用绝对路径**：systemd 的 PATH 不含 `~/.local/bin`；给 `--core-bin` 绝对
  路径还有个附带好处 —— 管理器能从它的 shebang 推出 `core.python`。
- **`Restart=always` 是安全的**：期望状态（`desired=running`）已持久化，重启后 reconcile
  会把环境重新拉起来；主动 `stop` 不会被当成失败再拉起。
- **`KillMode=mixed`**：主进程退出时实例靠 `PR_SET_PDEATHSIG` 一起走，漏网的由 systemd
  对整个 cgroup 补 SIGKILL。用 `KillMode=process` 会留下孤儿实例继续占端口。

只要"环境常驻"不要 UI，把 `ExecStart` 换成 `run` 变体（unit 文件末尾有现成的两行）。
想在没有登录会话时也活着：`loginctl enable-linger $USER`（本机已是 `yes`）。
改配置别动这个文件，用 `systemctl --user edit envboard` 写 drop-in。

## 已知限制

- **上游连接复用会丧失**：改写后的地址与请求 host 不再相等，连接池匹配不上 ——
  被规则覆盖的域名每个请求都会新建上连（多一次 TLS 握手）。这是"只改连到哪"的代价，
  不是缺陷；正确性不受影响。
- **指向证书不匹配的测试机 → 上游校验失败 502**：这是 hosts 语义的必然（客户端看到的
  域名不变，所以证书也按那个域名校验）。解法是**按域名放宽**：
  - **`insecure_hosts`**：把该域名列进环境的放行清单（工作台「配置」表单，每行一个完整
    域名）。名单内的域名在 `tls_start_server` 里用 `VERIFY_NONE` 自建上游 context；
    **其余域名一律严格校验**，公网域名不受影响。匹配是**完全相等**：`365.kdocs.cn` 不会
    放行 `web.kdocs.cn`，也不会放行 `kdocs.cn.evil`；写入时**拒绝通配符** ——
    放宽校验必须逐条点名，偷偷扩大范围比不生效更危险。清单是**热**的：运行中改，
    注入器在轮询间隔（默认 5s）内跟上，不必重启。清单为空就是全部严格校验，
    **没有「整个环境全关」的开关**。
  - **想保留校验**：把目标机证书链上的 CA 拼进 mitmproxy 的信任库。工具链的信任库是自带的
    certifi（**不是操作系统信任库**），改它需要 `ssl_verify_upstream_trusted_ca` 这类选项 ——
    而任意选项透传通道**已经删除**，所以当前版本**没有**这条路径：要么按域名放宽，
    要么把证书换成公开 CA 能验的。这是有意的取舍（安全控制不留在「随便填个键值对」的入口上）。
  - 判据速查：日志里的 `unable to get local issuer certificate` 表示根/中间不在信任库 →
    该域名需要进 `insecure_hosts`；`hostname mismatch` 表示那台机器根本没有该域名的证书
    （进名单也救不了：名单只跳过校验，不改写 SNI）。
- **客户端要改代理配置**（换端口即换环境）。工作台给出的那行 `export https_proxy=…`
  就是为此。
- **`sni is None` 分支未经实测**：显式代理下 curl 总会带 SNI，构造不出该分支；目前只有
  源码核读结论（读的是注入器里那条 `sni is None` 的回退路径）。
- **实例日志默认落盘**：日志写在 `<state_dir>/logs/`，单文件上限 8 MiB（保留一份 `.1`）。
  默认详细度下日志里有请求的连接/流向信息（不含请求体，除非你把 `flow_detail` 调大）；
  不想让它落盘就用 `--no-log-file`，代价是回到"管道必须被持续读走"的形态。
- **Windows/Hyper-V 保留端口段未核实**：本机 WSL2 的 interop 被禁用，查不到 `netsh` 的
  保留段。已证实的是 Windows 侧浏览器能直接访问 WSL2 内绑定的 `127.0.0.1:<port>`；
  端口真起不来时环境会被标成 `port_conflict`，而不是静默换端口。
- **改名不会搬日志文件**：日志按环境名落盘，`beta` 改名 `gamma` 之后新日志写
  `<state_dir>/logs/gamma.log`，旧的 `beta.log` 留在原处（历史不丢，但不自动合并）。

## 契约

`core/spec/` 是语言中立契约，也是两份 hosts 解析实现的共同仲裁：

- `capabilities.md` —— 能力清单、领域不变量、端口分配与生命周期语义、ProxyCore 能力矩阵
- `rules.md` —— hosts 规则语法的 **BNF**（容错规则、跳过原因码、渲染的逐字节要求）
- `errors.md` —— 统一错误码与健康状态表
- `fixtures/` —— 68 个 golden case

改实现之前先问"契约该不该改"；该改就改契约并同步 fixture。

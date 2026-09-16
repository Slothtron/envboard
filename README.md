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
| 每实例选项（`ssl_insecure`、`ssl_verify_upstream_trusted_ca` …）**持久化在环境上**，CLI 与工作台都能改 | ✅ |
| 端口自动分配（区间内随机试绑，撞了就重试一次） | ✅ |
| 按 host 改写上连目标（真 mitmproxy `server_connect`） | ✅ |
| 共享 CA（所有实例一张，客户端只装一次） | ✅ |
| 工作台（环境列表 / 启停 / 编辑 / 规则导入与载入 / 跨环境对比 / 日志 / SSE） | ✅ |
| 工作台 URL token 鉴权（默认启用、自动生成；header 优先，`?token=` 等效） | ✅ |
| 代理访问鉴权（环境 `proxy_auth` 字段 → mitmproxy `proxyauth`，Basic 认证） | ✅ |
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
（同一个表单切到编辑模式，可改名字 / 端口 / 规则绑定 / 实例选项 / 描述），命令行是
`env edit`：

```bash
envboard env edit beta --rules beta            # 补绑规则（停止状态）
envboard env edit beta --port 16302            # 换端口
envboard env edit beta --rename gamma          # 改名（期望状态、端口归属一起搬过去）
envboard env edit beta --no-rules              # 解绑，回到"不覆盖"
envboard env edit beta --description "灰度 v2" # 只改描述
envboard env edit beta --option ssl_insecure=true                 # 透传给 core 的选项
envboard env edit beta --option ssl_insecure=true --option block_global=false
envboard env edit beta --no-options            # 清空全部选项（整体替换，不是逐键合并）
envboard rules show beta                        # 改规则文件前先取回原文，免得盲覆盖
```

`--option K=V` 也可以在建环境时直接给：`envboard env add beta --option ssl_insecure=true`。
选项**持久化在环境上**，启动与崩溃恢复（reconcile）都从它取值 —— 常驻工作台下也带得上，
不需要"先停工作台再用直连模式启动"。

工作台「配置」表单里是同一份配置的两个入口（「概览」会显示当前生效值）：

- **显式开关**：目前只做了 `ssl_insecure`（跳过上游证书校验）一个 —— 它是布尔选项里
  语义最确定、最常用的一项，用勾选框比让人手写 `K=V` 更不容易错。开关是该项**唯一**的
  编辑入口（文本框里重复写会报字段错误，避免同一份配置两个真相）；
- **「其他实例选项」文本框**：上面开关之外的 core 选项，每行一个 `K=V`
  （如 `ssl_verify_upstream_trusted_ca=/path/ca.pem`）。

加第二个开关：`assets/index.html` 加一块控件，`assets/app.js` 的 `OPTION_SWITCHES` 加一条，
键名两边一致即可（`formPayload` / `startEdit` / `syncEditLock` 会自动带上它）。

三条规则（服务端裁决，工作台与 CLI 只是同一份语义的两个面）：

- **描述随时可改**，改动不会重启实例；
- **规则文件的内容热重载** —— 导入同名文件覆盖即可，注入器按 mtime 重读，不必重启；
- **改名 / 换端口 / 换绑定 / 换选项必须先停止**。实例启动时才固定监听端口、规则文件路径
  与 `--set` 选项，运行中改这几项会让"配置说绑了 A、实例还按 B 干活"，所以服务端返回
  `conflict` 并在消息里说明原因（工作台会把这些输入框锁住，只放开描述）。

选项的键由 core 侧同一张 denylist 裁决（管理器自有键如 `listen_port` / `confdir`、
会改变进程拓扑或加载第三方代码的键如 `mode` / `scripts` / `web*`、以及 `envboard_`
前缀）；命中时**写入即失败**，错误路径是 `environment.options.<key>`，不会留下一份
启动时才爆炸的坏配置。

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
| `--reload-interval` | 5s | 注入器检查规则文件 mtime 的间隔 |
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
- **代理访问鉴权**：环境新增 `proxy_auth` 字段（`user:password`，恰好一个冒号、
  两段非空、无空白/控制字符、总长 ≤128；`null` = 不启用）。启用后实例以
  `--set proxyauth=<user:password>` 启动，客户端凭据才能连代理（否则 407）；
  凭据明文存于状态目录（文件 0600），
  视图与日志只暴露"是否启用"，不回显值本身；运行中修改要先停止实例。
- **对外服务**：环境监听默认 `127.0.0.1`；工作台表单的「对外服务」开关把
  `listen.host` 换成 `0.0.0.0`（也可 PATCH `listen` 显式指定）。勾选而未启用
  `proxy_auth` 时表单会给警示 —— 代理暴露给局域网后任何能连通的机器都能借它发请求。

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
| PATCH | `/api/environments/:name` | 改环境（未提及字段不动；`null` 清空；运行中限描述） |
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
core/spec/            语言中立契约：能力清单、错误码、规则语法 BNF、58 个 golden fixture
core/rs/crates/
  envboard-core-api   ProxyCore trait、实例契约、错误码、端口（时钟/日志）  ← 根，无内部依赖
  envboard-domain     环境校验/合并、端口选择、reconcile 决策              ← 纯逻辑
  envboard-rules      hosts 解析 + 确定性渲染                              ← 纯逻辑
  envboard-core-fake  只监听端口的测试替身 core
  envboard-core-mitmproxy  ProxyCore 的 mitmproxy 实现（spawn / CA / 监督 / 探活）
  envboard-manager    环境 CRUD、端口分配、状态持久化、锁、健康检查、reconcile
  envboard-web        axum API + 内嵌前端（index.html / app.css / app.js）
  envboard-cli        envboard 二进制
  envboard-contract-tests  消费全部 58 个契约 fixture
adapters/mitmproxy/   单文件、纯标准库的注入器（被二进制 include_str! 内嵌）
```

依赖方向由 `scripts/rust_dependency_lint.py` 强制：core-api 是根；domain / rules / core-api
是**纯逻辑**（不得依赖 tokio / libc）；web 只认识管理器的公开 API，不认识具体 core
（所以换 core 不必动它）。`adapters/` 只做宿主接线，不含业务逻辑。

注入器只做三件事：读+解析规则、在 `server_connect` 里改写上连目标、回写状态文件。
它不知道自己是哪个环境，也不碰状态目录 —— 所以随时可以丢掉。

## 验证

单一入口（托管方无关，CI 直接调它即可）：

```bash
bash ci/verify.sh            # 默认层：编译/命名/文本自包含/依赖方向/格式/clippy/单测/契约对拍/产物校验/冒烟
bash ci/verify.sh rust       # 只跑 Rust 相关
bash ci/verify.sh live       # 实机层：真 mitmdump + 真改写 + 真管理器（需要宿主）
```

| 层 | 手段 | 关键断言 |
|---|---|---|
| 文本纪律 | `scripts/naming_lint.py` + `scripts/doc_scope_lint.py` | 代码身份零命中发包标识；**入库文本不得引用不在仓库里的本地文档**（见下） |
| 契约 | `cargo test -p envboard-contract-tests` + `scripts/verify_contract.py` | 58 个 fixture 由实现消费；`rules.parse` 两侧一致 |
| 跨语言对拍 | `scripts/verify_dual_impl.py` | Rust 与注入器的解析/渲染输出**逐字节**一致（63 个用例；已知分歧必须显式声明，声明过期同样失败） |
| 单元 / 集成 | `cargo test` | 端口分配、reconcile、锁、健康判定（含僵尸判活）、日志尾部与轮转、参数拼装、CLI 瘦客户端、**环境编辑（热改 vs 必须停机）** |
| 实机 | `scripts/verify_live_v2.py` | 两个环境**同时**可用且结果不同、规则热重载、安全（Host / CSRF）、CSP 形态、日志、崩溃恢复、连打 320 个请求不卡死、实例崩溃可见且不留僵尸、**编辑后按新配置真的生效** |
| M0.5 spike | `scripts/spike_m0_5.py` | 共享 CA 配对、HTTPS 改写与 SNI、连接复用、PDEATHSIG |
| 浏览器 | 人工走查（真浏览器 + `getComputedStyle` 对齐令牌；转录是本机工作材料，不入库） | JS 真的跑了 + 样式真的生效 + 无 CSP 报错（v1 的 CSP 教训） |

实机与验收的**转录**是本机工作材料，不入库；可重跑的断言在 `bash ci/verify.sh live` 那一层。

工作台界面的视觉令牌、CSP 约束与交互纪律以 `core/rs/crates/envboard-web/assets/app.css`
第 ① 区为准（那里是机器可读的唯一来源）。

### 文本自包含（`doc-scope-lint`）

**入库的一切面向读者的文本必须自包含** —— `README.md`、`CHANGELOG.md`、`core/spec/**`
与代码注释都不例外。判据有三条，由 `scripts/doc_scope_lint.py` 机械执行：

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
  域名不变，所以证书也按那个域名校验）。两条解法，按安全性与代价取舍：
  - **保留校验**：把目标机证书链上的 CA 交给 mitmproxy。工具链的信任库是自带的
    certifi（**不是操作系统信任库**），所以 `--option ssl_verify_upstream_trusted_ca=…`
    是唯一入口，且该选项是**替换**默认库而不是追加 —— PEM 必须是
    `certifi 的 cacert.pem + 你的内部根 CA` 拼起来（只给内部 CA 会让所有公网域名一起 502）。
    目标机若只发叶子证书、不发中间证书，拼接时要把中间证书一起放进去。
  - **放弃该环境的上游校验**：`--option ssl_insecure=true`（只建议用于内网测试环境）。
  - 判据速查：日志里 `unable to get local issuer certificate` = 根/中间不在信任库；
    `hostname mismatch` = 那台机器根本没有该域名的证书。
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
- `fixtures/` —— 58 个 golden case

改实现之前先问"契约该不该改"；该改就改契约并同步 fixture。

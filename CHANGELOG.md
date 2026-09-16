# Changelog

All notable changes to `slothtron-envboard` are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versioning follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

v2 把 envboard 从「一个 mitmproxy 进程内的运行时开关」改成**多环境管理器**
（一个环境 = 一个独立代理实例 + 一个独立端口，附 Rust 工作台），落地分支
`feat/rust-multi-env-manager`（从 `v0.1.0` 切出）。以下是**已经落地的契约层改动**——
它们先于实现改动，因为契约是两份实现的裁定依据。

### Changed (breaking)

- `core/spec/capabilities.md` **重写**：领域模型从 v1 的 8 字段收为 4 字段
  （`name` / `listen` / `rules` / `description`）。删除 `dns_servers`、`hosts`、
  `domain_suffix`、`color`、`labels` 与 mapping / annotate / activate / resolve
  全部语义 —— v2 的切换模型是"一个环境 = 一个独立实例 + 一个独立端口"，
  不再有"当前环境"这个全局状态。
- `core/spec/errors.md`：删除 `dns_failure`、`disabled` 与 `rcode` 表；
  新增 `port_conflict`、`port_range_exhausted`、`config_mismatch`，并补上
  环境健康状态表（`stopped` / `starting` / `running` / `port_conflict` /
  `config_mismatch` / `unhealthy` / `failed`）。
- 契约 fixture **重新基线化**：v1 的 11 个 `environment/` + 2 个 `merge/` 全部重写
  （它们测的是被删除的字段），新增 `ports/` 与 `lifecycle/` 两组。
  现状：`environment/` 15 + `merge/` 5 + `rules/` 8（**原样保留**）+ `ports/` 6
  + `lifecycle/` 6 = **40** 个。

### Added

- `core/spec/rules.md`：hosts 规则语法的 **BNF**，作为 Rust 实现与 Python 注入器的
  共同仲裁（v1 只有散文式描述，无法支撑"两侧输出逐字节一致"这条要求）。

### Added

- **Rust 核心（v2 M0 + M1）**：多环境管理器的实现落地，由 7 个 crate 组成，
  依赖方向由 `scripts/rust_dependency_lint.py` 强制（core-api 是根，纯逻辑 crate
  不得依赖运行时）。已实现的部分：
  - `envboard-core-api`：`ProxyCore` trait、`InstanceSpec`、`StatusReport`、统一错误码、
    `ClockPort`/`LoggerPort`、`InstanceSpec.options` 的 denylist。
  - `envboard-domain`：4 字段环境模型（校验/归一化/PATCH 合并）、端口选择
    （`port.allocate` 的全部规则）、reconcile 决策（孤儿清理与 PID 复用保护）。
  - `envboard-rules`：hosts 解析与**确定性渲染**，与 Python 侧在 63 个用例上逐字节一致。
  - `envboard-core-fake`：只监听端口的测试替身 core —— **管理器测试因此完全不依赖
    mitmproxy**（"没装 mitmproxy 的机器也能跑默认流水线"这条验收条件）。
  - `envboard-manager`：环境 CRUD、随机端口分配与持久化、状态存储（原子写 + 0600）、
    单实例锁（flock）、健康判定（状态文件为主 + 契约回显比对）、reconcile、规则导入。
  - `envboard-cli`：`envboard` 二进制（`env`, `rules`, `run`, `status`）。
  - `envboard-contract-tests`：**消费 `core/spec/fixtures` 全部 40 个 golden case**。
- `scripts/verify_dual_impl.py`：跨语言对拍门禁 —— 63 个用例里 Rust 与 Python 输出必须
  逐字节一致，**已知分歧必须显式声明**（声明了却不再分歧也会失败，防止白名单掩盖新分歧）。
- `scripts/rust_dependency_lint.py`：Rust 侧的依赖方向与纯度门禁（含"纯逻辑 crate 不得
  依赖 tokio/libc"、workspace 根唯一、`Cargo.lock` 与 `rust-toolchain.toml` 在位）。
- `ci/verify.sh` 新增步骤：`rust-dep-lint` / `rust-fmt` / `rust-clippy` / `rust-check` /
  `rust-build` / `rust-test` / `contract-dual`，全部带 `--locked --offline`。

### Added（M2 — mitmproxy core + 注入器）

- `adapters/mitmproxy/envboard_mitmproxy.py`：**单文件、纯标准库**的注入器。
  只做三件事：读+解析规则（契约 BNF）、在 `server_connect` 里改写上连目标、
  回写状态文件。它不知道自己是哪个环境，也不碰任何状态目录 —— 所以随时可以丢掉。
  既是 addon（`-s`），也有 `--render` 模式供跨语言对拍（该模式**不需要 mitmproxy**）。
- `envboard-core-mitmproxy`：`ProxyCore` 的真实实现 —— 按启动契约拼 `--set` 参数、
  spawn `mitmdump`（带 `PR_SET_PDEATHSIG`）、**预物化并校验共享 CA**、
  物化注入器、等状态文件而非等进程、SIGTERM→SIGKILL 停止、按身份探活。
- `core.python` 的推导规则落地：shebang → uv-tool 布局 → 同目录，**每一步都真跑一次
  `import mitmproxy` 自检**，并要求与 `core.bin --version` 的版本一致。
  裸命令名（默认 `mitmdump`）先经 PATH 解析 —— 否则读不到 shebang，
  实测会报"找不到带 mitmproxy 的解释器"而它其实就在 PATH 上。
- **契约回显自检机制**：注入器核对"管理器下发的每个选项"是否被宿主接受
  （`options_echo`），管理器据此判定 `config_mismatch`。宿主对拼错的 `--set`
  是静默忽略的，只有让注入器去 `ctx.options` 里查才知道。
- 实机测试 `live_manager.rs`：真管理器 + 真 mitmdump + 真改写（200 vs 502 判别性对照）
  + 假选项触发 `config_mismatch`；没有 mitmdump 时**跳过**而不是失败。

### Changed（日志通道与存活判定：从"读管道"改成"文件直写"）

- **实例日志默认改为文件直写**（`<state_dir>/logs/<env>.log`）：子进程的 stdout/stderr
  用 `Stdio::from(File)` 直接接文件（两路 `try_clone()` 共享 offset），**没有管道、
  没有读线程**。原来靠"两个读线程把管道读走"来避免子进程阻塞；实测管道容量 64 KiB、
  默认详细度约 307 字节/请求 → **213 个请求就能写满**，写满后 mitmdump 阻塞在自己的
  事件循环里、所有客户端一起挂住（不读管道的对照实验：第 212 个请求开始失败，读走
  65116 字节后立刻恢复）。文件由内核写盘，我们进程不在链路上，这条路径从设计上消失；
  顺带换来"日志跨重启留存、崩溃现场可追溯"。
  - 每次启动追加一行运行标记分段落；`PYTHONUNBUFFERED=1` 两种形态都保留
    （Python 对非 TTY 的文件同样是块缓冲）。
  - 新增 `--no-log-file`（退回"管道 + 内存环形缓冲"，磁盘零写入）与
    `--max-log-bytes`（默认 8 MiB，`0` = 不轮转，下限 64 KiB）；`--log-dir` 由
    "可选落盘目录"变为"覆盖默认目录"。
- **轮转是 copytruncate**（尾部搬去 `<env>.log.1` 后 `set_len(0)`）：**不能 rename** ——
  子进程还持着那个 inode 的 fd，改名之后它会继续写被移走的文件，新文件永远是空的。
- **读尾部改为有界**：只从文件末尾读 256 KiB 窗口再切行，并可跨 `.1` 往前拼；
  删掉原来整个文件 `read_to_string` 的兜底（日志一大就会把管理器拖住）。
- **判活看 `/proc/<pid>/stat` 的状态位**：僵尸（`Z`/`X`）不再算活着。原来只比
  PID + starttime，而僵尸这两个量都没变 → 判成"活着"。新增每 500 ms 的**回收任务**
  用 `try_wait()` 收尸（tokio 1.53.1 该方法是公开同步的）。一处修好，`health`
  / `reconcile` / 孤儿清理 / `terminate` 全部同时受益。
- **视图层与权威判定同源**：`health_blocking()` 原来"只要句柄表里有这个环境就返回
  `running`"，与带状态文件 TTL 的 `health()` 分叉；实测 `kill -9` 掉实例后工作台
  **60 秒仍报 `health=running, reason=None`**。现在两条路径共用同一套判据
  （进程存活 → 状态文件时效 → 契约回显），并且"由本管理器拉起、又没人叫它停、
  进程却没了"报 `failed`（带退出码或信号）而不是 `stopped`。
- 管道路径（`--no-log-file`）硬化：单行 8 KiB 上限、写盘不再每行 open/close、
  `Mutex` 中毒不再 `unwrap()`（一个读线程 panic 会连带把另一个也弄死，管道就再没人读了）。
- 工作台日志面板跟随 SSE 快照刷新（原来只在点"日志"时取一次，之后永远停在那一次快照上）。

### Added（文本自包含门禁）

- **`scripts/doc_scope_lint.py`**：包内文本不得引用本仓之外的本地文档（设计文档、开发计划、
  流程规范、工作区规范文件）：不给路径/链接/章节号、不点名字；指向本仓的路径必须真实存在；
  含 `§` 的行必须写明是哪份本仓文件（或"本文件"）。纳入 `ci/verify.sh` 默认层
  （`doc-scope-lint` 步骤），判据与豁免见 README 的「文本自包含」一节。
- 按同一条规则清理了包内既有引用：README / CHANGELOG / `core/spec/**` / 验收文档与
  **代码注释**里对外部文档的章节号引用、风险表编号（如 `R14`）一律改成自包含表述或指向
  本仓内真实存在的文件与标题；顺带修掉两处失效引用（对拍入口指向已删除的脚本、
  清单里"断言已不存在"的路径改为显式标记）。

### Added（部署工件）

- `scripts/systemd/envboard.service`：用户级 systemd unit（常驻管理器 + 工作台），
  附 headless（`run`）变体。设计取舍都在文件内注释：`StateDirectory=` 建状态目录
  （`ReadWritePaths=` 要求路径预先存在，首次安装会以 `226/NAMESPACE` 失败 —— 实测踩到）、
  `ExecStart` 全用绝对路径（systemd 的 PATH 不含 `~/.local/bin`）、`Restart=always` 的
  安全性来自持久化的期望状态、`KillMode=mixed` 避免孤儿实例占端口。
  已在真机跑完一轮启停：实例随服务停止而退出、重启后 reconcile 自动拉起、经代理的请求 200。

### Added（日志通道与存活判定）

- `envboard-manager/src/logs.rs`：日志文件的有界尾部读取（含跨轮转拼接）与 copytruncate
  轮转；日志文件落在管理器的状态目录里，所以**读与轮转都只有这一个实现**，
  core 侧不重复一份。
- `ProxyCore::last_exit`：进程**自己退出**时的退出原因（信号/退出码），供同步的视图层
  在不 `await` core 的前提下给出准确结论。
- 实机断言：`verify_live_v2.py` 新增 check 8（连打 320 个请求——足以越过旧的 64 KiB
  卡死阈值——全部成功，日志文件 97 KB 为证）与 check 9/9b/9c（实例被 SIGKILL 后工作台
  不再报 running、说清是被信号杀死、子进程被回收、日志仍可读、能重新拉起）；
  `live_manager.rs` 新增同款崩溃用例并覆盖 reconcile 重新拉起。证据与实测数字见
  `docs/acceptance/log-channel.md`。

### Fixed（日志通道与存活判定）

- `view_of` 有调用点是在**持有 `state` 锁**时调用 `health_blocking` 的，而
  `std::sync::Mutex` 不可重入 —— 判定路径里再取一次状态锁会让 `env add` 直接挂死。
  现在环境定义由调用方传入，判定路径不碰状态锁。

### Added（M3 — 工作台）

- `envboard-web`：axum 路由 + **内嵌前端**（`index.html` / `app.css` / `app.js`，
  全部 `include_str!`，零前端构建链、零 CDN）。
- REST 面：环境 CRUD/启停/重启/**显式重分配端口**/日志尾部、规则导入/查看/删除、
  **跨环境静态对比**（承接 v1 `all_envs` 的用例意图，且不发任何请求）、SSE 快照。
- 安全（三条都不可省）：默认只监听回环、Host 头校验防 DNS rebinding、
  变更类路由要求自定义头防 CSRF；非回环监听必须给 token。
- 严格 CSP（无 `unsafe-inline`），因此**样式与脚本必须外置**；验收断言三件事：
  JS 真的跑了 + 样式真的生效（读计算样式）+ 控制台无 CSP 报错。

### Fixed

- 端口探测改为**惰性**（找到第一个空闲端口即停）。本机实测单次 `bind` 约 **21ms**
  （WSL2 mirrored 模式，连绑定端口 0 也一样），按原设计"先把整个区间探一遍"会在
  16000–16999 上花约 21 秒；现在常见情形只探 1 次，最坏仍是 `max_attempts` 次。
- 注入器**不能**写 `from __future__ import annotations`：mitmdump 用 `-s` 加载脚本时
  不登记 `sys.modules`，而 `@dataclass` 解析字符串注解会去查它 → 启动即崩。
- mitmproxy 的选项系统**不支持 `float`**（启动即失败），间隔类选项一律用整秒。
- axum 0.7 的路径参数语法是 `:name`（`{name}` 是 0.8 的），写错的表现是**全 404**；
  另给 API 加了 JSON 404 兜底，避免前端只能报"响应不是 JSON"。

### Added（编辑已有环境）

- **工作台的「编辑」**：每行一个入口，把「新建环境」表单切到编辑模式（名字 / 端口 / 规则
  绑定 / 描述四个字段全可改），提交走 `PATCH /api/environments/:name`。之所以复用同一个
  表单而不是行内编辑：字段集本来一样，两套 DOM 迟早不一致（v1 的教训）。
  实例**运行中**会把名字 / 端口 / 绑定三个输入框锁住并说明原因，只放开描述 ——
  服务端仍会拒绝，但界面先把这条路封住，免得用户改完再吃一个红字。
- **规则库的「载入」**：把已导入的规则文件原文取回表单再改。原来只有「导入」，
  改一份规则等于盲覆盖（实测 10.8 KB 的规则文件就是这么被当成"没法编辑"的）。
- **CLI `env edit`**：`--rename` / `--port` / `--rules` / `--no-rules` / `--description`，
  空 patch 响亮失败（而不是"成功但什么都没改"）。与工作台**共用同一份** PATCH 构造，
  避免两处字段映射漂移。`ApiClient` 因此补了 `patch()`。
- **CLI `rules show`**：取回规则文件原文（改之前先看，同样是防盲覆盖）。
- 实机断言：`verify_live_v2.py` 新增 check 10 —— 运行中换绑定返回 409、描述热改 200、
  停止后补绑规则并改名换端口后，**真 mitmdump 按新配置把域名送到上游**（未绑定时是 502）。
  管理器侧新增 4 条用例（描述热改 / 绑定需停机 / 改名搬迁 / 旧标记作废），
  `thin_client.rs` 覆盖 `env edit` 走 HTTP 的往返。证据见
  `docs/acceptance/edit-environment.md`。

### Changed（编辑语义）

- **运行中换规则绑定改为拒绝**（`conflict`）。原来 `update()` 的注释写"改绑定是热更新"，
  与事实不符：实例在启动时通过 `--set envboard_rules=<path>` 固定规则**路径**，运行中换绑定
  它根本看不见，结果是"配置说绑了 A、实例还按 B 干活"。热重载的**是内容**（mtime 轮询
  同一个路径），这条能力不变。描述仍然随时可改。
  这个分叉是被一条单元测试逮到的：断言"热改绑定后仍是 running"，实际得到 `config_mismatch`
  —— 状态文件的 `rules_count` 回显如实报出了不一致。
- 改了 `listen` 或 `rules` 绑定即作废该环境旧的失败标记（`port_conflict` /
  `config_mismatch`）：标记陈述的是"上一个配置失败了"，留着它会让界面拿**新**端口号报旧冲突。
- `update()` 补一条审计日志（`updated <env>: changed name, listen, …`），
  系统日志里能看到"谁在什么时候动了哪个环境的哪几项"。
- **CLI 的本地 API 发现不再跨状态目录兜底**：显式给了 `--state-dir` 时只认该目录下的
  `runtime/api.json`，不再退回默认端口。原来 `--state-dir /tmp/scratch` 会被 8900 上
  一个跟它毫无关系的常驻实例接管 —— 命令看着成功了，改的却是**别人的**状态
  （本机实测撞上：`env add` 打到了正在运行的工作台上）。

### Fixed（编辑已有环境）

- **改名之后日志面板指向旧名字**：面板的目标名字不会跟着改，之后每秒一次的尾部读取都是
  404；而那个 404 会让整次 `refresh()` 失败，把"已保存。"顶成一条红字错误提示
  （真浏览器验收时踩到：用户会以为改动没生效）。现在面板目标跟着环境列表自愈，
  日志读取的失败也只落在日志面板里。
- **名字输入框的 `pattern` 在 `v` 标志下是非法正则**：Chrome 校验 `pattern` 用 `v`
  （不是 `u`），而 `v` 模式里裸的 `-` 是非法字符类成员 —— 于是整个 pattern 被浏览器
  **静默忽略**，只在控制台留一行 `SyntaxError`，原生校验形同不存在。转义成 `\-` 后
  两种模式都合法（`9bad` / `Beta` 现在真的会被前端挡下）。这类故障 curl 断言看不出来，
  只有真浏览器的控制台会说话。
- `doc-scope-lint` 漏掉**无后缀文本文件**：按后缀筛文件时 `.gitignore` 整类不在扫描范围内，
  而它的注释同样可能指向本仓之外的文档（实测确实有一条）。现在把 `.gitignore` / `.dockerignore`
  显式列入扫描集合（文件数 111 → 112）。

### Changed（文档脱敏）

- `docs/acceptance/v0.1.0.md`（v1 实机验收转录）里的家目录用户名与当时读到的**真实系统 DNS 地址**
  换成通用值：家目录统一写成 `/home/u/`（形状不变），DNS 地址改为文档专用网段 `192.0.2.53`
  （RFC 5737）。断言、时间戳、退出码一并保持原样 —— 脱敏只改"能指回某个具体人/某台真实机器"
  的部分。文件开头写明了这次替换，避免读者以为它就是当时的原始输出。
- 同一轮排查确认**真实 hosts 文件从未入库**（`examples/hosts.txt` 一直在忽略之列，
  入库的是脱敏示例 `examples/hosts.sample.txt`，用的是 RFC 5737 地址与 `example.com`）。

### Removed（M4 — 收敛与迁移，breaking）

- **v1 的 Python 实现整块移除**：`src/envboard/`（model / registry / mapping / resolver /
  rules / ports / errors / addon / web）、`addons/envboard.py`、`web/*`、v1 的 81 个单测，
  以及 `pyproject.toml`（本包**不再发布 Python 分发物** —— Python 这一侧只剩被二进制内嵌的
  单文件注入器）。`git tag v0.1.0` 保留 v1 全貌。
- 随之退休的门禁脚本：`dependency_lint.py`（分层检查已由 `rust_dependency_lint.py` 覆盖）、
  `pack_check.py`（改校验二进制产物）、`verify_live.sh`（55 条断言里能脚本化的部分迁到
  `verify_live_v2.py`）。
- 命名门禁**收紧**：没有分发物之后，`slothtron` 这个 token 在整棵树里（路径与代码内容）
  **零命中**，唯一例外是规则脚本自身。

### Notes

- `scripts/artifact_check.py` 取代 `pack_check.py`：校验发布物（二进制 + 内嵌注入器 +
  必需产物 + 零 v1 残留），并断言注入器源码真的被 `include_str!` 嵌进了二进制。
- `ci/verify.sh` 的 `smoke` 换成 v2 形态：二进制能跑 + `--core fake` 建环境 + 注入器
  `--render` 在**没有 mitmproxy**的解释器下也能跑。
- 实机层脚本化（`scripts/verify_live_v2.py`）：两个环境同时可用且结果不同、规则热重载、
  Host/CSRF 防护、CSP 形态、日志落盘、崩溃恢复 —— 补上了 v1 立下的
  "断言必须可重跑"这条规矩在 v2 上的缺口。

### Changed（工作台 UI 升级：明亮主题三视图）

- 前端三资产（`core/rs/crates/envboard-web/assets/` 下的 `index.html` / `app.css` /
  `app.js`）整体重写：从"单表格 + 深色"升级为明亮主题的三视图工作台 —— 环境 /
  规则库 / 跨环境对比三个视图、统计卡、环境详情（概览 / 配置 / 规则三页签）、
  可折叠日志栏（按事件类型过滤 + 搜索 + 跟随开关）。**REST API、SSE 与 CSP 形态
  零改动**，这是纯前端层的重写。
- 设计令牌收进 `app.css` 第 ① 区：表面三级、边框两级、文本四级灰阶（带对 `--panel`
  的实测对比度）、语义色「原色 + bg(10%) + border(30%)」三件套、4px 间距基数、五档
  圆角。组件层不写十六进制色值，主题切换只改令牌区。规范与理由见 `docs/ui-spec.md`。
- 破坏性操作（删除环境）改为**就地二次确认**（后果说明 + 确认/取消），不再用原生
  `confirm()` —— 与"无内联脚本"的 CSP 约束一致，且样式可控。
- 图标改用同文档 SVG `<symbol>` 精灵 + `<use>`：不引字体、不发请求，
  `stroke: currentColor` 使同一份图标随语境变色。
- 可访问性：`prefers-reduced-motion` 降级；`[hidden]` 加 `display:none!important` 修正
  （作者样式会盖过 UA 默认值）；文本四级灰阶全部达到可读对比度。
- 日志栏过滤维度是**事件类型**（请求/响应/连接/运行/异常）而不是日志级别：实测日志行
  不带 INFO/WARN 前缀，按级别过滤会是个假功能。

### Fixed（工作台 UI 升级）

- **引导链隔离**：`renderLogs` 排在 `connectEvents()` / `refreshAll()` 之前且无保护，
  一个写错的容器 id（`log-filters` 改名只落在 CSS/JS 两侧，HTML 未跟上）就能让
  `renderLogFilters` 抛空指针，进而中断整条引导链 —— 数据请求根本不发、页面停在空壳、
  `envboardReady` 永远停在 `"pending"`。现在每个引导步骤过 `step()` 壳：坏掉的步骤记进
  `dataset.envboardError`、ready 明确置 `"no"`，数据加载照常继续。
- `index.html` 注释里写出的脚本标签字面量会命中 `scripts/verify_live_v2.py` 的
  「页面源码里无内联脚本」断言（实机层 4a FAIL）—— 注释改写为不出现该字面量。

## [0.1.0] - 2026-09-15

### Added
- Multi-environment registry: name + DNS servers + domain suffix + static host
  overrides + colour/description, with CRUD and an active-environment switch.
  Persisted atomically to `<confdir>/envboard.json` with mode `0600`.
- Dynamic DNS server discovery, three sources: the environment's own
  `dns_servers`, runtime edits through the dashboard, and
  `mitmproxy_rs.dns.get_system_dns_servers()` for the OS configuration.
- Forward resolution (host → ip) via a per-environment
  `mitmproxy_rs.dns.DnsResolver` (Rust / hickory).
- Multi-environment comparison: resolve the same hostnames against every
  environment's DNS servers in one call (`all_envs`).
- Passive collection of host/ip mappings from `dns_response` when running with
  `--mode dns`.
- Bidirectional mapping index with per-environment sharding, TTLs, provenance
  (`static` / `passive` / `active`) and source precedence. Forward only —
  there is no ip → host reverse lookup by design.
- Flow annotation (`flow.comment` and/or `flow.metadata`) for hosts that match a
  known environment.
- Dashboard served from mitmweb at `/envboard/`, inheriting mitmweb's
  authentication, `Sec-Fetch-Site` guard and XSRF cookie handling. No second web
  server, no second auth system.
- Rules files: import a hand-written hosts-style file (`ip host…` or the reverse
  `host… ip`, several hosts sharing one IP, comments, blanks) and get a
  deterministic normalized rules file. Invalid content is ignored per item and
  reported (line + reason), never fatal; conflicting hosts resolve last-wins.
  One rules file is **bound per environment**, so switching environment switches
  the rules file. `Environment.hosts` still wins over the bound rules file.
- `examples/hosts.sample.txt`, a sanitized import sample (the real
  `examples/hosts.txt` is gitignored — it carries internal IPs and hostnames).
- 16 `envboard.*` mitmproxy commands giving full CLI/console parity with the
  dashboard (5 new: `rules.list` / `rules.import` / `rules.show` / `rules.bind`
  / `rules.remove`).
- `ci/verify.sh` verification entrypoint: compile-check, dependency-lint,
  unit tests, contract golden fixtures, best-effort mypy, pack-check and a smoke
  test. Reachable as `npm run verify`.
- `scripts/verify_live.sh` end-to-end check against a real mitmweb instance.

### Fixed
- `Environment.hosts` keys were stored **un-normalized**, so `API.Example.COM.`
  or `*.wild.example.com` never matched the normalized lookup host — a silent
  no-op, and contrary to what `core/spec/capabilities.md` promised. Hosts keys
  are now normalized on construction (`*.` prefix stripped, per the contract),
  and two keys collapsing to the same host is now a loud `invalid_config`.
- The dashboard's JavaScript was inline in `index.html`, which mitmweb's CSP
  (`default-src 'self'`, no `script-src`) makes browsers **refuse to execute**.
  The page still returned 200 and rendered, so the curl-based live check passed
  while the dashboard was in fact inert in a real browser (no data loaded, no
  button worked). The script now lives in `web/app.js`, served by a same-origin
  `AssetHandler`; `pack_check.py` rejects any inline `<script>` and
  `verify_live.sh` asserts the external asset is served.

### Known limitations
- `mypy` is not vendored; `typecheck` degrades to a skip with an explicit notice
  when it is unavailable.
- `envboard.resolve` is an asynchronous command implemented as "schedule + read
  the mapping table", because mitmproxy has no awaitable command path. The REST
  endpoints await properly and return results directly.
- Environment switching is observation-only (L1). Traffic rewriting (L2) is
  deliberately out of scope for this version.

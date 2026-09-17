# envboard 能力契约

语言中立契约：实现可以换语言、换宿主，但这里的语义与
`fixtures/` 的 golden case 是**裁定依据**。

> **v2 说明**：v2 把架构从"一个 mitmproxy 进程内的运行时开关"改为
> "一个环境 = 一个独立代理实例 + 一个独立端口"。
> 本文件按新模型**重写**，不是扩展：v1 的 mapping / annotate / activate / resolve
> 语义与 `dns_servers` / `hosts` / `color` / `domain_suffix` 四个字段**已删除**，
> 相关不变量不再存在。删除清单见文末。

## 能力清单

| 能力 id | 契约 | fixture 目录 |
|---|---|---|
| `environment.validate` | 环境定义合法性；归一化与默认值；错误码与 `field` 路径 | `fixtures/environment/` |
| `environment.merge` | patch 语义（部分更新；未提及字段保持不变；**显式 `null` = 清空**；合并后重新校验） | `fixtures/merge/` |
| `rules.parse` | hosts 风格输入 → 规范化规则；非法内容忽略；冲突后出现者胜；确定性渲染 | `fixtures/rules/` |
| `insecure.hosts` | 按域名放宽上游证书校验：`insecure_hosts` 的归一化/校验与**精确相等**命中判定 | `fixtures/insecure/` |
| `port.allocate` | 端口选择语义（候选序列 → 选中的端口）；显式指定；冲突；重试耗尽；区间外已分配端口保留 | `fixtures/ports/` |
| `instance.reconcile` | 期望状态与实际状态对齐：reconcile 顺序、孤儿清理判定、端口冲突标记 | `fixtures/lifecycle/` |

`fixtures/` 只放**纯逻辑**的确定性 golden。真正依赖网络与宿主的部分（拉起进程、
探活、CA 物化、浏览器行为）不进契约，由 `README.md` 的支持矩阵与 `ci/verify.sh`
的实机层覆盖 —— 这条边界与 v1 一致。

## 依赖图

```
environment.validate  ←  environment.merge
                      ←  insecure.hosts        （`insecure_hosts` 的语义由它裁定）
                      ←  port.allocate
                      ←  instance.reconcile   （经 port.allocate 取端口）
                      ←  rules.parse          （经 environment.rules 绑定）
                      ←  manager.*            （CRUD / 启停 / 状态持久化）
                      ←  adapter.webapi.*     （REST 面）
```

`environment.validate` 与 `rules.parse` 是根能力。缺依赖时必须 `invalid_config` 失败，
**禁止静默降级**。

## 领域不变量

### 环境（`Environment`）

v1 有 8 个字段，v2 收成 5 个，v2.1 增加到第 6 个 `proxy_auth`。**当前模型是 7 个字段**：
任意的 `options` 透传**已删除**、`proxy_auth` **已拆成两个字段**、新增 `insecure_hosts`。

| 字段 | JSON 形状 | 约束 | 生效 |
|---|---|---|---|
| `name` | `string` | 见下（唯一标识，也是持久化主键） | 停机 |
| `listen` | `{host, port}` | `host` 是 IP 字面量（默认 `127.0.0.1`，可为 `0.0.0.0` 对外服务）；`port` 见下 | 停机 |
| `rules` | `string \| null` | 规则名（**不是路径**，见本文件「规则库账本与固定名软链」） | **热**（换软链） |
| `insecure_hosts` | `string[]` | 完整域名（也接受 IP 字面量）；拒绝 `*` / `?`；≤200 条；归一化 + 去重 + 排序；默认 `[]` | **热** |
| `description` | `string` | 可空；最长 200 字符 | 热 |
| `proxy_user` | `string \| null` | 与 `proxy_password` **同生共死**；非空时不含 `:`、空白或控制字符；≤64 字符 | 停机 |
| `proxy_password` | `string \| null` | 同上；≤128 字符；**任何错误信息不回显取值** | 停机 |

1. **`name`**：先 `trim`，再整体小写化，然后必须匹配 `^[a-z][a-z0-9_-]{0,31}$`。
   归一化后仍不合规即失败，`field = environment.name`。
2. **`listen.host`**：必须是**裸 IP 字面量**（IPv4 或 IPv6，IPv6 可带方括号）。
   拒绝主机名、拒绝带端口的写法（`127.0.0.1:8080` 不是 host）。
   省略时取默认 `127.0.0.1`。失败 `field = environment.listen.host`。
   - 为什么不接受主机名：监听地址是环境的**外部身份**，让"解析成哪个地址"依赖 DNS
     会让工作台显示的端口与客户端实际能连的地址分叉。
3. **`listen.port`**：整数，`1..=65535`（`0` 与越界都失败，`field = environment.listen.port`）。
   是否落在 `port_range` 内**不属于本能力** —— 那是 `port.allocate` 的裁决
   （已持久化的端口在区间缩小后要保留，见下）。
4. **`rules`**：`null` 表示不覆盖任何域名；非空时是**规则名**，必须匹配
   `^[a-z][a-z0-9_-]{0,31}$`（沿用 v1 的 `RULES_NAME_RE`）。
   失败 `field = environment.rules`。
   - **线上形状是名字、不是路径**。Rust 侧可以用 `RuleRef { file }` 之类的类型承载，
     但 JSON 出入口只接受名字 —— 路径由管理器拼（见下条）。
   - **安全理由**：规则名白名单是路径穿越的唯一防线。适配器/管理器**必须**用
     "白名单名字 + 固定后缀 + 固定父目录"拼路径（`<rules_dir>/<name>.rules`），
     **禁止**接受调用方给出的路径片段、`..`、绝对路径或带分隔符的名字。
5. **`description`**：可空字符串；`trim` 后按字符计最长 200，超长失败
   `field = environment.description`。
6. **`insecure_hosts`**：按域名放宽上游证书校验的完整域名清单，语义见本文件
   「insecure_hosts（按域名放宽上游证书校验）」。写入时约束：
   - 必须是**数组**（不接受单个字符串的"方便写法"）；缺省/`null` = `[]`；
   - 每条 `trim` → 小写 → 去掉尾部根点，然后必须通过 host 校验（label 白名单、总长 ≤253）；
     **含 `*` 或 `?` 一律 `invalid_config`** —— 这里**不**做规则文件那套"剥掉 `*.` 前缀"
     的处理（那会让"没生效"变成"生效范围比你以为的大"）；
   - 条数 ≤200；失败 `field = environment.insecure_hosts`（不是数组 / 超限）或
     `environment.insecure_hosts.<index>`（某一项非法 / 值不是字符串）；
   - 持久化前**去重 + 按字典序排序**：同一份输入永远得到同一份列表，diff 才稳定。
7. **`proxy_user` / `proxy_password`**：两个字段，**同生共死**。
   - 都 `null`/缺省 = 不启用代理鉴权；**只有一边** → `invalid_config`，
     `field` 指向**缺失**的那一边（`environment.proxy_password` / `environment.proxy_user`）；
   - 每段非空，且不含 `:`、空白与控制字符。`:` 的禁用与 mitmproxy `proxyauth` 的
     `split(":")` 切分有关：那里要求**恰好一个**冒号，含 `:` 的段会让两侧语义分叉；
   - 长度上限：`proxy_user` ≤64、`proxy_password` ≤128（按字符计）；
   - **任何错误信息都不回显取值**；视图 / 日志 / SSE 只暴露 `proxy_auth_enabled` 布尔；
   - 明文**只**落在两处：0600 的状态存储文件，以及实例的启动参数
     （`--set proxyauth=<user>:<password>`，由 core 拼装）。记录进程身份时
     cmdline 里的凭据值必须先脱敏成 `***`，比对存活进程时两侧都脱敏后再比。
   - 运行中实例改它必须先停止（实例只在启动时读取凭据）。
8. **未知字段必须失败**，禁止静默忽略（`field` 指出该字段的点分路径）。
   v1 的 `dns_servers`、`hosts`、`color`、`domain_suffix` 会响亮报错，
   而不是被当成"没用的字段"默默收下（有 fixture 钉住这一点）。
9. **两条迁移 shim**（只读不写，读到旧形状就转换，写出永不含旧字段）：
   - `options`：`{}` 或 `null` **接受并忽略**（旧版每个环境都写 `options: {}`，
     一律拒绝会让升级卡死）；**非空** → `invalid_config`，`field = environment.options`，
     报错信息要指出唯一可行的替代（把受影响的域名列进 `insecure_hosts`）——
     非空就意味着这份配置**确实依赖**过那条通道，静默忽略会让用户以为设置还在生效；
   - `proxy_auth`（旧的一整串 `user:password`）：读时**无损拆分**成两个新字段
     （整体 `trim` 后按第一个 `:` 切开），写时不再输出。与两个新字段**同时给出**
     （都非空）则 `invalid_config`，`field = environment.proxy_auth`——那是真的歧义，
     不该猜。PATCH 语义下旧形状是"整体替换凭据"，所以 base 上的旧值先被清掉再迁移。

### 归一化与默认值：只有一个来源

默认值（`listen.host = 127.0.0.1`、`rules = null`、`insecure_hosts = []`、`description = ""`、
`proxy_user = null`、`proxy_password = null`）在 core 里定义**一次**。宿主配置只做映射，
适配器**禁止**再写一份默认值。
`environment.validate` 的输出即"归一化后的完整记录"，可直接持久化。

### 编辑已有环境（PATCH 语义）

环境是**可改的**，不必删了重建。请求体是"要改的字段"的集合，没出现的字段一律不动
（`environment.merge` 的 golden case 钉住了这条）；可改字段就是上表那 7 个，未知字段照样失败。

`rules: null` 与"没给 `rules`"是**两件事**：前者是"解绑，回到不覆盖"，后者是"别动绑定"。
`insecure_hosts: null` 与"没给"同理（前者 = 清空回 `[]`，回到全部严格校验）。

`insecure_hosts` 与 `listen` 同款：**整体替换**，不是元素级合并 —— 列表的"部分修改"
没有无歧义的语义，所以给出来的数组就是新值。凭据两个字段是逐字段的（合并后**重新校验**
"同生共死"）。

哪些改动**要求环境处于停止状态**，由"运行中的实例会不会因此与配置分叉"裁决：

| 改动 | 运行中 | 理由 |
|---|---|---|
| `description` | ✅ 允许（热） | 纯展示字段，实例不读它 |
| `insecure_hosts` | ✅ 允许（热） | 注入器轮询 `config.json`，一变就整份重读（见下节） |
| `rules` **绑定** | ✅ 允许（热） | 绑定由**固定名软链**承载：管理器原子换链 + 重写 `config.json`，注入器下一次轮询跟上；实例不必重启 |
| 规则文件的**内容** | ✅ 允许（热） | 注入器按目标文件的 `(mtime, size)` 重读；绑定没变也不必重启 |
| `name` / `listen` | ❌ 拒绝（`conflict`） | 它们是环境的身份：客户端代理配置、状态文件、进程记录会同时失效 |
| `proxy_user` / `proxy_password` | ❌ 拒绝（`conflict`） | 实例在启动时经 `--set proxyauth=…` 拿到凭据，运行中换不上 |

拒绝时的 `message` **必须**说清"先停止"，并列出哪些字段是热的，否则调用方只能猜。

改名是一次**搬迁**，不是"删一个建一个"：期望状态、端口归属（是否自动分配）、异常标记、
进程记录都跟着搬到新名字下，旧名字在账本里消失。改名**不搬日志文件** —— 日志按名字落盘，
新名字从新文件开始（见 `instance.logs`）。

改了 `listen`、`rules` 绑定、`insecure_hosts` 或凭据即作废**旧的失败标记**
（`port_conflict` / `config_mismatch`）并**重置收敛基准**：那两个标记陈述的是
"上一个配置失败了"，留着它会让界面拿**新**配置报旧冲突（`config_mismatch` 尤其可能
正是由某个凭据或放行清单引起的）；收敛基准不重置的话，新配置会继承旧窗口，
刚写完就被判成 `config_mismatch`。

## insecure_hosts（按域名放宽上游证书校验）

**问题**：规则把某域名改写到内网测试机，而那台机器的证书由私有 CA 签发（中间证书也不下发），
上游链路校验必然失败。这类"只是 issuer 不受信"的场景需要一个**按域名**的例外，
而不是全局关掉校验。

**定义**：`insecure_hosts` 是完整域名（或 IP 字面量）集合；被放行 ⟺ 归一化后的 SNI
与集合里某个元素**完全相等**。无通配符、无子域继承、无后缀匹配。

命中判定（`insecure.hosts`）：

| 清单 | 宿主 | 结果 | 钉住的语义 |
|---|---|---|---|
| `["365.kdocs.cn"]` | `365.kdocs.cn` | ✅ | 精确命中 |
| `["365.kdocs.cn"]` | `365.KDocs.CN.` | ✅ | 两侧都归一化 |
| `["kdocs.cn"]` | `365.kdocs.cn` | ❌ | 不做子域继承 |
| `["kdocs.cn"]` | `kdocs.cn.evil` | ❌ | 不做后缀匹配 |
| `[]` | 任意 | ❌ | 空 = 不放行 |
| `["10.13.34.11"]` | 无 SNI，上连地址 = 该 IP | ✅ | 无 SNI 时才退回地址 |
| `["10.13.34.11"]` | SNI = `365.kdocs.cn`，地址 = 该 IP | ❌ | SNI 存在就不退回地址 |

- **SNI 缺失才退回上连地址**：规则改写会让"请求的域名"与"连的地址"分叉，客户端不发 SNI
  时只能按地址判。SNI 存在但不在清单里**不**退回 —— 退回会让一次命名失配变成一次静默放行。
- **命中后做的是"只放宽校验"**：自建上游 TLS context 时把 `verify` 设为 `VERIFY_NONE`，
  其余（ALPN、cipher、TLS 版本、ECDH 曲线、SNI 设置）与不改写时逐项一致。
  名单外的域名一律严格校验，行为与今天完全相同。
- **只在 `tls_start_server` 这一步生效**：`-s` 脚本的这个钩子**先于** core 自己的
  tlsconfig 执行，所以注入器提供了 `ssl_conn` 之后 core 会直接返回；
  `server.sni` 必须**显式**设置，否则内网测试机按 SNI 选不到 vhost。
- **禁止静默扩大范围**：写入时拒绝通配符（见「领域不变量」第 6 条）；匹配是纯函数；
  清单为空就是全部严格校验。**没有"整个环境全关"的开关**（`ssl_insecure` 这类全局选项
  随 `options` 一起删除）。
- 判定失败时**倾向严格**：注入器构造 context 出错只记一条 warn，不设置 `ssl_conn`，
  于是 core 走它自己的严格路径 —— 宁可连不上，也不能在出错时把校验静默关掉。

## 配置下发与热应用（内存装配，v3）

引擎在管理器的**同一个进程**里：配置不再是"写给另一个进程去轮询的文件"，而是
一次同步装配。

- **装配（compile）**：归一化后的环境字段 + 规则正文 → `EngineSpec` →
  `envboard-core` 编译：凭据成对校验、规则文本解析成查找表、插件链装配与注册表
  校验（见「v3 插件与能力注册表」）、config_hash 计算。**任何一步失败即
  `invalid_config`（附字段路径），整套拒绝**。
- **生效（apply）**：`ProxyEngine::apply` 把新快照原子换入（ArcSwap），返回新的
  `config_hash`；**装配即生效，没有轮询间隔、没有收敛窗口**。运行中的请求按
  进入时取到的旧快照跑完（每请求 `load_full`，单请求内配置一致）。
- **失败保留旧快照**：apply 失败时旧快照继续服务，管理器记 `invalid_config`
  标记 —— 健康视图以 `unhealthy` + "configuration rejected, previous snapshot
  still serving" 的原因文本呈现。这是 v2 `config_error` 语义的换载体延续：
  **允许降级，禁止静默**。
- **回执**：`EngineReport.config_hash`（生效中的 spec 哈希）与 `epoch`
  （单调装配代次）。"账本里那份配置有没有生效"= 比较回执与按当前账本算出的
  期望哈希；v2 靠状态文件回显 + 规则条数 + `options_echo` 的三层比对**整体删除**
  （同步装配不存在"宿主静默忽略一个选项"的介质）。
- **热/停机字段**（PATCH 面，与「编辑已有环境」一节共同生效）：
  - 热：`description`、`insecure_hosts`、`rules` 绑定、规则**内容**（import 同名
    覆盖即对绑定环境热应用）；
  - 停机：`name`、`listen`、`proxy_user`、`proxy_password`。停机字段在实例运行时
    被修改 → `conflict`；热字段 apply 失败 → 按 `invalid_config` 标记，
    不得伪造成运行错误。

## 规则库账本（`rules.ledger`）

**账本是唯一真相**：`state.json` 里的 `rules[]` 记录每条规则的
`{name, source, rendered, entries, skipped, conflicts, imported_at}`，
其中 `rendered` 是**规范化渲染后的完整正文**。
`<rules_dir>/<name>.rules` 只是一份可再生的物化产物（v3 起**没有任何运行时消费者**：
引擎的规则输入是账本 rendered 经 `EngineSpec` 直供；物化文件保留给人看与兼容）。

- **导入**：解析 → 渲染 → 更新账本 → 写物化文件（0600）→ 保存账本 →
  **对绑定该规则且在跑的环境逐个热应用**（内容热生效 = 一次 apply，v2 靠轮询文件）。
- **对账（启动与常驻循环各一次，幂等）**：
  1. 物化目录里有、账本里没有的 `*.rules` → **回填**账本（读文件重新 parse 得统计）
     —— 升级迁移，旧规则库不丢；
  2. 账本里有、物化文件缺失或被改坏 → 按 `rendered` 逐字节重建。
  （v2 的"每环境固定名软链对齐"随注入器退场删除。）
- **读取**：清单与正文都从账本读，物化文件被删也答得出来；视图的"期望条数"同样
  来自账本 —— "期望什么"不依赖物化文件的瞬时状态。
- **删除**：仍拒绝删除"被任何环境绑定"的规则；删除 = 账本移除 + 物化文件删除。
- **绑定不存在**的规则名在写入时就被拒绝（`field = environment.rules`）：那会变成
  一次静默失效，宁可不接受。v3 里 rendered 直供引擎，**没有"链断了所以不覆盖"的
  形态**；`rules_missing` 字段的含义相应改为"**账本里没有该名字的规则**"。

## 端口分配（`port.allocate`）

端口是环境的身份，分配后**不再变**。契约只裁"选择规则"，不裁随机性 —— 候选序列的
生成（在 `port_range` 内随机打乱）由管理器负责，随机源不进契约，golden 才能确定。

输入：候选序列 `candidates`、已被占用集合 `taken`（含管理器账本与试绑失败的结果）、
可选的显式 `requested`。输出：选中的端口，或错误码。

1. **无显式指定**：按 `candidates` 顺序取**第一个**不在 `taken` 里的端口。
   已经分配过的环境重新启动时，直接用它的持久化端口，**不参与选择**。
2. **有显式指定**（`--port`）：该端口在 `taken` 里 → 失败
   （`invalid_config`，`field = environment.listen.port`），**绝不静默改**；
   不在 `taken` 里 → 采用（是否需要位于 `port_range` 内：显式指定可以越界，
   因为它的用途就是人工固定）。
3. **重试上限**：连续 N 次（默认 32）候选都不可用 → `port_range_exhausted`（**响亮失败**），
   错误信息要给出区间、尝试次数，并提示可用 `--port` 指定。
4. **区间缩小不迁移**：已持久化的端口即使落在新的 `port_range` 之外也**保留原端口**，
   只产出一条告警（`out_of_range`），不重新分配 —— 端口变了等于让客户端配置失效。
5. **试绑的判定语义也是契约的一部分**（"空闲"的定义）：
   - 绑定地址必须与实例的 `listen.host` **一致**；
   - **必须关闭 `SO_REUSEADDR`**。`tokio` 的 `bind` 在 Unix 上默认打开它，
     会让某些已被占用的端口绑定成功而被误判为空闲；
   - 试绑成功即视为空闲，**立刻释放**（不持有到实例启动）。
6. **试绑与真正监听之间存在 TOCTOU 窗口**，无法消除，因此：
   - **新建**环境时实例启动失败且判定为端口冲突 → 自动换一个端口**重试一次**，
     再失败才报错；
   - **已持久化**端口冲突时**禁止静默重分配** → 环境标记 `port_conflict`，
     由工作台显式提示"释放该端口或点『重新分配端口』"，重分配是**显式动作**，
     并提示用户同步更新客户端配置。

## 实例生命周期（`instance.reconcile`，v3）

- 持久化的是**期望状态** `desired: running | stopped`；**实际**状态 = 引擎实例
  是否在跑（内存报告：`starting / running` 即在跑）。v2 的进程身份判据
  （PID + starttime + cmdline 三重比对、僵尸回收、PID 复用告警）随**子进程模型**
  一起退场 —— 实例不出进程边界，不存在"跨进程身份错认"的介质。
- reconcile（启动时一次 + 常驻循环）顺序**固定**：**先清理，后启动**。
  `desired=running` 但实例不在跑（含崩溃后）→ 重新 `start` —— 这就是崩溃自愈；
  重启节奏由管理循环的间隔约束，不在决策函数里做退避。
- **端口 = 绑定即真相**：启动流程等待定态（`running / port_conflict / failed`）。
  已持久化端口被占 → 标记 `port_conflict`，**绝不自动换端口**；自动换一次只允许
  发生在**新建环境的启动路径**里，且仅一次（见「端口分配」）。
- **健康判据（v3 全部来自内存）**：
  1. **标记优先**：`port_conflict`、`invalid_config`（"配置被拒，旧快照在服务"）
     这类上一次动作留下的事实先于推导；
  2. `desired != running` → `stopped`；
  3. 引擎内存报告：`starting / running / unhealthy / port_conflict / failed`。
  - **视图层与权威判定同源**：同一个 verdict 函数（v2 里"列表只要句柄表有就算
    running"的病根在结构上不可再发生 —— 没有第二套判据）。
  - **崩溃隔离是任务级**：请求任务 panic 只死那一条连接；引擎线程 panic →
    `failed`（带原因）→ reconcile 按 desired 重启。release 构建**必须保持
    unwind**（`panic = "abort"` 会让任务级隔离失效）—— 这条是构建契约。
  - 引擎线程终止后**不会留活口**：实例随线程消失，不存在 v2 的"孤儿清理"面；
    升级说明负责一次性清掉 v2 遗留的 mitmdump 进程。

## 实例日志（`instance.logs`）

代理核心是**外部进程**，它的 stdout/stderr 归管理器管。这里有一条硬约束：

- **绝不能让子进程阻塞在写日志上**。管道容量是 64 KiB（本机实测 `F_GETPIPE_SZ`），
  而 mitmproxy 默认详细度约 307 字节/请求 → **约 213 个请求就能写满**；写满之后子进程会
  阻塞在自己的事件循环里，**所有客户端一起挂住**（不是"日志丢了"，而是代理停止服务）。
  实测：不读管道时第 212 个请求开始失败，读走 65116 字节后下一个请求立刻恢复 200。
- **默认形态是文件直写**：子进程的 stdout/stderr 直接接 `<state_dir>/logs/<env>.log`
  （`log_dir`，可用 `--log-dir` 改，`--no-log-file` 关掉）。内核负责写盘，我们进程
  **不在链路上**，所以上面那条路径从设计上不存在。
  - 每次启动写一行运行标记（`--- envboard: env=… listen=… started=… ---`），追加而非截断：
    跨重启的历史要留着，标记负责分段；
  - `PYTHONUNBUFFERED=1` 在两种形态下都必须设：Python 对**非 TTY 的管道与文件都是块缓冲**，
    不设这个变量，日志会攒到几 KB 才吐一次，工作台长期读到空；
  - `log_dir = None`（`--no-log-file`）时退回"管道 + 内存有界环形缓冲"。这条路径
    **必须**持续把管道读走，且要防三件事：单行长度无上限会吃光内存；写盘是阻塞调用，
    不得压在 async worker 上；锁中毒不得 `unwrap()` —— 一个读线程 panic 会连带把另一个
    也弄死，管道就再没人读了。
- **体积必须封顶**：`max_log_bytes`（默认 8 MiB；`0` = 不轮转；下限 64 KiB，低于它等于
  静默丢日志）。轮转**只能是 copytruncate**：把尾部搬去 `<env>.log.1`，再把原文件截断为 0。
  - **禁止用 rename 轮转**：子进程还持着这个 inode 的 fd 在写，改名之后它会继续写那个
    已经被移走的文件，新文件永远是空的。
- **读尾部必须是有界的**：只从文件末尾读一个窗口（256 KiB）再切行，并允许跨 `.1`
  往前拼；**禁止**整个文件 `read_to_string`（日志一大就会把管理器拖住）。
- 实例崩溃或被停止之后日志**仍然可读**：崩溃现场正是最需要日志的时刻。
- 日志文件按**环境名**命名，所以改名会让新名字从新文件开始写（旧文件留在原处，
  历史不丢、也不合并）—— 改名是"搬迁环境"，不是"搬迁它的输出"。
- 检查/轮转的触发点：实例启动前一次，加上常驻循环（工作台 30 秒、`run` 每次刷新）
  一次；都是"每环境一次 `stat`"，只有超上限才真正动手。

## v3 插件与能力注册表（envboard-core）

引擎三层能力模型：**内核能力**（原生快路径）/ **内置插件** / **扩展插件**。
注册表是静态只读清单（core/rs/crates/envboard-core/src/plugin.rs 的
CAPABILITIES）：只参与装配期校验，不参与请求路径查找；热路径走装配后的
扁平链，避免与 ArcSwap 快照形成第二真相。内核条目登记在此是为了可见性
（能力清单有唯一出处），它们的实现必须保持原生 —— 插件链里出现内核
id 是装配错误。

| id | layer | phase | depends_on | 语义与载体 |
|---|---|---|---|---|
| kernel:listen | kernel | startup | — | listen.host/port；绑定即真相，EADDRINUSE → port_conflict（不换端口、不试绑） |
| kernel:proxy-auth | kernel | startup | — | proxy_user/proxy_password 的 407 门；凭据只在内存，定长时间比对 |
| kernel:tls-policy | kernel | connect | — | insecure_hosts → ConnectTarget.tls_policy 两档；无全局关校验 |
| kernel:mitm-ca | kernel | startup | — | confdir 共享 CA 的加载/物化（已装客户端零感知判据见代码与 ProxyCore 矩阵） |
| kernel:protocol | kernel | request | — | HTTP/1.1 协议面与引擎侧超时常量；101 透传 |
| hosts-rules | builtin | connect | — | 消费环境 rules 字段（hosts 文本）；只改 resolved_addr |
| request-log | builtin | log | — | 默认启用；终局记录写日志通道；on_error = bypass |
| debug-inject | extension | request | — | 测试/故障注入旋钮（对齐 envboard-core-fake 先例）：行为由持有者指定，产品装配路径恒为空 |

**装配与错误契约**

- 插件执行序 = 配置声明序；**依赖约束 > 声明序**（同层同依赖内保持声明序）。
- 装配期校验：id 未注册、内核冒充插件、缺依赖、依赖环 → invalid_config，
  消息点名双方。**没有**"静默等待依赖"的语义 —— 缺依赖必须当场可见。
- 错误档位：connect/改写钩子 Err → fail-closed 502（带插件名）；显式声明
  bypass 的插件跳过并强制 WARN + 逐插件计数；每钩子超时（connect/head 1s、
  body 5s）按 Err 处理；on_log 错误永不外溢（类型即契约：同步、返回 unit）。
- 热更新：配置编译通过 → ArcSwap 原子换入新的插件集快照；任一插件构建/校验
  失败 → 整套拒绝（invalid_config），旧快照继续服务。
- 门禁：注册表 ↔ 内置插件实现 ↔ 本节表格一一对应，由 policy 层的注册表
  门禁判定（core/rs/crates/envboard-policy-tests/tests/registry.rs）。

## 引擎能力矩阵（v3；换实现时要重新满足的清单）

v3 只有一种 core（进程内纯库引擎，`envboard-core`）。矩阵保留的意义：**任何未来
替换（含插件化内核的再分层）必须逐项重新满足这张表**，否则契约不许落地。

| 能力 | 契约要求 | v3 实现位置 |
|---|---|---|
| 监听 | bind listen.host:port；EADDRINUSE → `port_conflict`（不试绑、不换端口） | 引擎线程内 `TcpListener::bind`，绑定即 `running` |
| 代理鉴权 | Basic 407；CONNECT 与 absolute-URI 同一条门；凭据不进 argv/状态文件/视图 | 内核门（`kernel:proxy-auth`），内存定长时间比对 |
| TLS 中间人 | 加载/兼容既有 confdir CA；按 SNI 现签叶子证书；已装客户端零感知 | `ca::SharedCa`（`kernel:mitm-ca`）+ `tls::SniResolver` |
| 上游证书策略 | `insecure_hosts` 精确匹配语义不变：SNI 优先、无 SNI 回退上连地址、SNI 存在未命中不回退；无全局关校验 | `ConnectTarget.tls_policy` + 连接执行器两档位（`kernel:tls-policy`） |
| 改写 | hosts 规则只改连接目标：不动请求内容、不动 Host 头、不动 SNI 基准 | **内置插件 hosts-rules**（`on_connect` 修订 `resolved_addr`） |
| 日志 | 文件直写 `<log_dir>/<env>.log`、copytruncate 轮转、有界尾读；**数据面永不阻塞在写日志上** | request-log 内置插件 + 有界总线（丢弃计数入 `EngineReport.log_drops`） |
| 协议面 | HTTP/1.1（CONNECT 隧道 + absolute-URI）；ALPN 只协商 h1；101 透传 | `kernel:protocol`；强制 h2 的客户端失败 = 已登记的已知限制，非静默降级 |
| 配置生效 | 装配即生效；失败整套拒绝 + 旧快照继续 + 标记可见 | `ProxyEngine::apply`（ArcSwap） |

**已知限制（必须随 README 发布）**：v1 引擎只实现 HTTP/1.1 —— 客户端 ALPN 只协商
`http/1.1`，浏览器无感，强制 h2 的客户端（gRPC 等）会失败；信任库来源从 certifi
换成系统库（rustls-native-certs），"哪些域名无需进名单就能通过"的集合随之变化。

## 规则文件（rules）语义

**规则文件是"已规范化"的静态覆盖**，由一份手写的 hosts 风格输入生成
（账本与物化文件的关系见本文件「规则库账本与固定名软链」）：

```
输入（容忍）                         输出（确定性）
─────────────────────────────       ─────────────────────────────
10.0.0.1 a.example.com b.example.com 10.0.0.1 a.example.com b.example.com
b.example.com 10.0.0.2        →      10.0.0.2 b.example.com
# 注释 / 空行 / 非法行                每个 ip 一行，ip 与 host 均排序
```

完整的输入语法、容忍规则、跳过原因码与渲染格式见 **`rules.md`（BNF）** —— 
那份文件是 Rust 实现与 Python 注入器的**共同仲裁**，两份实现都必须对着它写。

核心不变量（细节与形式化描述在 `rules.md`）：

1. **非法内容永远不会让整次导入失败** —— hosts 文件是人手写的，一个笔误不该让
   其余几百条一起报废。但每一处忽略都**必须**如实记录（原因码 + 原文），
   不允许静默吞掉。
2. **冲突：后出现者胜**，并记入 `conflicts`（同 host 同 ip 不算冲突）。
3. **输出确定性**：同一份 `entries` + `source` 渲染出的字节必须完全相同。
   `parse(render(x)) == parse(x)`，重复导入同一份文件不产生 diff。
4. **生效语义**：规则在**建连时**改写上连目标地址（不改请求内容、不改 `Host`、
   不改 SNI 基准），因此只对**请求里给出了域名**的上连生效；客户端直连 IP 不匹配规则。
5. **作用域**：规则只对**流量指向了该环境端口**的请求生效。v1 那套改系统 DNS、
   全机生效的行为**不再是本产品的语义**。

## 多语言实现的仲裁规则

`rules.parse` 有**两份刻意的实现**（Rust 用于导入/规范化与持久化，Python 注入器用于
运行时改写 —— 注入器要在没有管理器时也能工作，且热重载不该依赖 IPC）。
重复的代价由两条纪律兜住：

1. 同一批 `fixtures/rules/*.json`，两份实现都要跑，**输出逐字节一致**；
   渲染结果（规范化文本）也要逐字节一致，不只是"entries 相等"。
2. 语法分歧一律回到 `rules.md` 的 BNF 裁决，**不得**以某一侧的实现为准。

## 相对 v1 删除的能力与字段（breaking）

| 删除项 | v1 用途 | 删除理由 |
|---|---|---|
| `Environment.options`（透传任意 core 选项） | 临时调参、`ssl_insecure` 这类全局开关 | 它是一条绕过契约的任意通道：安全控制可以不经一等字段被打开；且"配置说开了、实例没收到"这类分叉无法从契约上排除。替代：受影响的域名进 `insecure_hosts`，其它 core 选项**没有**替代 |
| `Environment.proxy_auth`（一整串 `user:password`） | 代理访问鉴权 | 拆成 `proxy_user` + `proxy_password`：字段级校验与字段级错误路径，且落库后仍只以布尔出现在视图/日志里 |
| `Environment.hosts` | 环境内联静态覆盖 | 与规则文件功能重叠，两套真相 |
| `Environment.dns_servers` | 指定该环境用哪台 DNS | v2 不再有解析层编排（差异由规则直接给出结果） |
| `Environment.domain_suffix` | 展示辅助 | 从未参与解析，纯装饰 |
| `Environment.color` / `labels` | UI 装饰 | 归工作台，不进领域模型 |
| `mapping.*` 观测索引 | "某条流量属于哪个环境" | 环境 = 实例，归属由端口天然确定 |
| `resolver.*` 解析编排 | 按环境选 DNS、TTL、来源优先级 | 同 `dns_servers` |
| `activate`（全局当前环境） | 切换"如何解读流量" | v2 **没有"当前环境"这个全局状态**，换端口即换环境 |
| `/api/resolve` 的 `all_envs` | 一次请求对比所有环境的解析 | 转为静态对比视图：规则是确定性的，"某域名在各环境被覆盖成什么"查表即得，不需要发请求 |
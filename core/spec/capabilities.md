# envboard 能力契约

语言中立契约（AGENTS.md §3/§4）。实现可以换语言、换宿主，但这里的语义与
`fixtures/` 的 golden case 是**裁定依据**。

## 能力清单

| 能力 id | 契约 | fixture 目录 |
|---|---|---|
| `environment.validate` | 环境定义合法性；归一化规则；错误码与 `field` 路径 | `fixtures/environment/` |
| `environment.merge` | patch 语义（部分更新，未提及字段保持不变，重新校验） | `fixtures/merge/` |
| `rules.parse` | hosts 风格输入 → 规范化规则；非法内容忽略；冲突后出现者胜 | `fixtures/rules/` |

> MVP 刻意只把**纯逻辑**纳入契约。DNS 查询与 Dashboard 的 REST 语义依赖网络与
> 宿主，不适合做成确定性 golden case，故不入契约；其行为由 `README.md` 的
> 支持矩阵与 `docs/acceptance/` 的实机证据覆盖。

## 依赖图

```
environment.validate  ←  environment.merge
                      ←  registry.*        （CRUD / 激活切换 / rules_file 绑定）
                      ←  resolver.*        （解析编排：env.hosts > rules > DNS）
                      ←  adapter.rules     （导入 / 渲染 / 落盘）
                      ←  adapter.webapi.*  （REST 面）
```

`environment.validate` 是根能力。缺依赖必须以 `invalid_config` 失败，**禁止静默降级**。

## 领域不变量

1. **环境名**：`^[a-z][a-z0-9_-]{0,31}$`。输入会被 trim + 小写化；归一化后仍不合规即失败。
2. **`dns_servers`**：每项必须是**裸 IP 字面量**。
   - 拒绝 `10.0.0.53:53`（`mitmproxy_rs.dns.DnsResolver` 只接受裸 IP）
   - 拒绝主机名
   - 拒绝重复项
   - **空列表 = 跟随操作系统 DNS**（显式语义，不是"未配置"）
3. **`hosts`**：键为域名（允许 `*.` 前缀与尾部根点，归一化时剥离），值为裸 IP。
   静态覆盖的优先级高于任何 DNS 结果，且**永不过期**。
4. **`domain_suffix`**：可空；非空时按域名标签规则校验。
5. **`color`**：可空；非空时必须是 `#rgb` 或 `#rrggbb`。
6. **未知字段**：必须失败，禁止静默忽略。

## 映射（Mapping）语义

| 字段 | 语义 |
|---|---|
| `env` | 所属环境 |
| `host` | 归一化后的域名（小写、无根点） |
| `ip` | IP 字面量字符串 |
| `source` | `static` \| `passive` \| `active` |
| `ttl` | 秒；`0` 表示永不过期（仅 `static`） |
| `resolved_at` / `expires_at` / `age` | 时间戳与派生值 |

**来源优先级**（高 → 低）：`static` > `passive` > `active`。

- 高优先级来源**覆盖**低优先级。
- 低优先级来源**既不覆盖也不刷新**高优先级条目的 TTL。
- 同优先级来源会刷新 `resolved_at` / `ttl`。

**只有正向（host → ip）。** 不提供任何 ip → host 的能力：不查 PTR、不建反向索引、
不暴露反查接口。同一域名在多套环境里各自的 IP，靠 `env` 这一维区分 ——
这正是本插件存在的意义。

## 规则文件（rules）语义

**规则文件是"已规范化"的静态覆盖**，由一份手写的 hosts 风格输入生成：

```
输入（容忍）                         输出（确定性）
─────────────────────────────       ─────────────────────────────
10.0.0.1 a.example.com b.example.com 10.0.0.1 a.example.com b.example.com
b.example.com 10.0.0.2        →      10.0.0.2 b.example.com
# 注释 / 空行 / 非法行                每个 ip 一行，ip 与 host 均排序
```

### 输入容忍规则

| 情形 | 行为 |
|---|---|
| 空行、`#` 注释（整行或行尾）、BOM、CRLF、行首缩进 | 忽略，不影响解析 |
| `ip host1 host2 …` | **多个 host 共用同一个 ip** |
| `host1 host2 … ip` | 与上一种**等价**（反序写法同样接受） |
| 一行里既没有合法 IP | 整行记 `no-ip-literal` 忽略 |
| 只有 1 个 token | 记 `too-few-tokens` 忽略 |
| 某 token 是 IP 却出现在 host 位置（如 `1.2.3.4 5.6.7.8`） | 该项记 `invalid-host` 忽略 |
| 某 host 不合法 | **只丢该项**，同一行其余 host 照常保留 |
| 同一 host 映射到不同 ip | **后出现者胜**，并记进 `conflicts` |

**非法内容永远不会让整次导入失败** —— 这是有意的：hosts 文件是人手写的，
一个笔误不该让其余几百条一起报废。但每一处忽略都**必须**如实记录（行号 + 原因 + 原文），
不允许静默吞掉。

### 输出确定性

同一份 `entries` + `source` 渲染出的字节必须完全相同：ip 数值序、host 字典序、
每个 ip 一行。`parse(render(x)) == parse(x)`，且重复导入同一份文件不产生 diff。

### 生效与优先级

规则文件**按环境绑定**（`Environment.rules_file`）—— 切环境即切规则文件。
绑定为空表示该环境不使用规则文件。

```
环境 inline hosts   >   绑定到该环境的规则文件   >   DNS 解析
      └────────────── 两者都属于 static 源：优先于 DNS，且永不过期 ──────────────┘
```

环境里的条目是"这个环境特有的例外"，比共用的规则文件更具体，所以它赢。
解析结果的 `static_from` 字段会标明这条到底来自 `environment` 还是 `rules:<name>`。

**被环境绑定的规则文件禁止删除**（`conflict`）—— 否则那个环境会静默失去覆盖。

## 环境切换（activate）语义

MVP 只实现 **L1 观测层**：

- 切换环境改变的是"如何解读流量"：解析用哪套 DNS、`flow` 注解归属哪个环境。
- 切换环境**不改写任何流量**。请求改写（L2）不在本版本契约内。

`activate` 是幂等操作：切到当前环境不产生 `active` 事件。

## 注解（annotate）语义

给一个 `host`（可选带 flow 实际连到的服务端 `ip`）判定环境归属：

| 情形 | `confirmed` | 选中的环境 |
|---|---|---|
| 某环境解析出的 IP 列表含该 `ip` | `true` | 命中者中优先取激活环境 |
| 无 `ip` 线索，或没有任何环境命中 | `false` | 激活环境（若该 host 在其中），否则第一个已知环境 |
| 该 host 在所有环境都无记录 | — | 返回 `None`，不注解 |

`confirmed=false` 不是错误：CDN 边缘 IP 与 DNS 解析结果不一致是常态，
按 host 标注仍然有价值，但要如实标明"未证实"。

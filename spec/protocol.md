# 界面边界词汇（视图 DTO、快照帧与游标）

envboard 的界面只有一个形态（桌面 GUI），但它消费的词汇**不认识界面**：视图 DTO、
快照帧、游标与错误信封。这些形状的**唯一定义处**是 `crates/protocol`
（`envboard-protocol` crate，纯 serde，零传输依赖）；本文件是它的语言中立契约。
事件信封与两档存储格式是另一份契约，见 spec/events.md。

曾经有一层 HTTP/SSE 绑定把这些词汇送到浏览器，它随 web 形态一起出库
（见 docs/adr-003-retire-web-frontend.md）。**词汇本身一件没动** —— 这正是当初
把词汇拆成叶子 crate 的目的：换界面、换通道，都不该改这份契约。
门面上有哪些方法、吃什么吐什么，见 spec/admin-api.md。

## 错误信封

门面返回的 `Error` 与快照帧里的错误共用一个形状：

```json
{"error": {"code": "not_found", "message": "…", "field": "…"}}
```

- `code`：错误码，契约见 spec/errors.md（曾经的 HTTP 状态映射随传输面出库，
  错误码与它的可重试性判据不变）；
- `field`：可选，指向配置字段的路径（如 `web.token`）；
- 界面只翻译它，不重新判定：退出码与"能不能重试"都来自 `code` 的
  `retryability`。

## 视图 DTO

服务端构造，界面只读。凭据**永不回显**（代理只回显 `has_auth` 布尔）。

**EnvView**（`AdminService::list` / `get` / `create` / `update` / `snapshot` 的元素）：

```json
{"name": "dev", "listen": {"host": "127.0.0.1", "port": 9000},
 "rules": "stage", "upstream": null, "insecure_hosts": ["api.example.com"],
 "description": "…", "desired": "running", "health": "running",
 "health_reason": null, "rules_count": 3, "rules_missing": false,
 "proxy_command": "export https_proxy=http://…", "proxy_auth_enabled": false,
 "capture": false}
```

`desired` ∈ `running | stopped`；`health` 是健康字面量
（starting / running / failed / stopped / port_conflict / unhealthy），
`health_reason` 是它的原因短语（健康态为 null）；`rules_missing` = true 表示
绑定的规则名在账本里不存在（此时 `rules_count` 恒为 0）。
`proxy_command` 由服务端算好回显，界面不许自己拼。

**ProxyView**（`proxy_list` / `proxy_get` / `proxy_put`）：
`{name, host, port, has_auth, references}`，`references` 列出引用这条代理的环境名
（删除确认与"能不能删"的前置信息）。

**CaptureView / SessionInfo**（`captures`、DebugView 内嵌）：

```json
{"session": {"id": 7, "started_at": 1700000000000, "generation": 2},
 "captured": 128, "dropped": 0, "records": [ … ]}
```

记录形状与易失语义（会话 = 实例生命周期）见 spec/events.md「抓包会话」。

**DebugView**（`debug_view` / `start_debug`）：`{env, capture}`，`capture` 是 CaptureView。

**ReconcileReport**：`{actions: [[env, action]…], warnings: […]}`。

## 快照与增量：帧语义与游标域

这三条语义原来跑在 SSE 上，现在是界面自己的**取数节奏**（实现出处
`envboard-protocol` 的 `TrajectoryWindow / SnapshotFrame / DebugSnapshot /
DebugEvents / ErrorFrame`，形状一条没改）：

| 帧名 | 语义 | 用在哪路数据 |
|---|---|---|
| `snapshot` | 整幅替换（全环境快照 / 调试会话视图） | 快照、调试实时 |
| `baseline` | 尾部窗口整幅替换（首轮取数） | 轨迹 |
| `events` | 增量追加 | 轨迹、调试实时 |
| `error` | 终局错误（`{ok:false, error:{…}}`，之后该路数据关闭） | 轨迹 |

统一规则：

1. **每帧都带显式 `cursor`（裸 u64）** —— 不区分游标种类，每路数据只用自己那个域
   （下表），混用即实现错误；
2. 消费方把最新游标存下来用于续取（轨迹按字节偏移）或展示；
3. 界面对未知字段宽容（fail-open），服务端对未知事件类型 fail-closed ——
   两个方向都是刻意的，见 spec/events.md。

| 路 | 取数方式 | 游标域 | 续传语义 |
|---|---|---|---|
| 快照 | `AdminService::snapshot()`，界面 1s 轮询 | 控制面账本代次（`state.json` 每次 commit +1） | **advisory**：只是快照版本号。健康变化不经过控制面写入，同代次两次内容仍可能不同，界面不得据此跳过渲染 |
| 轨迹 | `trajectory` / `trajectory_since(name, offset)` | `trajectories/<env>.jsonl` 的**字节偏移** | 首轮 cursor = 窗口末端；按 offset 续取；offset 超过文件长度（轮转/截断）→ 从头重发 baseline |
| 调试实时 | `debug_delta(after, limit)` | 抓包记录的 `request_id` | 淘汰只移除游标之前的记录，增量无缺口；换代或目标消失 → 重收一次 snapshot，不续传 |

帧形状明细：

- **TrajectoryWindow**（轨迹的 `baseline`/`events`，也是 `trajectory` 的返回值 ——
  三处同形）：`{cursor, events: [<事件信封>…]}`；事件信封形状见 spec/events.md。
- **SnapshotFrame**：`{ok, cursor, environments: [<EnvView>…], error?}`；
  `ok:false` 时 `environments` 为空、`error` 是错误信封。
- **DebugSnapshot**：`{cursor, env, capture}`；无会话时 `env`/`capture` 为 null、
  cursor 为 0；有会话时与 DebugView 同形（整幅替换），cursor = 会话内最新
  `request_id`。会话换代（实例重启 / clear，即 `session.id` 或 `generation` 变化）
  或目标消失 → 重发 snapshot。
- **DebugEvents**：`{cursor, records: […], captured, dropped}`。

## 安全约定（现在的形态）

进程内直连之后，**没有传输面**，所以那三条为浏览器准备的防线（Host 头校验防 DNS
rebinding、token 档、变更请求带自定义头挡 CSRF）一起出库 —— 它们保护的是"一个能被
外部进程访问的 HTTP 端口"，而现在不存在那个端口。仍然成立的：

- **单实例锁**：同一状态目录只许一个宿主持有（`envboard-host`），拿不到就响亮失败；
- **凭据只进不出**：任何视图 DTO 都不回显用户名与密码，只有布尔；
- **私钥不出组合根**：`CaBundle` 的私钥不进任何交付给界面的结构（见
  crates/host/src/assembly.rs 的 `CaAssets` 注释）。

要对外提供服务，等于要把 HTTP 面带回来 —— 那是 ADR-003「复核触发条件」里的决定，
届时那三条安全约定必须一起回来，不是可选项。

## 兼容性

- 视图与帧**加字段**不构成破坏性变更（消费方对未知字段宽容）；
- 改字段语义、改游标域、删字段是破坏性变更：必须同步本文件与
  `envboard-protocol` 的类型定义，并评估界面的迁移；
- 事件词汇（信封内 `type`/`data`）的兼容规则在 spec/events.md，与本文件独立。

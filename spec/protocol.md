# 前后端通信协议（UI 边界词汇）

envboard 的界面形态不止一种：今天的 web 工作台、将来的桌面 GUI。它们消费的是
同一份词汇 —— 视图 DTO、推送流帧、游标与错误信封。这些形状的**唯一定义处**是
`crates/protocol`（`envboard-protocol` crate，纯 serde，零传输依赖）；本文件是
它的语言中立契约。事件信封与两档存储格式是另一份契约，见 spec/events.md。

传输绑定**不属于**本协议：HTTP/SSE 只是 web 形态（`crates/web`）对它的载体，
桌面形态可以换成任何进程间通道而不动这里的词汇。

## 错误信封

所有 REST 错误响应与推送流错误帧共用一个形状：

```json
{"error": {"code": "not_found", "message": "…", "field": "…"}}
```

- `code`：错误码，契约见 spec/errors.md（含 HTTP 状态映射）；
- `field`：可选，指向配置字段的路径（如 `web.token`）。

## 视图 DTO（REST 响应体）

服务端构造，UI 只读。凭据**永不回显**（代理只回显 `has_auth` 布尔）。

**EnvView**（`GET/POST /api/environments…` 等环境端点）：

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

**ProxyView**（`/api/proxies…`）：`{name, host, port, has_auth, references}`，
`references` 列出引用这条代理的环境名（删除确认的前置信息）。

**CaptureView / SessionInfo**（`GET …/captures?limit=`、DebugView 内嵌）：

```json
{"session": {"id": 7, "started_at": 1700000000000, "generation": 2},
 "captured": 128, "dropped": 0, "records": [ … ]}
```

记录形状与易失语义（会话 = 实例生命周期）见 spec/events.md「抓包会话」。

**DebugView**（`GET /api/debug`、`POST /api/debug`）：`{env, capture}`，
`capture` 是 CaptureView。

**ReconcileReport**：`{actions: [[env, action]…], warnings: […]}`。

## 推送流的帧

三条 SSE 流共用一套**帧语法**，实现出处 `envboard-protocol` 的
`TrajectoryWindow / SnapshotFrame / DebugSnapshot / DebugEvents / ErrorFrame`：

| 帧名 | 语义 | 载体流 |
|---|---|---|
| `snapshot` | 整幅替换（全环境快照 / 调试会话视图） | 快照流、调试实时流 |
| `baseline` | 尾部窗口整幅替换（连接建立时） | 轨迹流 |
| `events` | 增量追加 | 轨迹流、调试实时流 |
| `error` | 流终局错误（`{ok:false, error:{…}}`，之后流关闭） | 轨迹流 |

统一规则：

1. **每帧 data 都带显式 `cursor`（裸 u64）** —— wire 上不区分游标种类，
   每条流只使用自己的游标域（下表），混用即实现错误；
2. 消费方把最新游标存下来，用于断线续传（轨迹流 `?cursor=`）或展示；
3. 前端对未知字段宽容（fail-open），后端对未知事件类型 fail-closed ——
   两个方向都是刻意的，见 spec/events.md。

| 流 | 端点 | 游标域 | 续传语义 |
|---|---|---|---|
| 快照流 | `GET /api/events`（1s 轮推） | 控制面账本代次（`state.json` 每次 commit +1） | **advisory**：只是快照版本号。健康变化不经过控制面写入，同代次两帧内容仍可能不同，UI 不得据此跳过渲染 |
| 轨迹流 | `GET /api/environments/:name/trajectory/stream`（500ms） | `trajectories/<env>.jsonl` 的**字节偏移** | baseline 的 cursor = 窗口末端；重连带 `?cursor=` 从断点续传；offset 超过文件长度（轮转/截断）→ 服务端从头重发 baseline |
| 调试实时流 | `GET /api/debug/stream`（500ms） | 抓包记录的 `request_id` | 淘汰只移除游标之前的记录，增量无缺口；断线重连（EventSource 自动）重收一次 snapshot 即可，不续传 |

帧形状明细：

- **TrajectoryWindow**（轨迹流 `baseline`/`events` 帧，也是
  `GET …/trajectory?limit=` 的响应体 —— 三处同形）：
  `{cursor, events: [<事件信封>…]}`；事件信封形状见 spec/events.md。
- **SnapshotFrame**（快照流）：
  `{ok, cursor, environments: [<EnvView>…], error?}`；`ok:false` 时
  `environments` 为空、`error` 是错误信封。
- **DebugSnapshot**（调试实时流 `snapshot` 帧）：`{cursor, env, capture}`；
  无会话时 `env`/`capture` 为 null、cursor 为 0；有会话时 = DebugView 同形
  （整幅替换），cursor = 会话内最新 `request_id`。会话换代（实例重启 / clear，
  即 `session.id` 或 `generation` 变化）或目标消失 → 重发 snapshot。
- **DebugEvents**（调试实时流 `events` 帧）：
  `{cursor, records: […], captured, dropped}`。

## 安全约定（与协议同提交演进）

- **Host 校验**：任何请求的 Host 头必须等于配置的监听地址（防 DNS rebinding）；
- **token 档**：启用鉴权后除两个静态资产（`/app.css` / `/app.js`，编译期内嵌、
  不含数据）外都要携带 token —— header `x-envboard-token` 优先，SSE 与浏览器
  直开用 `?token=` 等价；
- **变更类请求**（非 GET/HEAD）必须带 `x-envboard-request: 1` 自定义头
  （跨站简单请求带不了自定义头，这一条挡住 CSRF）。

## 兼容性

- 视图与帧**加字段**不构成破坏性变更（消费方对未知字段宽容）；
- 改字段语义、改游标域、删字段是破坏性变更：必须同步本文件与
  `envboard-protocol` 的类型定义，并评估前端与未来形态的迁移；
- 事件词汇（信封内 `type`/`data`）的兼容规则在 spec/events.md，与本文件独立。

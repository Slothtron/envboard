# 事件格式与语义（控制面审计事件 + 数据面请求轨迹）

两份事件日志共用同一个信封与存取契约；实现唯一出处是 `crates/events`
（`envboard-events` crate），消费方是 `envboard-manager`（控制面）与
`envboard-engine`（数据面轨迹）。

## 信封（Envelope）

每行一个 JSON 对象：

```json
{"seq":1,"time":1700000000000,"type":"environment/created",
 "data":{"name":"dev","listen":"127.0.0.1:9000"}}
```

- `seq`：单份日志内**单调连续**，从 1 起，逐条 +1；断档即损坏证据。
- `time`：Unix epoch 毫秒。
- `type` / `data`：由事件枚举的 `tag = "type", content = "data"` 序列化产生。
- `ignorable`：缺省即 false（必需）。显式标 true 的事件允许被不认识它的读者跳过。
  **失败方向是刻意的**：忘标 ignorable 的代价是"过度拒绝"，不是"静默丢事件"。

## 存储格式

- JSONL：首行 `{"version":1}`，随后每行一个信封。
- 格式版本是**单一整数**：只有信封形状、首行或核心事件语义变化才 bump；
  普通新增事件类型不 bump，靠 `ignorable` 覆盖。
- 读者契约（fail-closed）：
  - 首行不是合法 header，或版本高于本实现 → **整份拒绝**；
  - 未知事件类型：未标 `ignorable: true` → **整份拒绝**；标了 → 跳过该行；
  - `seq` 不连续 → **拒绝**（丢失的事件必须可见）；
  - 最后一行非法 JSON（进程崩溃撕裂的残片）→ 容忍丢弃；中间坏行 → **拒绝**。

## 词汇表

### 控制面（`events.jsonl`，权威仍是 `state.json`）

`environment/created | environment/updated | environment/deleted |
rules/imported | rules/deleted | engine/applied | engine/rejected |
instance/started | instance/stopped | instance/reconciled | custom`

- 规则正文**不入事件**：`rules/imported` 只记 `rules_name` + `rules_sha256`
  （可与账本 rendered 比对）。正文随 `<rules_dir>/<name>.rules` 落盘。
- `engine/rejected` 记录的是整套拒绝（invalid_config）——"旧快照继续服务"
  的纪律在 spec/capabilities.md「配置下发与热应用」。
- 保留扩展位 `custom { kind, payload }`：`kind` 必须带域前缀
  （如 `manager/rules-reconciled`），载荷必须无损 JSON。

### 数据面（`trajectories/<env>.jsonl`）

`request/start | request/upstream | request/body | response/head |
request/end | custom`

- `request_id` 由引擎分配，实例生命周期内单调；一条请求的全部事件用同一
  `request_id` 关联，读者只做确定性关联。
- 事件按阶段成对：`request/start` 起、`request/end` 止（`error` 非空 =
  以引擎错误告终，status 恒 502）。
- **正文与头部不入轨迹**：轨迹回答"发生了什么"；内容排障看 `<env>.log`
  与抓包。`request/body` 只记字节数。
- `request/upstream` 只在 hosts 规则命中（`resolved_addr` 被改写）时出现。
- insecure_hosts 放宽命中记 `custom { kind: "tls/insecure" }`。
- 轨迹经**有界总线**落盘（与实例日志同一纪律：写失败丢行 + 计数，
  `EngineReport.trajectory_drops` 可见），数据面永不阻塞在写轨迹上。

## 写入纪律

- **控制面单一发射点**：`Manager::commit` 先 save（`state.json` 权威）
  后 emit（事件）；事件写失败（磁盘满/权限）**不得**让控制面动作失败 ——
  WARN + 丢弃计数（`/api/status` 的 `events_dropped`）。工程门禁钉住
  `repo.save` 只允许出现在 `commit` 内。
- 事件日志按大小轮转（8 MiB，保留一份 `.1`）：写方 open-write-close、
  无长持 fd，rename 轮转安全（与实例日志必须 copytruncate 的约束不同）。
- 新增事件类型：枚举加变体 + 登记进 `KNOWN_CONTROL_KINDS` /
  `KNOWN_DATA_KINDS`（静态注册表，与本文档表格保持一致由代码评审保证）。

## 消费面

- 控制面：`GET /api/history?name=<env>&limit=N`（拉取式只读）；
  工作台「活动」视图。
- 数据面：`GET /api/environments/:name/trajectory?limit=N`（拉取式）；
  `GET /api/environments/:name/trajectory/stream`（SSE：连接即发
  `baseline` 尾部窗口，此后 `events` 增量，断线带 `?cursor=` 续传）；
  工作台环境详情「轨迹」页签。

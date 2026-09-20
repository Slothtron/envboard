# 事件格式与语义（控制面审计事件 + 数据面请求轨迹）

两份事件日志共用同一个**信封**；存取契约则**分两档** —— 控制面是账本，
数据面是窗口。实现唯一出处是 `crates/events`（`envboard-events` crate），
消费方是 `envboard-manager`（控制面账本 + 数据面窗口读）与
`envboard-engine`（数据面轨迹写）。

## 信封（Envelope）

每行一个 JSON 对象：

```json
{"seq":1,"time":1700000000000,"type":"environment/created",
 "data":{"name":"dev","listen":"127.0.0.1:9000"}}
```

- `seq`：控制面账本内**单调连续**，从 1 起，逐条 +1；断档即损坏证据。
  数据面轨迹里它是**会话内编号**：每个实例会话从 1 重启，读者不得校验其连续性
  （见「存储格式」两档）。
- `time`：Unix epoch 毫秒。
- `type` / `data`：由事件枚举的 `tag = "type", content = "data"` 序列化产生。
- `ignorable`：缺省即 false（必需）。显式标 true 的事件允许被不认识它的读者跳过。
  **失败方向是刻意的**：忘标 ignorable 的代价是"过度拒绝"，不是"静默丢事件"。

## 存储格式（两档）

### 控制面 `events.jsonl` —— 账本（读者 `parse_log`）

- JSONL：首行 `{"version":1}`，随后每行一个信封。
- 格式版本是**单一整数**：只有信封形状、首行或核心事件语义变化才 bump；
  普通新增事件类型不 bump，靠 `ignorable` 覆盖。
- 读者契约（fail-closed）：
  - 首行不是合法 header，或版本高于本实现 → **整份拒绝**；
  - 未知事件类型：未标 `ignorable: true` → **整份拒绝**；标了 → 跳过该行；
  - `seq` 不连续 → **拒绝**（丢失的事件必须可见）；
  - 最后一行非法 JSON（进程崩溃撕裂的残片）→ 容忍丢弃；中间坏行 → **拒绝**。

### 数据面 `trajectories/<env>.jsonl` —— 窗口（读者 `parse_window`）

- **无头行**；`seq` 每个实例会话从 1 重启，同一文件里多个会话首尾相接；
  增量读的游标是**字节偏移**（SSE `cursor`），不是 seq。
- 读者契约（窗口）：
  - 首行若是版本头 → 跳过（容忍写方将来补头）；
  - 非法 JSON 行（撕裂残片）→ **逐行跳过** —— 一行残片不得让整份轨迹不可读；
    数据面的丢失可见性由有界总线的 `EngineReport.trajectory_drops` 计数承担；
  - 未知事件类型：未标 `ignorable: true` → **拒绝**（schema 漂移必须响亮，与账本同）；
    标了 → 跳过该行；
  - `seq` **不校验**。
- **为什么分档**：轨迹写方从写下第一行起就不满足账本契约（无头、seq 按会话
  重启）；用账本读者读它必然整份拒绝（BadHeader / SeqGap），表现为轨迹页与
  SSE 流永远不可用。窗口语义是这份文件的既有事实，本节把它成文。

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

## 抓包会话（capture；易失）

- `capture` 是环境第 8 个字段，**默认 false**，PATCH 热开/关。**开关只控记录**：
  关 = 停止记录新请求，已有记录原样保留；重新打开续用同一会话——开关往返不丢记录。
- 会话 = 实例生命周期：记录在内存缓冲（无文件落盘），**停止/重启实例即全部丢弃**。
  丢弃路径只有两个：手动 `POST .../capture/clear`（缓冲清零，会话延续，`generation` +1）
  与实例消亡。对齐 mitmproxy 的内存 View + `view.clear`（它无 id、无生命周期管理），
  这里把 `session.id / started_at / generation` 形式化出来供 UI 明示。
- 记录形状：`{version, session, request_id, time, request{method,path,authority,headers,body},
  response{status,headers,body}, error}`；成对落"盘"（内存条目），无半条记录
  （与 mitmproxy `save.py` 在 response/error 才写整条 flow 同一纪律）。
- **缺失而非截断**（mitmproxy `stream_large_bodies` 的语义等价物）：单侧 body 超
  256 KiB → 只记头与元数据、body 标 `omitted: true`；body 按原始字节存
  （utf8 直存 / 二进制 base64 标注），解码延迟到展示端。
- 缓冲按字节预算淘汰（默认 256 MiB/环境，`--capture-budget` 可调），**满即淘最旧**，
  `captured/dropped` 计数随 API 可见——内存占用恒定，长会话不丢记录的出路是导出。
- 消费：`GET .../captures?limit=`、`GET .../captures/:request_id`、
  `GET .../captures/export?format=har|jsonl`（HAR 1.2，对齐 mitmproxy savehar 形状）。

## 会话双轨：调试会话（live）与导入会话（HAR）

- **调试会话是工作区级单例**：`POST /api/debug {"env": ...}` 开启/切换目标；
  **切换即换代 —— 原目标环境的抓包停止并清空**（capture=false + 清缓冲），新环境开启。
  `POST /api/debug/stop` 停止并清空；`GET /api/debug` 返回当前会话视图。
  环境字段 `capture` 仍是 per-env 开关（API 用户可用），调试页经 debug 端点驱动它。
- **导入会话（HAR）**：`POST /api/har/import`（HAR 1.2）——多个并存、只读、
  进程生命周期内易失；entries 反向映射成抓包记录形状（与调试会话同一渲染组件）。
  有界：最多 8 个会话 / 总量 256 MiB，超限**拒收**（显式动作拒绝比静默淘汰合适）。
  畸形 HAR → `invalid_config` 响亮拒绝。

## 消费面

- 控制面：`GET /api/history?name=<env>&limit=N`（拉取式只读）；
  工作台「活动」视图。
- 数据面：`GET /api/environments/:name/trajectory?limit=N`（拉取式）；
  `GET /api/environments/:name/trajectory/stream`（SSE：连接即发
  `baseline` 尾部窗口，此后 `events` 增量，断线带 `?cursor=` 续传）；
  工作台环境详情「轨迹」页签。

//! envboard 的事件模型 —— **本 crate 文档即事件契约**：控制面审计事件 +
//! 数据面请求轨迹，两份日志共用同一个信封，存取契约分两档（控制面是账本，
//! 数据面是窗口）。消费方：`envboard-manager`（控制面账本 + 数据面窗口读）
//! 与 `envboard-engine`（数据面轨迹写）。
//!
//! ## 信封（Envelope）
//!
//! 每行一个 JSON 对象：
//! `{"seq":1,"time":1700000000000,"type":"environment/created","data":{…}}`
//!
//! - `seq`：控制面账本内**单调连续**，从 1 起逐条 +1，断档即损坏证据；
//!   数据面轨迹里它是**会话内编号**（每个实例会话从 1 重启，读者不得校验连续性）。
//! - `time`：Unix epoch 毫秒。
//! - `type` / `data`：事件枚举 `tag = "type", content = "data"` 序列化产生。
//! - `ignorable`：缺省即 false（必需）。**失败方向是刻意的**：忘标 ignorable
//!   的代价是"过度拒绝"，不是"静默丢事件"。
//!
//! ## 存储格式（两档）
//!
//! **控制面 `events.jsonl` —— 账本**（读者 [`parse_log`]，fail-closed）：
//! 首行 `{"version":1}`，随后每行一个信封。格式版本是单一整数：只有信封形状、
//! 首行或核心事件语义变化才 bump，普通新增事件类型靠 ignorable 覆盖。
//! 首行非法或版本过高 → 整份拒绝；未知类型未标 ignorable → 整份拒绝，标了 →
//! 跳过该行；seq 不连续 → 拒绝（丢失必须可见）；最后一行非法 JSON（崩溃撕裂
//! 残片）→ 容忍丢弃，中间坏行 → 拒绝。
//!
//! **数据面 `trajectories/<env>.jsonl` —— 窗口**（读者 [`parse_window`]）：
//! 无头行；seq 每会话从 1 重启，多会话首尾相接；增量读的游标是**字节偏移**，
//! 不是 seq。首行若是版本头 → 跳过；非法 JSON 行 → **逐行跳过**（一行残片不得
//! 让整份轨迹不可读，丢失可见性由 `EngineReport.trajectory_drops` 计数承担）；
//! 未知类型未标 ignorable → 拒绝（schema 漂移必须响亮）；seq 不校验。
//! 分档的理由：轨迹写方从第一行起就不满足账本契约，用账本读者读它必然整份
//! 拒绝，表现为轨迹页永远不可用。
//!
//! ## 词汇表
//!
//! 控制面（权威仍是 `state.json`）：`environment/created | updated | deleted`、
//! `rules/imported | deleted`、`engine/applied | rejected`、
//! `instance/started | stopped | reconciled`、`custom`。
//! 规则正文**不入事件**：`rules/imported` 只记 rules_name + rules_sha256；
//! `engine/rejected` 记整套拒绝（"旧快照继续服务"的纪律见 engine crate 文档）。
//! `custom { kind, payload }` 的 kind 必须带域前缀（如 `manager/rules-reconciled`），
//! 载荷必须无损 JSON。
//!
//! 数据面：`request/start | upstream | body | response/head | request/end`、`custom`。
//! `request_id` 由引擎分配，实例生命周期内单调，一条请求的全部事件用它关联；
//! 事件按阶段成对（start 起、end 止，error 非空 = 以引擎错误告终，status 恒 502）；
//! **正文与头部不入轨迹**（内容排障看实例日志与抓包，`request/body` 只记字节数）；
//! `request/upstream` 只在 hosts 规则命中时出现；insecure 放宽命中记
//! `custom { kind: "tls/insecure" }`。轨迹经**有界总线**落盘：写失败丢行 + 计数，
//! 数据面永不阻塞在写轨迹上。
//!
//! ## 写入纪律
//!
//! - **控制面单一发射点**：`Manager::commit` 先 save 后 emit；事件写失败不得
//!   让控制面动作失败 —— WARN + 丢弃计数（门面 `events_dropped()`）。
//! - 事件日志按大小轮转（8 MiB，保留一份 `.1`）：写方 open-write-close、
//!   无长持 fd，rename 轮转安全（与实例日志必须 copytruncate 的约束不同）。
//! - 新增事件类型 = 枚举加变体 + 登记进 [`KNOWN_CONTROL_KINDS`] /
//!   [`KNOWN_DATA_KINDS`]；载荷超限截断标 `truncated: true` 由写入方负责。
//!
//! ## 抓包会话（capture；易失）
//!
//! - `capture` 是环境字段，默认 false，PATCH 热开/关。**开关只控记录**：
//!   关 = 停止记录新请求，已有记录保留；重开续用同一会话，往返不丢记录。
//! - 会话 = 实例生命周期：记录在内存缓冲（无文件落盘），停止/重启实例即全部
//!   丢弃。丢弃路径只有两个：手动 clear（缓冲清零，会话延续，generation +1）
//!   与实例消亡。`session.id / started_at / generation` 供 UI 明示。
//! - 记录形状：`{version, session, request_id, time, duration_ms,
//!   request{method,path,authority,headers,body}, response{status,headers,body}, error}`；
//!   duration_ms 是上游交换耗时（请求行到达 → 响应体收齐，不含写回客户端）。
//!   成对落"盘"（内存条目），无半条记录。
//! - **缺失而非截断**：单侧 body 超 256 KiB → 只记头与元数据、body 标
//!   `omitted: true`；body 按原始字节存（utf8 直存 / 二进制 base64 标注），
//!   解码延迟到展示端。
//! - 缓冲按字节预算淘汰（默认 256 MiB/环境），满即淘最旧，captured/dropped
//!   计数随 API 可见；长会话不丢记录的出路是导出（HAR 1.2 / jsonl）。
//!
//! ## 会话双轨：调试会话（live）与导入会话（HAR）
//!
//! - **调试会话是工作区级单例**：`start_debug({"env": …})` 开启/切换目标；
//!   **切换即换代 —— 原目标环境的抓包停止并清空**，新环境开启。环境字段
//!   capture 仍是 per-env 开关，调试会话经 debug 那组门面方法驱动它。
//!   实时取数：`debug_view()` 整幅 + `debug_delta(after, limit)` 增量
//!   （游标域 = request_id，帧语义见 `envboard-protocol` crate 文档）；
//!   记录正文只经这一路与 captures 出口。
//! - **导入会话（HAR）**：多个并存、只读、进程生命周期内易失；entries 反向
//!   映射成抓包记录形状（与调试会话同一渲染路径）。有界：最多 8 个会话 /
//!   总量 256 MiB，超限**拒收**；畸形 HAR → `invalid_config` 响亮拒绝。

mod control;
mod data;
mod envelope;
mod store;

pub use control::{ControlEvent, KNOWN_CONTROL_KINDS};
pub use data::{DataEvent, KNOWN_DATA_KINDS};
pub use envelope::Envelope;
pub use store::{
    FORMAT_VERSION, ParseError, ParsedEvent, append_line, header_line, parse_log, parse_window,
};

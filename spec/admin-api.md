# Admin 管理 API 契约

| 项 | 值 |
|---|---|
| 适用范围 | envboard 控制面的全部读写端点（`/api/*`）与三条推送流 |
| 词汇唯一定义处 | `crates/protocol`（视图 DTO、请求 DTO、帧形状、错误信封）；本文件是它的语言中立契约 |
| 端点实现 | `crates/admin`（AdminService，校验与用例编排）→ `crates/manager`（账本与装配） |
| 传输绑定 | `crates/web`（HTTP/SSE + 安全中间件）；桌面形态可换任何进程间通道而不动本契约 |
| 错误码 | 见 [errors.md](errors.md)；推送流帧见 [protocol.md](protocol.md)；事件信封见 [events.md](events.md) |
| 版本 | v1.0（2026-09-21） |

本文件回答「Admin API 有哪些端点、每个端点吃什么吐什么」。**URL 与响应形状
是对外契约**：工作台、live 验收与未来桌面形态共同依赖，改形状必须同提交改本文件。

## 全局约定

- **鉴权分档**（`crates/web` 强制，不属本契约但影响调用）：回环监听默认免鉴权；
  `--token` 启用后所有端点（两个静态资产除外）要求头 `x-envboard-token` 或 query
  `?token=`；非回环监听必须显式给 token。
- **变更类请求**（一切非 GET/HEAD）必须带 `x-envboard-request: 1`，否则 403（CSRF 防线）。
- **错误信封**：`{"error":{"code","message","field"?}}`；HTTP 状态由错误码映射。
- **凭据只进不出**：代理鉴权与上游凭据永不回显（`has_auth` / `proxy_auth_enabled` 布尔）。
- **请求 DTO 一律 `deny_unknown_fields`**：未知字段响亮拒绝（`invalid_config`，
  field 指向具体路径）。

## 端点分组

### status —— 运行时概览

| 方法 路径 | 请求 | 响应 |
|---|---|---|
| GET `/api/status` | — | `{version, core{name,version}, capabilities{…}, config{state_dir, port_range}, environments, running, events_dropped}` |

### environments —— 环境生命周期

请求 DTO：`EnvCreateReq` / `EnvPatchReq`（protocol）。响应 DTO：`EnvView`。

| 方法 路径 | 请求 | 响应 |
|---|---|---|
| GET `/api/environments` | — | `[EnvView…]`（**裸数组**） |
| GET `/api/environments/:name` | — | `EnvView` |
| POST `/api/environments` | `EnvCreateReq` | **201** `EnvView`（端口缺省时自动分配 16000–16999） |
| PATCH `/api/environments/:name` | `EnvPatchReq`（三态：未提及/显式 null 解绑/给值） | `EnvView` |
| DELETE `/api/environments/:name` | — | `{"removed": name}` |
| POST `/api/environments/:name/start` \| `stop` \| `restart` \| `reallocate` | 无 body | `EnvView` |
| GET `/api/environments/:name/logs?lines=`（默认 200） | — | `{"env", "lines":[…]}` |
| GET `/api/environments/:name/trajectory?limit=`（默认 200，≤2000） | — | `TrajectoryWindow` `{cursor, events}` |
| GET `/api/environments/:name/captures?limit=`（默认 100，≤2000） | — | `CaptureView`；无活会话 → **404** |
| GET `/api/environments/:name/captures/:request_id` | — | 单条 `CaptureRecord`（裸对象） |
| POST `/api/environments/:name/capture/clear` | — | `{"ok":true}` |
| GET `/api/environments/:name/captures/export?format=har\|jsonl` | — | attachment 下载（非 JSON API） |
| GET `/api/environments/:name/trajectory/stream` | SSE：`?cursor=`（字节偏移）续传 | 首帧 `baseline`，增量 `events`（均 `TrajectoryWindow`） |

### rules —— 规则账本

| 方法 路径 | 请求 | 响应 |
|---|---|---|
| GET `/api/rules` | — | `{"rules":[name…]}`（对象包装） |
| POST `/api/rules` | `RulesImportReq` `{name, text}` | **201** `{"name","path"}` |
| GET `/api/rules/:name` | — | `{"name","text"}` |
| DELETE `/api/rules/:name` | — | `{"removed": name}`（绑定它的环境转 `rules_missing`） |

### proxies —— 上游代理账本

| 方法 路径 | 请求 | 响应 |
|---|---|---|
| GET `/api/proxies` | — | `[ProxyView…]`（**裸数组**，含 `references`） |
| GET `/api/proxies/:name` | — | `ProxyView` |
| POST `/api/proxies` | `ProxyPutReq`（put 语义：同名覆盖） | **201** `ProxyView` |
| DELETE `/api/proxies/:name` | — | `{"removed": name}`；被引用 → 拒绝并点名 |

### debug + captures —— 工作区级抓包单例

| 方法 路径 | 请求 | 响应 |
|---|---|---|
| GET `/api/debug` | — | `DebugView` `{env, capture}`；无会话 → `{"env":null,"capture":null}`（200） |
| POST `/api/debug` | `DebugStartReq` `{env}` | `DebugView` |
| POST `/api/debug/stop` | — | `{"ok": bool}` |
| GET `/api/debug/stream` | SSE（不续传，重连即 snapshot） | `snapshot`=`DebugSnapshot`；`events`=`DebugEvents`（request_id 域游标） |

### har —— 导入会话

| 方法 路径 | 请求 | 响应 |
|---|---|---|
| GET `/api/har` | — | `{"sessions":[…]}` |
| POST `/api/har?name=`（query 可选，默认 `imported.har`） | 裸 HAR 1.2 JSON | 导入会话 view |
| GET `/api/har/:id?limit=`（默认 200，≤5000） | — | 会话 view |
| DELETE `/api/har/:id` | — | `{"ok": bool}` |

### activity —— 控制面审计（只读）

| 方法 路径 | 请求 | 响应 |
|---|---|---|
| GET `/api/history?name=&limit=`（默认 100，≤1000） | — | `{"events":[信封…]}`（词汇表见 events.md） |

### ca-settings —— 证书与配置（只读）

| 方法 路径 | 请求 | 响应 |
|---|---|---|
| GET `/api/ca` | — | CA 摘要 `CertInfo`（version/serial_hex/not_before/not_after/sig_alg/pubkey_alg/pubkey_curve/pubkey_bits/DN 字段）；CA 不可读 → 404 |
| GET `/api/ca.pem` | — | PEM 字节（attachment，非 JSON） |
| GET `/api/ca/qrcode.svg?data=`（≤512 B） | — | `image/svg+xml` |

### compare —— 跨环境确定性查账

| 方法 路径 | 请求 | 响应 |
|---|---|---|
| GET `/api/compare?host=`（必填） | — | `{"host", "environments":[{env, port, rules, ip, covered}…]}` |

### events —— 快照流（UI 状态总线）

| 方法 路径 | 请求 | 响应 |
|---|---|---|
| GET `/api/events` | SSE，固定 1s 轮推 | 唯一事件 `snapshot` = `SnapshotFrame` `{ok, cursor, environments, error?}`；`cursor` 为 **advisory** 代次，UI 不得据此跳过渲染 |

### fault —— 测试探针（test-only）

| 方法 路径 | 请求 | 响应 |
|---|---|---|
| POST `/api/_fault` | `{env, reason?}` | `{"injected":true,"env"}`；不支持 → 400（**纯文本**，非 JSON 信封，仅此端点） |

## 推送流帧速查

| 流 | 事件名 | 帧 DTO | 游标域 |
|---|---|---|---|
| `/api/events` | `snapshot` | `SnapshotFrame` | 控制面代次（advisory） |
| `/api/debug/stream` | `snapshot` / `events` | `DebugSnapshot` / `DebugEvents` | 会话内最新 request_id |
| `/api/environments/:name/trajectory/stream` | `baseline` / `events` | `TrajectoryWindow` | jsonl 字节偏移 |

## 修订规则

1. 端点增删改必须同提交改本文件与 `crates/protocol`（形状）/`crates/admin`（实现）。
2. 请求 DTO 的字段清单与 `envboard-engine::domain::KNOWN_FIELDS` 对齐；domain 加字段
   时本契约与 protocol 同步跟进。
3. 裸数组 / 裸对象 / 包装对象是各端点的**既有形状**，统一化重构（如全部加包装键）
   属破坏性变更，须升本文件主版本。

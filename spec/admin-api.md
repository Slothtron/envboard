# Admin 管理门面契约（`AdminService`）

| 项 | 值 |
|---|---|
| 适用范围 | envboard 控制面的**全部读写动作**与三路取数 |
| 词汇唯一定义处 | `crates/protocol`（视图 DTO、请求 DTO、帧形状）；本文件是它的语言中立契约 |
| 实现 | `crates/admin`（`AdminService`：入口校验 + 用例编排）→ `crates/manager`（账本与装配） |
| 消费方 | `crates/desktop`（进程内直连，经 `crates/host` 交付的门面） |
| 错误码 | 见 [errors.md](errors.md)；视图与帧见 [protocol.md](protocol.md)；事件信封见 [events.md](events.md) |
| 版本 | v2.0（2026-10-29）：v1.0 写的是 HTTP 端点表，随 web 形态出库；方法语义与形状一条没改（沿革见 docs/adr-003-retire-web-frontend.md） |

本文件回答「控制面能做哪些动作、每个动作吃什么吐什么、什么时候生效」。
**方法名、入参出参形状、生效方式是对外契约**：界面、契约测试与验收共同依赖，
改形状必须同提交改本文件与 `crates/protocol`。

## 全局约定

- **没有传输，也就没有鉴权档**：门面只在同进程内被调用。曾经的回环免鉴权 /
  `--token` / CSRF 头三档随 HTTP 面出库；要对外提供服务等于要把那一层带回来，
  那时这三档必须一起回来，不是可选项（见 spec/protocol.md「安全约定」）。
- **请求体是 JSON `Value`，不是结构体参数**：写动作收 `&Value`，先过 `crates/protocol`
  的类型化请求 DTO（`deny_unknown_fields`，未知字段响亮拒绝 → `invalid_config` +
  `field` 指向具体路径），再转发 manager。界面因此不需要（也不许）自己拼领域对象。
- **错误一律 `Error` 信封**：`{code, message, field?}`；界面只翻译不判定，
  能不能重试来自 `code` 的 retryability。
- **凭据只进不出**：代理鉴权与环境鉴权都只回显布尔（`has_auth` /
  `proxy_auth_enabled`），永不回显用户名与密码。
- **生效方式写在方法上**：下表「生效」列标 `热` 的表示不重启实例即生效，
  标 `停机` 的表示需要（或会触发）实例重启。这条是界面文案的依据，
  界面不许自己猜。
- **审计**：控制面写入必须走 `Manager::commit`（先落盘后发事件），旁路就没有
  activity 记录（deps 门禁第 10 条钉住发射点唯一）。

## status / snapshot —— 运行时概览与取数总线

| 方法 | 出参 | 说明 |
|---|---|---|
| `snapshot()` | `Result<(u64, Vec<EnvView>)>` | **界面主循环**：全量快照 + 账本代次。代次是 advisory，界面不得据此跳过渲染；桌面按 1s 轮询驱动它 |
| `state_generation()` | `u64` | 当前账本代次（只作展示参考） |
| `core_info()` | `CoreInfo` | 版本与核信息 |
| `capabilities()` | `CoreCapabilities` | 能力位（对应 spec/capabilities.md） |
| `config()` | `ManagerConfig` | 状态目录、端口区间等运行时配置（**只读**，改配置属于组合根入参，不属于门面） |
| `events_dropped()` | `u64` | 事件环形缓冲丢弃计数（诊断用） |

## environments —— 环境生命周期

请求 DTO：`EnvCreateReq` / `EnvPatchReq`。响应 DTO：`EnvView`。

| 方法 | 生效 | 说明 |
|---|---|---|
| `list()` | — | 全量 `EnvView` |
| `get(name)` | — | 单个环境；不存在 → `not_found` |
| `create(body)` | 停机（新建即按 desired 起） | 端口缺省时在配置区间自动分配 |
| `update(name, patch)` | 逐字段 | patch 三态：未提及 = 不动；显式 `null` = 解绑；给值 = 设定。热应用被拒时环境标 `failed` 并给原因，**调用方必须补一次 `snapshot()`** 才看得见 |
| `remove(name)` | 停机 | 删除账本与实例 |
| `start(name)` / `stop(name)` / `restart(name)` | 启停 | `async`：要等端口定态，界面不许在 main 线程上阻塞等它 |
| `reallocate_port(name)` | 停机 | 重新分配监听端口 |
| `reconcile()` | — | `async`：按期望状态收敛，返回 `ReconcileReport`（宿主编排用，界面一般不直接调） |
| `logs_tail(name, lines)` | — | 实例日志尾部（默认 200 行） |
| `trajectory(name, limit)` | — | 轨迹窗口 `(cursor, events)`，默认 200、上限 2000 |
| `trajectory_since(name, offset)` | — | 按**字节偏移**续取（游标域见 spec/protocol.md） |
| `is_live(name)` | — | 该环境当前有没有活实例（布尔，不解释原因） |

## rules —— 规则账本

| 方法 | 生效 | 说明 |
|---|---|---|
| `rules_list()` | — | 名字数组（顺序即账本顺序） |
| `import_rules(name, text)` | 热 | 覆盖式导入；返回物化文件路径。正文解析与拒绝规则见 spec/rules.md |
| `rules_read(name)` | — | 规则正文原文（界面的「查看原文」与详情规则页签共用） |
| `rules_delete(name)` | 热 | 绑定它的环境转 `rules_missing`，不再覆盖任何域名 |

## proxies —— 上游代理账本

| 方法 | 生效 | 说明 |
|---|---|---|
| `proxy_list()` | — | `ProxyView` 数组，含 `references`（引用它的环境名） |
| `proxy_get(name)` | — | 单个 `ProxyView` |
| `proxy_put(body)` | 热 | **put 语义**：同名整条覆盖。`port` 必须是 JSON 数字；`user`/`password` 要么成对给、要么都不出现（空串是非法凭据）。不鉴权 = 这两个字段缺席，**不是**空串 |
| `proxy_delete(name)` | 热 | 被引用 → 拒绝并在错误里点名引用方环境 |

## debug / captures —— 工作区级抓包（P2 界面）

| 方法 | 生效 | 说明 |
|---|---|---|
| `debug_view()` | — | `Option<DebugView>`；无会话 = `None` |
| `start_debug(body)` | 热 | 收 `DebugStartReq` `{env}` |
| `stop_debug()` | — | 关闭当前会话 |
| `debug_delta(after, limit)` | — | 增量 `(最新游标, DebugEvents)`；游标域 = 会话内 `request_id` |
| `captures(name, limit)` | — | `Option<CaptureView>`；无活会话 → `None` |
| `capture_detail(name, request_id)` | — | 单条记录 |
| `clear_capture(name)` | — | 清空当前会话记录（`generation` +1） |
| `inject_fault(name, reason)` | — | **测试探针**，只给验收用；引擎不支持 → `false` |

## har —— 导入会话

| 方法 | 说明 |
|---|---|
| `har_list()` | 会话列表 |
| `har_import(name, body)` | 导入 HAR 1.2（`name` 缺省 `imported.har`）；体积上限由 manager 判，不在界面判 |
| `har_get(id, limit)` | 单会话（默认 200、上限 5000） |
| `har_delete(id)` | 删除会话 |

## activity / compare —— 审计与确定性查账（只读）

| 方法 | 说明 |
|---|---|
| `history(name, limit)` | 控制面事件（默认 100、上限 1000），信封词汇见 spec/events.md |
| `compare(host)` | 同一域名在所有环境里各自解析到哪里、由哪条规则决定 —— 纯查账，不发真实请求 |

## 不在门面上的东西

- **CA**：摘要与 PEM 由组合根交付（`Prepared::ca`，`CaBundle`），不经过门面；
  **私钥永远不在任何交付给界面的结构里**。曾经的「下载 pem / 二维码配对」是 HTTP
  端点，随形态出库；桌面要做扫码装证书属于 P2 设置屏，需要重新决定渲染件。
- **状态目录 / 端口区间 / 日志策略**：`HostOptions` 的入参，属于启动方式而非控制面。
- **单实例锁**：`envboard-host` 的职责，门面不感知（拿不到锁根本构造不出来）。

## 修订规则

1. 方法增删改必须同提交改本文件、`crates/protocol`（形状）与 `crates/admin`（实现）；
   界面消费方式变了也要改 spec/desktop-parity.md 的对账表。
2. 请求 DTO 的字段清单与 `envboard-engine::domain::KNOWN_FIELDS` 对齐；domain 加字段
   时本契约与 protocol 同步跟进。
3. 「生效」列是契约的一部分：把热生效改成停机生效，等同于破坏性变更。
4. 返回值的裸数组 / 裸对象 / 包装对象形状是**既有形状**，统一化重构属破坏性变更，
   须升本文件主版本。

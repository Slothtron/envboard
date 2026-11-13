//! envboard 环境管理器 —— 环境 CRUD、端口分配、进程编排、状态持久化、健康
//! 检查。本 crate 文档承载管理面契约；纯逻辑裁定（环境字段、规则、端口选择、
//! reconcile 决策）在 `envboard-engine::domain`，golden fixture 在
//! `tests/contract-tests/fixtures/`。
//!
//! 分层（依赖单向）：
//!
//! ```text
//! envboard-protocol（UI 边界词汇：视图 DTO / 帧形状）
//!        ↑
//! envboard-engine（trait / 契约词汇）
//!        ↑
//! engine::domain / engine::rules（纯逻辑模块）
//!        ↑                        ↑
//!        └──── envboard-manager ──┘
//!                     ↑
//!              ports（trait） + infra（真实实现）
//! ```
//!
//! 管理器只依赖 [`ProxyEngine`](envboard_engine::ProxyEngine) 抽象，不认识任何具体引擎实现。
//!
//! ## 能力清单（契约测试的能力注册表与之对应）
//!
//! | 能力 id | 契约 | fixture 目录 |
//! |---|---|---|
//! | `environment.validate` | 环境定义合法性、归一化与默认值、错误码与 field 路径 | environment/ |
//! | `environment.merge` | patch 语义（未提及不动；显式 null=清空；合并后重新校验） | merge/ |
//! | `rules.parse` | hosts 输入 → 规范化规则（见 engine::rules 模块文档） | rules/ |
//! | `insecure.hosts` | 按域名放宽上游证书校验的归一化与精确相等命中 | insecure/ |
//! | `port.allocate` | 端口选择语义（候选序列 → 选中端口） | ports/ |
//! | `instance.reconcile` | 期望与实际对齐：顺序、端口冲突标记 | lifecycle/ |
//! | `proxy.validate` | 上游代理实体合法性 | proxy/ |
//!
//! fixture 只放**纯逻辑**的确定性 golden；真网络与宿主能力（进程起停、CA
//! 物化、浏览器行为）由实机层测试覆盖（`mise run verify-live`）。
//!
//! ## 端口分配
//!
//! 端口是环境的身份，分配后**不再变**。契约只裁选择规则，不裁随机性（候选
//! 序列在 port_range 内随机打乱由管理器负责，随机源不进契约）。
//! 无显式指定 → 按候选顺序取第一个不在 taken 里的；已分配环境重启直接用持久化
//! 端口。显式指定被占 → `invalid_config`，绝不静默改（可以越出区间，用途就是
//! 人工固定）。连续 N 次（默认 32）候选不可用 → `port_range_exhausted` 响亮
//! 失败。区间缩小不迁移：已持久化端口落在新区间外也保留，只告警。
//! **试绑语义**：绑定地址与 listen.host 一致；**必须关 SO_REUSEADDR**（否则
//! 误判空闲）；试绑成功即释放。试绑与真正监听之间有 TOCTOU 窗口：**新建**
//! 环境启动遇端口冲突自动换端口重试**一次**；**已持久化**端口冲突禁止静默
//! 重分配 → 标 port_conflict，由工作台显式提示重分配。
//!
//! ## 实例生命周期与健康
//!
//! 持久化的是**期望状态** desired: running | stopped；实际状态来自引擎**内存
//! 报告**（实例不出进程边界，没有跨进程身份错认的介质）。reconcile（启动时
//! 一次 + 常驻循环）顺序固定：**先清理，后启动**；desired=running 但实例不在
//! 跑（含崩溃后）→ 重新 start，这就是崩溃自愈。
//!
//! 健康判定次序（视图与权威判定**同一个函数**，见 reconcile_health 模块）：
//! **标记 → desired → 引擎内存报告**。
//!
//! | 状态 | 含义 |
//! |---|---|
//! | stopped | 无登记实例（期望停止，或从未启动/已被停止） |
//! | starting | 已下发启动，绑定结果未定态 |
//! | running | 引擎线程存活且 listener 已绑定；配置装配即生效 |
//! | port_conflict | 绑定失败（EADDRINUSE）；不换端口 |
//! | unhealthy | 线程存活但监听/装配异常：accept 持续失败，或配置被 apply 拒绝（旧快照仍在服务） |
//! | failed | 引擎线程终止（panic，原因在 health_reason）→ reconcile 按 desired 自动重启 |
//!
//! **崩溃隔离是任务级**：请求任务 panic 只死那一条连接；引擎线程 panic 才让
//! 实例进入 failed。release 构建必须保持 unwind（`panic = "abort"` 使任务级
//! 隔离失效）—— 属构建契约。
//!
//! ## 规则库账本
//!
//! **账本是唯一真相**：state.json 的 rules[] 记 `{name, source, rendered,
//! entries, skipped, conflicts, imported_at}`，rendered 是规范化渲染后的完整
//! 正文；`<rules_dir>/<name>.rules` 只是可再生物化产物（引擎的规则输入是账本
//! rendered 经 EngineSpec 直供）。导入：解析 → 渲染 → 更新账本 → 写物化文件
//! （0600）→ 保存 → 对绑定且在跑的环境逐个热应用。对账（启动与常驻循环，
//! 幂等）：物化有而账本无 → 回填；账本有而物化缺失/坏 → 按 rendered 重建。
//! 删除仍拒绝"被任何环境绑定"的规则；绑定不存在的规则名写入时即拒。
//! `rules_missing` = 账本里没有该名字的规则。
//!
//! ## 上游代理账本
//!
//! state.json 的 proxies[]：`{name, host, port, user, password}`。name 白名单
//! 与环境/规则名同形；host 是域名或 IP 字面量（归一化小写、去尾点）；port
//! 1..=65535；user/password 同生共死（≤64 / ≤128 字符）。未知字段响亮失败
//! （field = `proxy.<key>`）。凭据明文只落 0600 状态文件，API/视图/日志永不
//! 回显（只有 has_auth 布尔）。环境 upstream 按名引用（null=直连，热字段）；
//! 删除被引用代理 → conflict 点名引用方；绑定不存在名字 → invalid_config；
//! 手改出的悬空引用加载即拒启。改代理实体 host/port/凭据对运行中环境是热的
//! （重编译 spec → 同步 apply）。建连语义见 `envboard-engine` crate 文档。
//!
//! ## 状态目录与实例日志
//!
//! 状态目录里能出现什么是规范，路径字面量唯一来源是 [`state::layout`]；
//! 目录怎么定下来（回退链：调用方 → ENVBOARD_STATE_DIR → 便携标记 → 平台
//! 默认）见 `envboard-host::options` 文档。
//!
//! 实例日志：每环境一份 `<state_dir>/logs/<env>.log`，追加不截断（每次启动写
//! 运行标记分段）。**数据面永不阻塞在写日志上**：请求终点把单行摘要 try_send
//! 进有界总线（1024 行），满即丢弃并计数（EngineReport.log_drops），专用泵
//! 线程落盘。体积封顶 max_log_bytes（默认 8 MiB；0=不轮转；下限 64 KiB），
//! 轮转**只能 copytruncate**（禁止 rename：写方还持着旧 fd）。读尾部有界
//! （256 KiB 窗口再切行，允许跨 .1）。轨迹目录与日志目录**平级**且共用
//! "要不要落盘"开关（数据面事件流不是可丢弃的诊断产物）。

pub mod agent;
pub mod events;
pub mod har;
pub mod infra;
pub mod logs;
pub mod manager;
pub mod ports;
pub mod state;
pub mod state_sqlite;

// Manager 的域拆分：各文件以 `impl Manager` 扩展同一类型（见 manager.rs 头注）。
mod debug_capture;
mod ledger;
mod lifecycle;
mod observability;
mod projection;
mod proxies;
mod reconcile_health;
mod rules_store;

pub use envboard_protocol::{DebugView, Desired, EnvView, ProxyView, ReconcileReport};
pub use manager::Manager;
pub use ports::{FilePort, MemoryFiles, MemoryPortProbe, MemoryStateRepo, PortProbe, StateRepo};
pub use state::{
    InstanceLock, JsonFileStateRepo, ManagerConfig, PersistedState, STATE_VERSION, StoredError,
    StoredRules, layout,
};
pub use state_sqlite::SqliteStateRepo;

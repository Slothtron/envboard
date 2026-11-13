//! envboard-admin —— **Admin 管理 API 的门面**（传输无关），本 crate 文档即
//! 控制面方法契约：能做哪些动作、每个动作吃什么吐什么、什么时候生效。
//!
//! 分层（依赖单向）：
//!
//! ```text
//! envboard-protocol（词汇） ← envboard-manager（账本与装配编排） ← envboard-admin（本 crate）
//!                                                      ↑
//!                                        envboard-desktop（Tauri IPC 传输绑定）
//! ```
//!
//! ## 全局约定
//!
//! - **门面不认识传输；圈禁在传输层**：[`AdminService`] 只在同进程内被
//!   `envboard-desktop` 的 IPC 命令调用。capabilities 圈禁与 CSP 是 Tauri 绑定的
//!   职责（安全约定见 `envboard-protocol` crate 文档）。历次换传输
//!   （GPUI → web → Tauri）门面零漂移，是这条拆分的直接兑现。
//! - **请求体是 JSON `Value`，不是结构体参数**：写动作收 `&Value`，先过
//!   [`protocol`](envboard_protocol) 的类型化请求 DTO（`deny_unknown_fields`，
//!   未知字段响亮拒绝 → `invalid_config` + `field` 指向具体路径），再转发
//!   manager。界面不需要（也不许）自己拼领域对象。
//! - **统一错误信封**：所有失败以 [`Error`] 返回
//!   `{code, message, field?}`；码表与可重试性见 `envboard-engine::api::error`
//!   模块文档。界面只翻译不判定。
//! - **凭据只进不出**：代理鉴权与环境鉴权都只回显布尔（`has_auth` /
//!   `proxy_auth_enabled`），永不回显用户名与密码。
//! - **生效方式写在方法上**：下表「生效」列标 *热* 的表示不重启实例即生效，
//!   标 *停机* 的表示需要（或会触发）实例重启。这是界面文案的依据，界面不许
//!   自己猜；把热生效改成停机生效等同于破坏性变更。
//! - **审计**：控制面写入必须走 `Manager::commit`（先落盘后发事件），旁路就
//!   没有 activity 记录。
//! - **不认识引擎装配**：装配符号只住在 `envboard-host`（唯一组合根）。
//!
//! ## status / snapshot —— 运行时概览与取数总线
//!
//! | 方法 | 出参 | 说明 |
//! |---|---|---|
//! | `snapshot()` | `(u64, Vec<EnvView>)` | 界面主循环：全量快照 + 账本代次（advisory，不得据此跳过渲染） |
//! | `state_generation()` | `u64` | 当前账本代次（只作展示参考） |
//! | `core_info()` / `capabilities()` | `CoreInfo` / `CoreCapabilities` | 版本核信息 / 能力位 |
//! | `config()` | `ManagerConfig` | 运行时配置**只读**；改配置属组合根入参，不属于门面 |
//! | `events_dropped()` | `u64` | 事件环形缓冲丢弃计数（诊断用） |
//!
//! ## environments —— 环境生命周期
//!
//! 请求 DTO：`EnvCreateReq` / `EnvPatchReq`；响应 DTO：`EnvView`。
//!
//! | 方法 | 生效 | 说明 |
//! |---|---|---|
//! | `list()` / `get(name)` | — | 全量 / 单个；不存在 → `not_found` |
//! | `create(body)` | 停机（新建即按 desired 起） | 端口缺省时在配置区间自动分配 |
//! | `update(name, patch)` | 逐字段 | patch 三态：未提及=不动；显式 null=解绑；给值=设定。热应用被拒时环境标 failed 并给原因，**调用方必须补一次 snapshot()** 才看得见 |
//! | `remove(name)` | 停机 | 删除账本与实例 |
//! | `start/stop/restart(name)` | 启停 | async：要等端口定态，界面不许阻塞等它 |
//! | `reallocate_port(name)` | 停机 | 重新分配监听端口（显式动作） |
//! | `reconcile()` | — | async：按期望状态收敛，返回 ReconcileReport（宿主编排用） |
//! | `logs_tail(name, lines)` | — | 实例日志尾部（默认 200 行） |
//! | `is_live(name)` | — | 有没有活实例（布尔，不解释原因） |
//!
//! ## rules —— 规则账本
//!
//! | 方法 | 生效 | 说明 |
//! |---|---|---|
//! | `rules_list()` | — | 名字数组（顺序即账本顺序） |
//! | `import_rules(name, text)` | 热 | 覆盖式导入；返回物化文件路径。语法与拒绝规则见 `envboard-engine::rules` 模块文档 |
//! | `rules_read(name)` | — | 规则正文原文 |
//! | `rules_delete(name)` | 热 | 绑定它的环境转 rules_missing，不再覆盖任何域名 |
//!
//! ## proxies —— 上游代理账本
//!
//! | 方法 | 生效 | 说明 |
//! |---|---|---|
//! | `proxy_list()` / `proxy_get(name)` | — | ProxyView（含 references 引用方环境名） |
//! | `proxy_put(body)` | 热 | **put 语义**：同名整条覆盖。port 必须是 JSON 数字；user/password 成对给或都不出现（空串是非法凭据；不鉴权 = 字段缺席，**不是**空串） |
//! | `proxy_delete(name)` | 热 | 被引用 → 拒绝并点名引用方环境 |
//!
//! ## debug / captures —— 工作区级抓包
//!
//! | 方法 | 说明 |
//! |---|---|
//! | `debug_view()` | `Option<DebugView>`；无会话 = None |
//! | `start_debug(body)` / `stop_debug()` | 收 `DebugStartReq {env}`；切换即换代（原目标停止并清空） |
//! | `debug_delta(after, limit)` | 增量 `(最新游标, DebugEvents)`；游标域 = 会话内 request_id |
//! | `captures(name, limit)` / `capture_detail(name, request_id)` | 无活会话 → None；单条记录 |
//! | `clear_capture(name)` | 清空当前会话记录（generation +1） |
//! | `inject_fault(name, reason)` | **测试探针**，只给验收用；引擎不支持 → false |
//!
//! ## har / activity / compare
//!
//! | 方法 | 说明 |
//! |---|---|
//! | `har_list()` / `har_import(name, body)` / `har_get(id, limit)` / `har_delete(id)` | HAR 1.2 导入会话（name 缺省 imported.har；体积上限由 manager 判；get 默认 200、上限 5000） |
//! | `history(name, limit)` | 控制面事件（默认 100、上限 1000），信封见 `envboard-events` |
//! | `compare(host)` | 同一域名在所有环境里各自解析到哪里、由哪条规则决定 —— 纯查账，不发真实请求 |
//!
//! ## 不在门面上的东西
//!
//! - **CA**：操作面由组合根交付（`Prepared::ca`，`Option<CaControl>`），不经过
//!   门面；**CA 私钥永远不在任何交付给界面的结构里**。CaControl 提供摘要 /
//!   指纹 / PEM / 证书路径 / rotate / issue_pem / 信任库只读探测。
//! - **状态目录 / 端口区间 / 日志策略**：`HostOptions` 入参，属启动方式而非控制面。
//! - **单实例锁**：`envboard-host` 的职责，门面不感知。
//!
//! ## 传输绑定
//!
//! 方法契约到 IPC 命令的映射住在 `envboard-desktop` 的 ops.rs（纯函数核，
//! 命令级契约测试直接打它）+ commands.rs（薄壳）：一方法一命令，两路取数走
//! Tauri Channel（`watch_environments`、`watch_debug`），CA 操作面绑定为
//! `ca_*` 命令。**命令名不是方法契约的一部分**：换传输可以重新绑定，
//! 方法语义不动（旧 HTTP 端点映射随 web 形态退场，git 历史可查）。
//!
//! ## 修订规则
//!
//! 方法增删改必须同提交改本 crate（实现）、`envboard-protocol`（形状）与
//! `envboard-desktop` 的 ops.rs/commands.rs（绑定）；请求 DTO 字段清单与
//! `envboard-engine::domain::KNOWN_FIELDS` 对齐；返回值的裸数组 / 裸对象 /
//! 包装对象形状是既有形状，统一化重构属破坏性变更。

mod activity;
mod debug;
mod environments;
mod har;
mod proxies;
mod rules;
mod settings;

use std::sync::Arc;

use envboard_engine::Error;
use envboard_manager::Manager;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Admin API 的服务面：包一个 [`Manager`]，一域一模块（见各文件头注释）。
#[derive(Clone)]
pub struct AdminService {
    manager: Arc<Manager>,
}

impl AdminService {
    pub fn new(manager: Arc<Manager>) -> Self {
        Self { manager }
    }

    /// 从裸 JSON 解析类型化请求；失败归 `invalid_config`（field 指向请求体）。
    ///
    /// DTO 只做**形状**校验（键集合与类型）；字段级语义校验仍归 domain
    /// （`Environment::from_json` 等），两层共用同一份词汇。
    fn parse_request<T: DeserializeOwned>(field: &str, body: &Value) -> Result<T, Error> {
        serde_json::from_value(body.clone()).map_err(|error| {
            Error::invalid_config(field, format!("malformed request body: {error}"))
        })
    }
}

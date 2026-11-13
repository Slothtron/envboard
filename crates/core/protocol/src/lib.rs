//! envboard 的 UI 边界词汇：**跨界面形态复用的数据形状，本 crate 文档即契约**。
//!
//! 工作台不止一种界面形态（今天的 web 控制台、将来的 Tauri 壳），它们消费的是
//! 同一份词汇：环境/代理/抓包的视图 DTO、推送流的帧形状、游标与错误信封。
//! 本 crate 把这些形状收拢到唯一定义处 —— 纯 serde 数据结构，零传输依赖；
//! HTTP/SSE 只是 web 形态对它们的绑定，不在这份词汇里。
//!
//! 分层：叶子 crate（不依赖任何内部 crate）。`envboard-engine` 与
//! `envboard-manager` 在服务端构造这些类型，界面形态 crate 消费它们。
//! 词汇不认识传输：换界面、换通道都不改这份契约。门面上有哪些方法见
//! `envboard-admin` crate 文档；事件信封与两档存储见 `envboard-events` crate 文档。
//!
//! ## 错误信封
//!
//! 门面返回的 `Error` 与快照帧里的错误共用一个形状：
//! `{"error": {"code": "not_found", "message": "…", "field": "…"}}`。
//! `code` 的契约（码表 / 可重试性）在 `envboard-engine::api::error` 模块文档；
//! `field` 可选，指向配置字段的点分路径。界面只翻译不判定：能不能重试来自
//! `code` 的 retryability。
//!
//! ## 视图 DTO
//!
//! 服务端构造，界面只读；凭据**永不回显**（代理只回显 `has_auth` /
//! `proxy_auth_enabled` 布尔）。各类型的字段语义见其自身文档，关键约定：
//!
//! - [`EnvView`]：`desired` ∈ running | stopped；`health` 是健康字面量
//!   （starting / running / failed / stopped / port_conflict / unhealthy），
//!   `health_reason` 是配对的原因短语（健康态为 null）；`rules_missing` = true
//!   表示绑定的规则名在账本里不存在（此时 `rules_count` 恒 0）；
//!   `proxy_command` 由服务端算好回显，界面不许自己拼。
//! - [`ProxyView`]：`references` 列出引用这条代理的环境名（删除确认的前置信息）。
//! - [`CaptureView`] / [`SessionInfo`]：记录形状与易失语义（会话 = 实例生命周期）
//!   见 `envboard-events` crate 文档「抓包会话」。
//! - [`DebugView`]：`{env, capture}`；[`ReconcileReport`]：
//!   `{actions: [[env, action]…], warnings: […]}`。
//!
//! ## 快照与增量：帧语义与游标域
//!
//! 两条推送流（Tauri Channel，ADR-006）共用帧名 `snapshot / events`：
//!
//! | 帧名 | 语义 | 用在哪路数据 |
//! |---|---|---|
//! | `snapshot` | 整幅替换（全环境快照 / 调试会话视图） | 快照、调试实时 |
//! | `events` | 增量追加 | 调试实时 |
//!
//! 统一规则：**每帧都带显式 `cursor`（裸 u64）**，每路数据只用自己那个域，
//! 混用即实现错误；消费方存最新游标用于续取或展示；界面对未知字段宽容
//! （fail-open），服务端对未知事件类型 fail-closed（见 `envboard-events`）。
//!
//! | 路 | 取数方式 | 游标域 | 续传语义 |
//! |---|---|---|---|
//! | 快照 | `watch_environments`（Tauri Channel，1s 节拍推 snapshot） | 控制面账本代次（每次 commit +1） | **advisory**：健康变化不经控制面写入，同代次内容仍可能不同，界面不得据此跳过渲染 |
//! | 调试实时 | `debug_delta(after, limit)` | 抓包记录的 `request_id` | 淘汰只移除游标之前的记录，增量无缺口；换代或目标消失 → 重收 snapshot，不续传 |
//!
//! 帧形状明细：[`SnapshotFrame`]
//! `{ok, cursor, environments, error?}`（ok:false 时 environments 为空）；
//! [`DebugSnapshot`] `{cursor, env, capture}`（无会话时 env/capture 为 null、
//! cursor 为 0；会话换代即 `session.id`/`generation` 变化 → 重发 snapshot）；
//! [`DebugEvents`] `{cursor, records, captured, dropped}`。
//!
//! ## 安全约定（桌面形态，ADR-006）
//!
//! 数据面走 Tauri commands-IPC（`envboard-desktop`），**不起监听端口**：
//!
//! - **IPC 面由 capabilities 圈禁**：命令只对主窗开放（capabilities/default.json），
//!   页面 origin 是 `tauri://localhost`，外部进程与网页够不着；
//! - **严格 CSP** 钉在 tauri.conf.json：无 unsafe-inline、资产必须外置、
//!   react-aria 注入样式按 hash 豁免、`connect-src` 含 `ipc: http://ipc.localhost`。
//!
//! 旧 web 形态的三档（回环免鉴权 / `--token` / CSRF 头 / Host 校验）随 HTTP 面
//! 退场（git 历史可查）。配套纪律不变：**单实例锁**（同一状态目录只许一个宿主，
//! `envboard-host`）；**凭据只进不出**；**CA 私钥不出组合根**
//! （`envboard-host::CaControl` 只暴露证书公开面与按需签发的叶子）。
//!
//! ## 兼容性
//!
//! 视图与帧**加字段**不构成破坏性变更（消费方对未知字段宽容）；改字段语义、
//! 改游标域、删字段是破坏性变更：必须同步本 crate 的类型定义并评估界面迁移。
//! 事件词汇（信封内 type/data）的兼容规则在 `envboard-events`，与本契约独立。

use serde::{Deserialize, Serialize};
use serde_json::Value;

// --------------------------------------------------------------------------- #
// 视图 DTO（服务端构造，UI 消费）
// --------------------------------------------------------------------------- #

/// 监听端点的 wire 形状（引擎内部的 `Listen` 以 IP 类型为准，过界面时转成字符串）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}

/// 期望状态的 wire 形状。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Desired {
    Running,
    Stopped,
}

/// 一个环境对外的视图（工作台各视图与未来的 GUI 都渲染它）。
///
/// 凭据**永不回显**：上游代理只出现名字，代理鉴权只给布尔。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvView {
    pub name: String,
    pub listen: Endpoint,
    /// 绑定的规则集名（None = 未绑定）。
    pub rules: Option<String>,
    /// 上游代理绑定（按名引用账本；None = 直连）。
    pub upstream: Option<String>,
    /// 按域名放宽上游校验的完整域名清单（不是凭据，可以回显）。
    pub insecure_hosts: Vec<String>,
    /// 只对这些域名做 TLS 解密（空 = 全部解密；名单外的 CONNECT 直通）。
    pub mitm_hosts: Vec<String>,
    pub description: String,
    pub desired: Desired,
    /// 健康字面量（starting / running / failed / stopped / port_conflict …）。
    pub health: String,
    /// 健康判定的原因短语（与 `health` 配对展示；健康态为 null）。
    pub health_reason: Option<String>,
    /// 生效后的规则条数（账本解析得出）。
    pub rules_count: usize,
    /// 绑定了规则名，但账本里没有 —— 该环境当前**不覆盖任何域名**。
    pub rules_missing: bool,
    pub proxy_command: String,
    /// 代理鉴权是否启用。只给布尔，不回显凭据。
    pub proxy_auth_enabled: bool,
    /// 抓包开关（只控记录；清空走显式动作）。
    pub capture: bool,
}

/// 一条上游代理对外的视图。凭据**永不回显**，只有 `has_auth` 布尔。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyView {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub has_auth: bool,
    /// 引用这条代理的环境名（删除确认的前置信息；删除仍由服务端裁决）。
    pub references: Vec<String>,
}

/// 会话元数据：会话 = 实例生命周期；停止/重启即消失。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: u64,
    pub started_at: u64,
    /// 手动清空的代次（clear 一次 +1；用于 UI 区分前后两段）。
    pub generation: u64,
}

/// 抓包会话视图（易失：会话 = 实例生命周期）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureView {
    pub session: SessionInfo,
    pub captured: u64,
    pub dropped: u64,
    pub records: Vec<Value>,
}

/// 抓包增量视图（调试实时流的读侧原语）：会话元数据 + 计数 + 游标之后的新记录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureDelta {
    pub session: SessionInfo,
    pub captured: u64,
    pub dropped: u64,
    /// 缓冲内最旧的 request_id（None = 空缓冲）。
    pub oldest: Option<u64>,
    /// request_id 严格大于游标的记录（时间序，至多 limit 条）。
    pub records: Vec<Value>,
}

/// 调试会话视图：目标环境 + 其抓包会话。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DebugView {
    pub env: String,
    pub capture: CaptureView,
}

/// reconcile 的执行结果。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReconcileReport {
    pub actions: Vec<(String, String)>,
    pub warnings: Vec<String>,
}

// --------------------------------------------------------------------------- #
// 请求 DTO（UI → Admin API 的写入口）
// --------------------------------------------------------------------------- #
//
// 这些形状是写端点请求体的**词汇化**：此前 handler 把裸 `Value` 直传 manager，
// 校验在 domain 深处进行；Admin 层（`envboard-admin`）用这里的类型化请求做
// 入口校验，错误仍走统一信封（envboard-engine::api::error 模块文档）。字段清单与
// `envboard-engine::domain::KNOWN_FIELDS` 对齐，`deny_unknown_fields` 保持
// 「未知字段响亮拒绝」的既有契约。

/// `listen` 子对象（create 可整体缺省 = 自动分配；patch 可只给 host 或只给 port）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListenReq {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

/// `POST /api/environments` 请求体。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvCreateReq {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen: Option<ListenReq>,
    /// 绑定的规则集名；缺省 = 不覆盖。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<String>,
    /// 上游代理名；缺省 = 直连。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insecure_hosts: Option<Vec<String>>,
    /// MITM 白名单（空/缺省 = 全部解密）。与 `insecure_hosts` 同款：整体替换。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mitm_hosts: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_password: Option<String>,
}

/// patch 的「解绑」字段：`Option<Option<T>>` 三态桥接。
///
/// serde_json 默认把 JSON `null` 归为外层 `None`，与「字段缺失」不可区分；
/// 这个 with-module 把 null 映射为 `Some(None)`（显式解绑），缺失走 `default`
/// 映射为 `None`（未提及）——merge 语义的生命线。
mod double_option {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn deserialize<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
    where
        T: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        Option::<T>::deserialize(deserializer).map(Option::Some)
    }

    pub fn serialize<T, S>(value: &Option<Option<T>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        T: serde::Serialize,
        S: Serializer,
    {
        use serde::Serialize;
        match value {
            Some(inner) => inner.serialize(serializer),
            None => serializer.serialize_none(),
        }
    }
}

/// `PATCH /api/environments/:name` 请求体。
///
/// `Option<Option<T>>` 区分「未提及」与「显式置 null（解绑）」——
/// 这正是 domain merge 的既有语义，词汇化后不得丢失。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvPatchReq {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen: Option<ListenReq>,
    #[serde(
        default,
        with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub rules: Option<Option<String>>,
    #[serde(
        default,
        with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub upstream: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insecure_hosts: Option<Vec<String>>,
    /// MITM 白名单（空/缺省 = 全部解密）。与 `insecure_hosts` 同款：整体替换。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mitm_hosts: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_password: Option<String>,
}

/// `POST /api/proxies` 请求体（put 语义：同名覆盖）。凭据只进不出。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyPutReq {
    pub name: String,
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

/// `POST /api/rules` 请求体（包装对象，非裸正文）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulesImportReq {
    pub name: String,
    #[serde(default)]
    pub text: String,
}

/// `POST /api/debug` 请求体。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebugStartReq {
    pub env: String,
}

// --------------------------------------------------------------------------- #
// 游标：推送流断点续传的统一词汇
// --------------------------------------------------------------------------- #

/// 推送流游标。**wire 上是裸 u64**；这个枚举是服务端/客户端侧的类型化词汇，
/// 用来把「游标属于哪条流」显式化 —— 三条流的游标域不同，混用即错：
///
/// * [`Cursor::Byte`] —— 轨迹流：`trajectories/<env>.jsonl` 的字节偏移
///   （窗口化日志，seq 每实例会话重启，字节偏移才是稳定坐标）；
/// * [`Cursor::Request`] —— 调试实时流：抓包记录的 `request_id`
///   （淘汰只移除游标之前的记录，增量无缺口）；
/// * [`Cursor::Generation`] —— 快照流：控制面账本代次（每次控制面写入 +1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    Byte(u64),
    Request(u64),
    Generation(u64),
}

impl Cursor {
    /// wire 值（SSE 帧与 `?cursor=` 查询参数里的那个数字）。
    pub fn value(self) -> u64 {
        match self {
            Cursor::Byte(value) | Cursor::Request(value) | Cursor::Generation(value) => value,
        }
    }

    /// 游标域的稳定名（诊断与协议文档对账用）。
    pub fn kind(self) -> &'static str {
        match self {
            Cursor::Byte(_) => "byte",
            Cursor::Request(_) => "request",
            Cursor::Generation(_) => "generation",
        }
    }
}

// --------------------------------------------------------------------------- #
// 错误信封与帧形状
// --------------------------------------------------------------------------- #

/// 统一错误体：`{error: {code, message, field?}}`。错误码契约见 `envboard-engine::api::error` 模块文档。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

/// 统一错误帧（SSE `error` 事件的 data）。
/// 快照流帧（SSE `snapshot` 事件的 data）：全环境视图 + 账本代次。
/// `cursor` 是**advisory**（快照版本号）：健康变化不经过控制面写入，
/// 同代次的两帧内容仍可能不同，UI 不得据此跳过渲染。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotFrame {
    pub ok: bool,
    pub cursor: u64,
    pub environments: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

/// 调试实时流的 snapshot 帧（整幅替换）：无会话时 `env`/`capture` 均为 null。
/// `cursor` = 会话内最新 `request_id`（无会话/空会话为 0）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DebugSnapshot {
    pub cursor: u64,
    pub env: Option<String>,
    pub capture: Option<CaptureView>,
}

/// 调试实时流的 events 帧（增量）：游标域是 [`Cursor::Request`]。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DebugEvents {
    pub cursor: u64,
    pub records: Vec<Value>,
    pub captured: u64,
    pub dropped: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// EnvView 的 wire 形状与历史 JSON 契约逐键一致（前端按这些键渲染）。
    #[test]
    fn env_view_serializes_to_the_historical_shape() {
        let view = EnvView {
            name: "dev".into(),
            listen: Endpoint {
                host: "127.0.0.1".into(),
                port: 9000,
            },
            rules: Some("stage".into()),
            upstream: None,
            insecure_hosts: vec!["api.example.com".into()],
            mitm_hosts: vec!["api.example.com".into()],
            description: "开发".into(),
            desired: Desired::Running,
            health: "running".into(),
            health_reason: Some("config applied".into()),
            rules_count: 3,
            rules_missing: false,
            proxy_command: "export https_proxy=http://127.0.0.1:9000".into(),
            proxy_auth_enabled: false,
            capture: true,
        };
        let value = serde_json::to_value(&view).unwrap();
        for key in [
            "name",
            "listen",
            "rules",
            "upstream",
            "insecure_hosts",
            "mitm_hosts",
            "description",
            "desired",
            "health",
            "health_reason",
            "rules_count",
            "rules_missing",
            "proxy_command",
            "proxy_auth_enabled",
            "capture",
        ] {
            assert!(value.get(key).is_some(), "缺键 {key}");
        }
        assert_eq!(value["listen"]["port"], 9000);
        assert_eq!(value["desired"], "running");
    }

    /// 无会话的调试 snapshot 必须序列化出 `"env": null` —— 前端以 null 判空态。
    #[test]
    fn idle_debug_snapshot_keeps_explicit_nulls() {
        let value = serde_json::to_value(DebugSnapshot {
            cursor: 0,
            env: None,
            capture: None,
        })
        .unwrap();
        assert!(value["env"].is_null());
        assert!(value["capture"].is_null());
        assert_eq!(value["cursor"], 0);
    }

    #[test]
    fn cursor_kinds_are_stable() {
        assert_eq!(Cursor::Byte(7).kind(), "byte");
        assert_eq!(Cursor::Request(7).kind(), "request");
        assert_eq!(Cursor::Generation(7).kind(), "generation");
        assert_eq!(Cursor::Byte(7).value(), 7);
    }

    /// create 请求体：最小形状（只有 name）与完整形状都能解析，序列化省略缺省键。
    #[test]
    fn env_create_req_round_trips_minimal_and_full() {
        let minimal: EnvCreateReq = serde_json::from_str(r#"{"name":"dev"}"#).unwrap();
        assert_eq!(minimal.listen, None);
        let encoded = serde_json::to_value(&minimal).unwrap();
        assert!(
            encoded.get("listen").is_none(),
            "缺省键不得序列化出来：{encoded}"
        );

        let full: EnvCreateReq = serde_json::from_str(
            r#"{"name":"dev","listen":{"host":"127.0.0.1","port":16000},
               "rules":"beta","upstream":null,"description":"开发",
               "insecure_hosts":["api.example.com"],"mitm_hosts":["api.example.com"],
               "capture":true,
               "proxy_user":"u","proxy_password":"p"}"#,
        )
        .unwrap();
        assert_eq!(full.listen.as_ref().unwrap().port, Some(16000));
        assert_eq!(full.upstream, None);
    }

    /// 未知字段响亮拒绝（与 domain KNOWN_FIELDS 契约同向）。
    #[test]
    fn request_dtos_reject_unknown_fields() {
        let error = serde_json::from_str::<EnvCreateReq>(r#"{"name":"dev","nope":1}"#).unwrap_err();
        assert!(error.to_string().contains("nope"), "{error}");
        let error =
            serde_json::from_str::<ProxyPutReq>(r#"{"name":"corp","host":"h","strict":true}"#)
                .unwrap_err();
        assert!(error.to_string().contains("strict"), "{error}");
    }

    /// patch 的「未提及 / 显式 null / 给值」三态必须可区分 —— merge 语义的生命线。
    #[test]
    fn env_patch_distinguishes_absent_null_and_value() {
        let absent: EnvPatchReq = serde_json::from_str("{}").unwrap();
        assert_eq!(absent.rules, None);
        let cleared: EnvPatchReq = serde_json::from_str(r#"{"rules":null}"#).unwrap();
        assert_eq!(cleared.rules, Some(None));
        let bound: EnvPatchReq = serde_json::from_str(r#"{"rules":"beta"}"#).unwrap();
        assert_eq!(bound.rules, Some(Some("beta".into())));
    }
}

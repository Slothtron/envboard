//! v3 引擎接缝 —— 编排层（manager）只认识这里的类型，不认识任何具体实现。
//!
//! 与 v2 抽象（同目录 proxy 模块）的根本差别：**实例在进程内**。
//! 因此这里没有 PID、没有状态文件、没有 argv 凭据通道、没有收敛窗口 ——
//! 健康判定是内存读，热更新是同步换快照，退出原因就在报告里。
//! v2 的 ProxyCore 面已随 v3 重构整体退场：这里就是编排层认识的全部。

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::ports::LineWriter;
use crate::proxy::{CoreInfo, Listen};
use crate::sha256;
use crate::{CaptureDelta, CaptureView};

/// 启动/热更一个引擎实例所需的全部输入（一等字段，没有透传通道）。
#[derive(Clone)]
pub struct EngineSpec {
    pub listen: Listen,
    /// 已归一化的完整域名清单（空 = 全部严格校验）。
    pub insecure_hosts: Vec<String>,
    pub proxy_user: Option<String>,
    pub proxy_password: Option<String>,
    /// 上游二级代理绑定（None = 直连）。凭据由管理面校验后下发。
    pub upstream: Option<UpstreamSpec>,
    /// hosts 风格规则文本；None = 不带规则。
    pub rules_text: Option<String>,
    /// 规则文本的来源文件名（仅展示：日志与状态视图里说明规则绑到哪份账本）。
    pub rules_source: Option<PathBuf>,
    /// 日志线出口。None = 丢弃。
    pub log: Option<Arc<dyn LineWriter>>,
    /// 请求轨迹出口（trajectories/<env>.jsonl）。None = 不记轨迹。
    pub trajectory: Option<Arc<dyn LineWriter>>,
    /// 抓包开关（运行期观测旋钮；不进 config_hash —— 与 log 同一性质）。
    pub capture: bool,
    /// 抓包缓冲字节预算（0 = 引擎默认 256 MiB）。
    pub capture_budget: usize,
}

/// 上游二级代理的出向定义：先连它（CONNECT / absolute-URI），再由它转达目标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamSpec {
    pub host: String,
    pub port: u16,
    /// both-or-neither 由管理面校验；引擎只按"都有才算有"处理。
    pub user: Option<String>,
    pub password: Option<String>,
}

impl UpstreamSpec {
    pub fn has_auth(&self) -> bool {
        self.user.is_some() && self.password.is_some()
    }
}

impl EngineSpec {
    /// 稳定序列化形状（config_hash 的输入；log 与 rules_source 不参与 ——
    /// 前者不是配置，后者只是展示）。
    pub fn hashable_json(&self) -> String {
        #[derive(Serialize)]
        struct UpstreamShape<'a> {
            host: &'a str,
            port: u16,
            user: Option<&'a str>,
            secret_digest: Option<String>,
        }
        #[derive(Serialize)]
        struct Shape<'a> {
            listen: &'a Listen,
            insecure_hosts: &'a [String],
            user: Option<&'a str>,
            secret_digest: Option<String>,
            upstream: Option<UpstreamShape<'a>>,
            rules: Option<&'a str>,
        }
        let shape = Shape {
            listen: &self.listen,
            insecure_hosts: &self.insecure_hosts,
            user: self.proxy_user.as_deref(),
            secret_digest: self
                .proxy_password
                .as_deref()
                .map(|p| sha256::hex(p.as_bytes())),
            upstream: self.upstream.as_ref().map(|upstream| UpstreamShape {
                host: &upstream.host,
                port: upstream.port,
                user: upstream.user.as_deref(),
                secret_digest: upstream
                    .password
                    .as_deref()
                    .map(|p| sha256::hex(p.as_bytes())),
            }),
            rules: self.rules_text.as_deref(),
        };
        serde_json::to_string(&shape).unwrap_or_default()
    }
}

/// 实例状态（v3 健康表的代码面；契约文案在迁移阶段与 core/spec 对齐）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum InstanceState {
    Starting,
    Running,
    Stopped,
    Unhealthy {
        reason: String,
    },
    /// 端口被占。**不换端口**（契约），等一次显式动作。
    PortConflict {
        port: u16,
    },
    /// 线程终止；reason 含摘要。
    Failed {
        reason: String,
    },
}

impl InstanceState {
    pub fn as_str(&self) -> &'static str {
        match self {
            InstanceState::Starting => "starting",
            InstanceState::Running => "running",
            InstanceState::Stopped => "stopped",
            InstanceState::Unhealthy { .. } => "unhealthy",
            InstanceState::PortConflict { .. } => "port_conflict",
            InstanceState::Failed { .. } => "failed",
        }
    }
    pub fn is_running(&self) -> bool {
        matches!(self, InstanceState::Running)
    }
    /// 附带原因的状态返回原因文本（视图的 health_reason）。
    pub fn reason(&self) -> Option<&str> {
        match self {
            InstanceState::Unhealthy { reason } | InstanceState::Failed { reason } => Some(reason),
            _ => None,
        }
    }
}

/// 内存健康报告 —— 取代 v2"状态文件 + 探活 + 收敛窗"的整条证据链。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineReport {
    pub state: InstanceState,
    /// 当前生效配置的摘要（与 apply 的返回值一致 = 已落地）。
    pub config_hash: String,
    /// 单调递增的装配代次（回执确认用；概念沿用 v2 的 config_epoch）。
    pub epoch: u64,
    pub last_error: Option<String>,
    /// 日志总线累计丢弃行数（数据面永不阻塞的代价必须可见）。
    pub log_drops: u64,
    /// 轨迹总线累计丢弃行数（与 log_drops 同一有界纪律）。
    pub trajectory_drops: u64,
}

/// 运行中的引擎实例句柄。无进程身份：实例不是进程。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineHandle {
    pub env: String,
    pub listen: Listen,
}

/// 可被管理面编排的 v3 引擎后端（真引擎与 fake 共用这一个面）。
#[async_trait]
pub trait ProxyEngine: Send + Sync {
    fn describe(&self) -> CoreInfo;

    /// 拉起实例。返回成功 = 实例已登记（状态可能仍是 starting）；
    /// 同名实例已存在 = Conflict，**不**隐式替换。
    async fn start(&self, env: String, spec: EngineSpec) -> Result<EngineHandle, Error>;

    /// 热更新：通过 = 新快照已生效并返回新 hash；失败 = 旧快照继续服务。
    /// 同步签名是刻意的：装配即生效没有异步语义，也让管理面的同步入口
    /// （update/import_rules）能直接调用，不必 runtime 套 runtime。
    fn apply(&self, handle: &EngineHandle, spec: EngineSpec) -> Result<String, Error>;

    /// 优雅停止（幂等；未知句柄 = NotFound）。
    async fn stop(&self, handle: &EngineHandle) -> Result<(), Error>;

    /// 内存读健康（同步：视图层因此不需要 await 引擎）。
    fn report(&self, handle: &EngineHandle) -> EngineReport;

    /// 抓包会话视图（易失：会话 = 实例生命周期）。None = 实例不在跑。
    /// 默认 None：不支持抓包的实现不必假装。
    fn capture_view(&self, _env: &str, _limit: usize) -> Option<CaptureView> {
        None
    }

    /// 抓包增量（调试实时流用）：游标之后的新记录 + 会话/计数。None = 实例不在跑。
    fn capture_delta(&self, _env: &str, _after: u64, _limit: usize) -> Option<CaptureDelta> {
        None
    }

    /// 单条抓包详情（全缓冲查找；后写优先）。None = 记录不在缓冲或实例不在跑。
    fn capture_get(&self, _env: &str, _request_id: u64) -> Option<serde_json::Value> {
        None
    }

    /// 手动清空抓包会话（generation +1，会话延续）。false = 实例不在跑。
    fn clear_capture(&self, _env: &str) -> bool {
        false
    }

    /// 故障注入旋钮（live 断言与集成测试用）：
    /// 把已登记实例置为 failed 并释放监听 —— 与"引擎线程 panic 后运行时散掉、
    /// socket 关闭"同构。返回 false = 该实现不支持注入（调用方不得假装成功）。
    /// 生产路径没有任何入口调它；它存在的唯一目的是让"实例崩溃可见、可回收、
    /// 可重拉"这条契约可以在真进程里被断言。
    fn inject_failed(&self, _env: &str, _reason: &str) -> bool {
        false
    }
}

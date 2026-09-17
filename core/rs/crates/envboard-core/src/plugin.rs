//! 插件阶段管道、能力注册表与装配期校验。
//!
//! 三层能力模型在这里落地：**内核能力**（监听、鉴权门、TLS 策略、MITM CA、
//! 协议面）是引擎原生快路径，注册表里登记为只读条目但**不进插件链**；
//! **内置插件**（hosts-rules、request-log）走与扩展插件完全相同的接口 ——
//! 接口被产品功能自举验证；**扩展插件**（请求/响应改写、mock、二级代理）
//! 沿同一接缝增量接入。
//!
//! 规则（与 \`core/spec/capabilities.md\` 的「v3 插件与能力注册表」一致）：
//!
//! * 阶段写死：connect → request_head → request_body →（上游）→ response_head
//!   → response_body → log；插件按需覆写所在阶段的子集。
//! * 插件序 = 配置声明序；**依赖约束 > 声明序**（同层同依赖内保持声明序）。
//! * 注册表**静态只读**：装配期查依赖，热路径只走装配后的扁平链 ——
//!   注册表永不参与请求路径，避免与 ArcSwap 快照形成第二真相。
//! * 装配期校验：id 未注册、内核能力冒充插件、缺依赖、环 → \`invalid_config\`
//!   （消息给出依赖名与插件名）；**响亮失败，禁止静默等待**（明确不采纳
//!   "永久 PENDING"那类语义）。
//! * 错误契约：connect/改写钩子 Err → fail-closed 502（消息带插件名）；
//!   显式声明 \`Bypass\` 的插件跳过并强制 WARN + 计数；每钩子超时按 Err；
//!   \`on_log\` 永不外溢（非 async、返回 ()，实现必须自吞错误）。

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

use crate::target::ConnectTarget;
use envboard_core_api::{Error, ErrorCode};

/// 能力的层。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Kernel,
    Builtin,
    Extension,
}

impl Layer {
    pub fn name(self) -> &'static str {
        match self {
            Layer::Kernel => "kernel",
            Layer::Builtin => "builtin",
            Layer::Extension => "extension",
        }
    }
}

/// 能力参与的阶段（注册表元数据；链上执行序由阶段模型写死）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Startup,
    Connect,
    Request,
    Response,
    Log,
}

/// 静态能力/插件清单条目。**phase 是登记性元数据**（文档与门禁可读）；
/// 链上每员都过全部钩子，"参与哪些阶段"由插件覆写哪些钩子决定（默认实现即无操作）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityDescriptor {
    pub id: &'static str,
    pub layer: Layer,
    /// 仅跨插件/跨阶段的依赖（crate 级依赖由 \`deps\` 门禁覆盖，不在此登记）。
    pub depends_on: &'static [&'static str],
    pub phase: Phase,
}

/// 全量注册表。新条目必须同时出现在 \`core/spec/capabilities.md\` 的
/// 「v3 插件与能力注册表」表里 —— 一致性由工程门禁判定。
pub static CAPABILITIES: &[CapabilityDescriptor] = &[
    CapabilityDescriptor {
        id: "kernel:listen",
        layer: Layer::Kernel,
        depends_on: &[],
        phase: Phase::Startup,
    },
    CapabilityDescriptor {
        id: "kernel:proxy-auth",
        layer: Layer::Kernel,
        depends_on: &[],
        phase: Phase::Startup,
    },
    CapabilityDescriptor {
        id: "kernel:tls-policy",
        layer: Layer::Kernel,
        depends_on: &[],
        phase: Phase::Connect,
    },
    CapabilityDescriptor {
        id: "kernel:mitm-ca",
        layer: Layer::Kernel,
        depends_on: &[],
        phase: Phase::Startup,
    },
    CapabilityDescriptor {
        id: "kernel:protocol",
        layer: Layer::Kernel,
        depends_on: &[],
        phase: Phase::Request,
    },
    CapabilityDescriptor {
        id: "hosts-rules",
        layer: Layer::Builtin,
        depends_on: &[],
        phase: Phase::Connect,
    },
    CapabilityDescriptor {
        id: "request-log",
        layer: Layer::Builtin,
        depends_on: &[],
        phase: Phase::Log,
    },
    // 测试/故障注入旋钮（对齐 envboard-core-fake 先例）：注册它，是为了让注入路径
    // 走真实的注册表校验与装配语义，而不是旁路后门。产品装配链里它永远不出现。
    CapabilityDescriptor {
        id: "debug-inject",
        layer: Layer::Extension,
        depends_on: &[],
        phase: Phase::Request,
    },
];

pub fn descriptor(id: &str) -> Option<&'static CapabilityDescriptor> {
    CAPABILITIES.iter().find(|entry| entry.id == id)
}

/// 钩子错误：detail 面向日志与 502 正文；插件名由引擎补上（引擎知道是谁）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginError {
    pub detail: String,
}

impl PluginError {
    pub fn new(detail: impl Into<String>) -> Self {
        PluginError {
            detail: detail.into(),
        }
    }
}

/// 短路语义：mock/早答类插件在 head 阶段直接给出响应（跳过上游）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Early(PlannedResponse),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// request_head 的只读视图。
#[derive(Debug)]
pub struct RequestView<'a> {
    pub target: &'a ConnectTarget,
    pub method: &'a str,
    pub path: &'a str,
    pub headers: &'a [(String, String)],
    pub client_sni: Option<&'a str>,
}

impl RequestView<'_> {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// response_head 的可修订视图。
#[derive(Debug)]
pub struct ResponseView<'a> {
    pub status: u16,
    pub headers: &'a mut Vec<(String, String)>,
    pub method: &'a str,
    pub authority: &'a str,
}

/// 终局记录：log 阶段唯一的输入。字段是"这一请求的全部可见事实"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord {
    pub timestamp_unix_ms: u64,
    pub method: String,
    pub authority: String,
    pub path: String,
    pub status: u16,
    pub request_bytes: usize,
    pub response_bytes: usize,
    pub duration_ms: u64,
    /// 连接目标被插件链改写（resolved_addr 从 None 变为 Some）。
    pub rewritten: bool,
    /// 上游 TLS 是严格校验还是名单放宽。
    pub insecure: bool,
    /// 请求以引擎错误告终时的原因。
    pub error: Option<String>,
}

/// 插件接口。所有钩子默认无操作；覆写自己参与的阶段即可。
///
/// \`async_trait\` 是 AFIT 不支持 dyn 的必然代价（每请求一次 dyn 分配，[推断]
/// 单机流量下不可测量；内置插件热路径的静态化是后续优化位）。
#[async_trait]
pub trait Plugin: Send + Sync {
    /// 注册表 id（Builtin/Extension 层）。内核 id 出现在链里 = invalid_config。
    fn id(&self) -> &'static str;

    /// 修订建连描述符（改 resolved_addr / 填 chained_proxy / 读策略后拒绝）。
    async fn on_connect(&self, _target: &mut ConnectTarget) -> Result<(), PluginError> {
        Ok(())
    }

    async fn on_request_head(&self, _cx: &RequestView<'_>) -> Result<Flow, PluginError> {
        Ok(Flow::Continue)
    }

    /// 缓冲体上的改写（ADR-4：默认缓冲让这里有完整视图）。
    async fn on_request_body(&self, _body: &mut Vec<u8>) -> Result<(), PluginError> {
        Ok(())
    }

    async fn on_response_head(&self, _cx: &mut ResponseView<'_>) -> Result<Flow, PluginError> {
        Ok(Flow::Continue)
    }

    async fn on_response_body(&self, _body: &mut Vec<u8>) -> Result<(), PluginError> {
        Ok(())
    }

    /// 终局：同步、错误永不外溢（契约在类型上：返回 unit）。
    fn on_log(&self, _event: Arc<LogRecord>) {}
}

impl fmt::Debug for dyn Plugin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "plugin:{}", self.id())
    }
}

/// 错误档位：默认 fail-closed；观测类插件显式声明 Bypass。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnError {
    FailClosed,
    Bypass,
}

#[derive(Debug)]
pub struct ChainEntry {
    pub plugin: Arc<dyn Plugin>,
    pub on_error: OnError,
    pub descriptor: &'static CapabilityDescriptor,
}

/// 装配后的不可变插件链（随环境快照整体 ArcSwap）。
#[derive(Debug)]
pub struct PluginSet {
    pub entries: Vec<ChainEntry>,
    /// 逐插件 bypass 计数（与 entries 下标对齐）。
    pub bypass_counts: Vec<std::sync::atomic::AtomicU64>,
}

impl PluginSet {
    /// 某插件的 bypass 累计（工作台状态视图用）。
    pub fn bypass_count(&self, id: &str) -> Option<u64> {
        self.entries
            .iter()
            .zip(self.bypass_counts.iter())
            .find(|(entry, _)| entry.descriptor.id == id)
            .map(|(_, count)| count.load(std::sync::atomic::Ordering::Relaxed))
    }
}

/// 装配请求：链上的一个插件实例 + 它的错误档位。声明序 = 数组序。
#[derive(Clone)]
pub struct PluginPlan {
    pub plugin: Arc<dyn Plugin>,
    pub on_error: OnError,
}

/// 用真实注册表装配。
#[must_use = "assembly validates the chain; dropping the result hides invalid config"]
pub fn assemble(plan: Vec<PluginPlan>) -> Result<PluginSet, Error> {
    assemble_with(plan, descriptor)
}

/// 装配期校验 + 依赖拓扑。校验通过后产出 PluginSet（顺序即执行序）。
///
/// 拓扑规则：依赖约束 > 声明序；同层同依赖内保持声明序（稳定的 Kahn 按声明
/// 下标出队）。缺依赖与环都是 invalid_config，消息点名双方。
///
/// lookup 参数化是为了让拓扑机器本身可测（单测注入合成注册表视图；产品路径
/// 永远走真实的 CAPABILITIES）。
pub fn assemble_with(
    plan: Vec<PluginPlan>,
    lookup: impl Fn(&str) -> Option<&'static CapabilityDescriptor>,
) -> Result<PluginSet, Error> {
    let mut descriptors: Vec<&'static CapabilityDescriptor> = Vec::with_capacity(plan.len());
    for entry in &plan {
        let id = entry.plugin.id();
        let Some(descriptor) = lookup(id) else {
            return Err(invalid(format!(
                "plugin {id:?} is not in the capability registry"
            )));
        };
        if descriptor.layer == Layer::Kernel {
            return Err(invalid(format!(
                "kernel capability {id:?} is registered for visibility only and must not appear in a plugin chain"
            )));
        }
        descriptors.push(descriptor);
    }

    // 依赖缺失：点名"谁依赖谁"。Kernel 层依赖天然满足（内核永远在场），只作为
    // 可见性登记；链内序只对非 Kernel 依赖生效。
    for entry_descriptor in &descriptors {
        for need in entry_descriptor.depends_on {
            let entry = lookup(need);
            if entry.is_some_and(|entry| entry.layer == Layer::Kernel) {
                continue;
            }
            let in_chain = descriptors.iter().any(|other| other.id == *need);
            if !in_chain {
                let known = entry
                    .map(|entry| format!("{:?}", entry.layer))
                    .unwrap_or_else(|| "unknown id".to_string());
                return Err(invalid(format!(
                    "plugin {:?} depends on {need:?} which is not in the chain (registry knows it as {known})",
                    entry_descriptor.id
                )));
            }
        }
    }

    // 稳定拓扑：节点保留声明下标；只有当前置全部就绪才出队，保证"依赖约束 >
    // 声明序、同层同依赖按声明序"。
    let mut placed: Vec<bool> = vec![false; plan.len()];
    let mut order: Vec<usize> = Vec::with_capacity(plan.len());
    loop {
        let progressed = {
            let mut picked: Option<usize> = None;
            for index in 0..plan.len() {
                if placed[index] {
                    continue;
                }
                let ready = descriptors[index].depends_on.iter().all(|need| {
                    if lookup(need).is_some_and(|entry| entry.layer == Layer::Kernel) {
                        return true;
                    }
                    descriptors
                        .iter()
                        .position(|entry| entry.id == *need)
                        .is_some_and(|at| placed[at] || at == index)
                });
                if ready {
                    picked = Some(index);
                    break;
                }
            }
            picked
        };
        match progressed {
            Some(index) => {
                placed[index] = true;
                order.push(index);
            }
            None => break,
        }
    }
    if order.len() != plan.len() {
        let stuck: Vec<&str> = descriptors
            .iter()
            .zip(placed.iter())
            .filter(|(_, done)| !**done)
            .map(|(descriptor, _)| descriptor.id)
            .collect();
        return Err(invalid(format!(
            "dependency cycle among plugins: {}",
            stuck.join(" <-> ")
        )));
    }

    let mut slots: Vec<Option<PluginPlan>> = plan.into_iter().map(Some).collect();
    let mut entries: Vec<ChainEntry> = Vec::with_capacity(order.len());
    for index in order {
        let slot = slots[index]
            .take()
            .expect("each plan slot is placed exactly once by the topological order");
        entries.push(ChainEntry {
            plugin: slot.plugin,
            on_error: slot.on_error,
            descriptor: descriptors[index],
        });
    }
    let bypass_counts = entries
        .iter()
        .map(|_| std::sync::atomic::AtomicU64::new(0))
        .collect();
    Ok(PluginSet {
        entries,
        bypass_counts,
    })
}

fn invalid(message: String) -> Error {
    Error::new(ErrorCode::InvalidConfig, message)
}

/// 日志出口就是 core 端口的 LineWriter（契约与 Null 实现收口在
/// envboard-core-api::ports）；这里保留语义别名给插件面使用。
pub use envboard_core_api::ports::{LineWriter as LogWriter, NullLineWriter as NullLogWriter};
// ---- 钩子执行器：阶段过滤、超时、错误档位 ----

use std::time::Duration;
use tokio::time::timeout;

/// connect/改写钩子的单次调用上限；超时按 Err 走错误档位（错误契约）。
pub const CONNECT_HOOK_TIMEOUT: Duration = Duration::from_secs(1);
pub const HEAD_HOOK_TIMEOUT: Duration = Duration::from_secs(1);
pub const BODY_HOOK_TIMEOUT: Duration = Duration::from_secs(5);

/// fail-closed 的产物：带着插件名向外走，502 正文与日志都用它。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookFault {
    pub plugin: &'static str,
    pub reason: String,
}

impl HookFault {
    pub fn to_report(&self) -> String {
        format!("{}: {}", self.plugin, self.reason)
    }
}

enum HookOutcome {
    Passed,
    Failed(String),
}

async fn drive<F>(deadline: Duration, call: F) -> HookOutcome
where
    F: std::future::Future<Output = Result<(), PluginError>>,
{
    match timeout(deadline, call).await {
        Err(_elapsed) => HookOutcome::Failed("hook timed out".to_string()),
        Ok(Ok(())) => HookOutcome::Passed,
        Ok(Err(error)) => HookOutcome::Failed(error.detail),
    }
}

fn settle(
    set: &PluginSet,
    log: &Arc<dyn LogWriter>,
    index: usize,
    reason: String,
) -> Result<(), HookFault> {
    let entry = &set.entries[index];
    match entry.on_error {
        OnError::FailClosed => Err(HookFault {
            plugin: entry.descriptor.id,
            reason,
        }),
        OnError::Bypass => {
            set.bypass_counts[index].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // 允许降级，禁止静默降级：每次 bypass 都留下 WARN 与计数。
            log.write_line(&format!(
                "WARN plugin {} bypassed a failure: {}",
                entry.descriptor.id, reason
            ));
            Ok(())
        }
    }
}

pub async fn run_connect(
    set: &PluginSet,
    log: &Arc<dyn LogWriter>,
    target: &mut ConnectTarget,
) -> Result<(), HookFault> {
    for (index, entry) in set.entries.iter().enumerate() {
        let outcome = drive(CONNECT_HOOK_TIMEOUT, entry.plugin.on_connect(target)).await;
        if let HookOutcome::Failed(reason) = outcome {
            settle(set, log, index, reason)?;
        }
    }
    Ok(())
}

/// request_head：短路语义在这里产出；bypass 只吞错误，不吞别人给的 Early。
pub async fn run_request_head(
    set: &PluginSet,
    log: &Arc<dyn LogWriter>,
    cx: &RequestView<'_>,
) -> Result<Flow, HookFault> {
    for (index, entry) in set.entries.iter().enumerate() {
        let call = match timeout(HEAD_HOOK_TIMEOUT, entry.plugin.on_request_head(cx)).await {
            Err(_) => Err(HookFault::plain(entry, "hook timed out")),
            Ok(Err(error)) => Err(HookFault::plain(entry, &error.detail)),
            Ok(Ok(flow)) => Ok(flow),
        };
        match call {
            Ok(flow) => {
                if matches!(flow, Flow::Early(_)) {
                    return Ok(flow);
                }
            }
            Err(fault) => {
                settle(set, log, index, fault.reason)?;
            }
        }
    }
    Ok(Flow::Continue)
}

pub async fn run_request_body(
    set: &PluginSet,
    log: &Arc<dyn LogWriter>,
    body: &mut Vec<u8>,
) -> Result<(), HookFault> {
    for (index, entry) in set.entries.iter().enumerate() {
        let outcome = drive(BODY_HOOK_TIMEOUT, entry.plugin.on_request_body(body)).await;
        if let HookOutcome::Failed(reason) = outcome {
            settle(set, log, index, reason)?;
        }
    }
    Ok(())
}

/// response_head：返回可能替换掉的响应（Early = 整条响应重写）。
/// 视图逐插件现造：借用在一次钩子调用内闭环，链上任何插件都不持有跨钩子的引用。
pub async fn run_response_head(
    set: &PluginSet,
    log: &Arc<dyn LogWriter>,
    method: &str,
    authority: &str,
    status: u16,
    headers: &mut Vec<(String, String)>,
) -> Result<Option<PlannedResponse>, HookFault> {
    for (index, entry) in set.entries.iter().enumerate() {
        let result = {
            let mut view = ResponseView {
                status,
                headers,
                method,
                authority,
            };
            match timeout(HEAD_HOOK_TIMEOUT, entry.plugin.on_response_head(&mut view)).await {
                Err(_) => Err(PluginError::new("hook timed out")),
                Ok(result) => result,
            }
        };
        match result {
            Ok(Flow::Early(replacement)) => return Ok(Some(replacement)),
            Ok(Flow::Continue) => {}
            Err(error) => settle(set, log, index, error.detail)?,
        }
    }
    Ok(None)
}

pub async fn run_response_body(
    set: &PluginSet,
    log: &Arc<dyn LogWriter>,
    body: &mut Vec<u8>,
) -> Result<(), HookFault> {
    for (index, entry) in set.entries.iter().enumerate() {
        let outcome = drive(BODY_HOOK_TIMEOUT, entry.plugin.on_response_body(body)).await;
        if let HookOutcome::Failed(reason) = outcome {
            settle(set, log, index, reason)?;
        }
    }
    Ok(())
}

/// log 阶段：同步扇出，错误在类型上就不存在（永不外溢）。
pub fn run_log(set: &PluginSet, event: Arc<LogRecord>) {
    for entry in &set.entries {
        entry.plugin.on_log(event.clone());
    }
}

impl HookFault {
    fn plain(entry: &ChainEntry, reason: &str) -> Self {
        HookFault {
            plugin: entry.descriptor.id,
            reason: reason.to_string(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    struct Dummy(&'static str);

    #[async_trait::async_trait]
    impl Plugin for Dummy {
        fn id(&self) -> &'static str {
            self.0
        }
    }

    fn d(
        id: &'static str,
        layer: Layer,
        deps: &'static [&'static str],
    ) -> &'static CapabilityDescriptor {
        Box::leak(Box::new(CapabilityDescriptor {
            id,
            layer,
            depends_on: deps,
            phase: Phase::Connect,
        }))
    }

    fn plan_of(ids: &[&'static str]) -> Vec<PluginPlan> {
        ids.iter()
            .map(|id| PluginPlan {
                plugin: Arc::new(Dummy(id)),
                on_error: OnError::FailClosed,
            })
            .collect()
    }

    #[test]
    fn unregistered_id_is_rejected_by_name() {
        let error = assemble_with(plan_of(&["ghost"]), |id| {
            (id == "hosts-rules").then(|| d("hosts-rules", Layer::Builtin, &[]))
        })
        .unwrap_err();
        assert!(error.message.contains("ghost"), "{}", error.message);
        assert_eq!(error.code, envboard_core_api::ErrorCode::InvalidConfig);
    }

    #[test]
    fn kernel_capability_cannot_masquerade_as_plugin() {
        let error = assemble_with(plan_of(&["kernel:auth"]), |_| {
            Some(d("kernel:auth", Layer::Kernel, &[]))
        })
        .unwrap_err();
        assert!(
            error.message.contains("must not appear"),
            "{}",
            error.message
        );
    }

    #[test]
    fn missing_dependency_names_both_sides() {
        let error = assemble_with(plan_of(&["a"]), |id| match id {
            "a" => Some(d("a", Layer::Builtin, &["b"])),
            "b" => Some(d("b", Layer::Builtin, &[])),
            _ => None,
        })
        .unwrap_err();
        assert!(
            error.message.contains("a") && error.message.contains("b"),
            "{}",
            error.message
        );
    }

    #[test]
    fn dependency_cycle_is_named() {
        let error = assemble_with(plan_of(&["a", "b"]), |id| match id {
            "a" => Some(d("a", Layer::Builtin, &["b"])),
            "b" => Some(d("b", Layer::Builtin, &["a"])),
            _ => None,
        })
        .unwrap_err();
        assert!(error.message.contains("cycle"), "{}", error.message);
    }

    #[test]
    fn dependency_beats_declaration_order() {
        // 声明序 a,b；a 依赖 b → 执行序必须是 b,a（依赖约束 > 声明序）。
        let set = assemble_with(plan_of(&["a", "b"]), |id| match id {
            "a" => Some(d("a", Layer::Builtin, &["b"])),
            "b" => Some(d("b", Layer::Builtin, &[])),
            _ => None,
        })
        .unwrap();
        let ids: Vec<&str> = set.entries.iter().map(|e| e.descriptor.id).collect();
        assert_eq!(ids, vec!["b", "a"]);
    }

    #[test]
    fn equal_dependencies_keep_declaration_order() {
        let set = assemble_with(plan_of(&["x", "y"]), |id| match id {
            "x" => Some(d("x", Layer::Builtin, &[])),
            "y" => Some(d("y", Layer::Builtin, &[])),
            _ => None,
        })
        .unwrap();
        let ids: Vec<&str> = set.entries.iter().map(|e| e.descriptor.id).collect();
        assert_eq!(ids, vec!["x", "y"]);
    }

    #[test]
    fn the_real_registry_boots_the_builtin_pair() {
        let set = assemble(plan_of(&["hosts-rules", "request-log"])).unwrap();
        assert_eq!(set.entries.len(), 2);
        assert_eq!(set.entries[0].descriptor.id, "hosts-rules");
    }
}

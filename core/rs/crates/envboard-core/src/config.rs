//! 引擎配置的编译与快照。
//!
//! 配置只在进入引擎时编译一次：归一化输入 → 规则解析 → 插件装配与校验 →
//! 不可变快照（ArcSwap 换入即生效，数据面每请求取快照）。非法配置在编译期
//! 响亮失败（invalid_config 附字段路径），线上旧快照继续服务 —— v2 的
//! "失败保留旧快照 + config_error"语义在此延续，差别只剩"旧快照"在内存里。
//!
//! 输入假定已由管理面归一化（envboard-domain 是唯一归一化者，引擎不再做
//! 第二份默认值）；引擎只增加"编译"这一步：解析规则文本、校验凭据成对、
//! 装配插件链。

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Serialize;

use envboard_core_api::proxy::Listen;
use envboard_core_api::sha256;
use envboard_core_api::{Error, ErrorCode};
use envboard_rules::parse_hosts_text;

use crate::builtin::{HostsRules, RequestLog};
use crate::http::MAX_BODY_BYTES;
use crate::plugin::{LogWriter, NullLogWriter, OnError, PluginPlan, PluginSet, assemble};

/// 引擎实例的完整输入（环境的外部字段 + 进程内的装配素材）。无环境名 ——
/// 实例不知道自己是哪个环境，这条 v2 纪律保留：环境身份只属于管理面。
#[derive(Clone)]
pub struct EngineConfig {
    pub listen: Listen,
    /// 已归一化的完整域名清单（空 = 全部严格校验）。
    pub insecure_hosts: Vec<String>,
    pub proxy_user: Option<String>,
    pub proxy_password: Option<String>,
    /// hosts 风格规则文本（core/spec/rules.md 的 BNF）；None = 不带规则。
    pub rules_text: Option<String>,
    /// 缓冲体上限的显式值（None = 引擎默认 MAX_BODY_BYTES）。
    pub max_buffered_body: Option<usize>,
    /// 日志出口。request-log 插件与引擎 WARN 共用；None = 丢弃（NullLogWriter）。
    pub log_writer: Option<Arc<dyn LogWriter>>,
    /// 测试/故障注入旋钮（对齐 envboard-core-fake 的先例）：追加到内置链尾。
    /// 产品装配路径永远为空；它的存在让"错误契约、Early 短路、钩子超时"
    /// 可被 hermetic 测试钉住。
    pub extra_plugins: Vec<PluginPlan>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            listen: Listen::localhost(0),
            insecure_hosts: Vec::new(),
            proxy_user: None,
            proxy_password: None,
            rules_text: None,
            max_buffered_body: None,
            log_writer: None,
            extra_plugins: Vec::new(),
        }
    }
}

/// 编译后的不可变快照：ArcSwap 换入的就是它。
pub struct CompiledConfig {
    pub listen: Listen,
    pub insecure_hosts: Vec<String>,
    pub plugins: Arc<PluginSet>,
    pub max_buffered_body: usize,
    pub log_writer: Arc<dyn LogWriter>,
    /// 凭据对；None = 不启用鉴权。secret 只在比对时读取。
    auth: Option<(String, Vec<u8>)>,
    pub config_hash: String,
}

impl CompiledConfig {
    pub fn auth_required(&self) -> bool {
        self.auth.is_some()
    }

    /// 按域名放宽判定：SNI 优先、无 SNI 回退上连地址、SNI 存在未命中不回退。
    /// 判定与 v2 同一份实现（envboard-rules 的 insecure::matches）。
    /// 内核能力：放宽只经这个一等字段，没有任何全局开关。
    pub fn is_insecure(&self, sni: Option<&str>, address: Option<&str>) -> bool {
        envboard_rules::insecure_matches(&self.insecure_hosts, sni, address)
    }

    /// 比对凭据：user 与 secret 都是定长时间比较。
    pub fn check_credentials(&self, user: &str, password: &[u8]) -> bool {
        match &self.auth {
            None => true,
            Some((expected_user, expected)) => {
                sha256::digest(user.as_bytes()) == sha256::digest(expected_user.as_bytes())
                    && constant_time_eq(expected, password)
            }
        }
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut acc = 0u8;
    for (left, right) in a.iter().zip(b) {
        acc |= left ^ right;
    }
    acc == 0
}

/// 编译：校验凭据成对 → 解析规则 → 装配插件链（注册表与依赖校验在内）→
/// config_hash。任何一步失败即 Err，调用方保留旧快照。
pub fn compile(config: &EngineConfig) -> Result<CompiledConfig, Error> {
    let auth = match (&config.proxy_user, &config.proxy_password) {
        (None, None) => None,
        (Some(user), Some(password)) => {
            if user.is_empty() {
                return Err(Error::new(
                    ErrorCode::InvalidConfig,
                    "proxy_user: empty user name is not a credential".to_string(),
                ));
            }
            Some((user.clone(), password.clone().into_bytes()))
        }
        (Some(_), None) => {
            return Err(Error::new(
                ErrorCode::InvalidConfig,
                "proxy_user is set without proxy_password: credentials come in pairs".to_string(),
            ));
        }
        (None, Some(_)) => {
            return Err(Error::new(
                ErrorCode::InvalidConfig,
                "proxy_password is set without proxy_user: credentials come in pairs".to_string(),
            ));
        }
    };

    let rules = match &config.rules_text {
        Some(text) => parse_hosts_text(text).entries,
        None => BTreeMap::new(),
    };

    let max_buffered_body = config.max_buffered_body.unwrap_or(MAX_BODY_BYTES);
    if max_buffered_body == 0 {
        return Err(Error::new(
            ErrorCode::InvalidConfig,
            "max_buffered_body must be positive; drop-to-zero buffering is not a mode".to_string(),
        ));
    }
    let log_writer = config
        .log_writer
        .clone()
        .unwrap_or_else(|| Arc::new(NullLogWriter));

    // 内置链（两员固定，声明序即执行序基线）+ 注入的测试插件。
    // hosts-rules 默认 fail-closed：规则脏数据宁停不猜；request-log 显式 Bypass：
    // 观测失败不得阻断流量 —— 错误契约的两档在装配时各有真实用户。
    let mut plan = vec![
        PluginPlan {
            plugin: Arc::new(HostsRules::from_table(rules.clone())),
            on_error: OnError::FailClosed,
        },
        PluginPlan {
            plugin: Arc::new(RequestLog::new(log_writer.clone())),
            on_error: OnError::Bypass,
        },
    ];
    plan.extend(config.extra_plugins.iter().cloned());
    let plugins = Arc::new(assemble(plan)?);

    #[derive(Serialize)]
    struct HashShape<'a> {
        listen: &'a Listen,
        insecure_hosts: &'a [String],
        rules: &'a BTreeMap<String, String>,
        max_buffered_body: usize,
        chain: Vec<(&'a str, String, &'a str)>,
        auth: Option<(&'a str, String)>,
    }
    let chain: Vec<(&str, String, &str)> = plugins
        .entries
        .iter()
        .map(|entry| {
            (
                entry.descriptor.id,
                format!("{:?}", entry.on_error),
                entry.descriptor.layer.name(),
            )
        })
        .collect();
    let shape = HashShape {
        listen: &config.listen,
        insecure_hosts: &config.insecure_hosts,
        rules: &rules,
        max_buffered_body,
        chain,
        auth: auth
            .as_ref()
            .map(|(user, secret)| (user.as_str(), sha256::hex(secret))),
    };
    let canonical = serde_json::to_vec(&shape).map_err(|e| {
        Error::new(
            ErrorCode::InternalError,
            format!("config is not serializable: {e}"),
        )
    })?;
    let config_hash = sha256::hex(&canonical);

    Ok(CompiledConfig {
        listen: config.listen,
        insecure_hosts: config.insecure_hosts.clone(),
        plugins,
        max_buffered_body,
        log_writer,
        auth,
        config_hash,
    })
}

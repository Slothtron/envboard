//! 引擎配置的编译与快照。
//!
//! 配置只在进入引擎时编译一次（规则文本 → 查找表），数据面每请求拿
//! \`ArcSwap\` 的全量快照 —— 装配即生效，没有文件通道与轮询。非法配置在编译期
//! 响亮失败（invalid_config 附字段路径），旧快照继续服务。
//!
//! 输入假定已由管理面归一化（\`envboard-domain\` 是唯一归一化者，引擎不再做
//! 第二份默认值）；引擎只增加"编译"这一步：解析规则文本、校验凭据成对。

use std::collections::BTreeMap;

use serde::Serialize;

use envboard_core_api::proxy::Listen;
use envboard_core_api::sha256;
use envboard_core_api::{Error, ErrorCode};
use envboard_rules::{insecure_matches, normalize_host, parse_hosts_text};

/// 引擎实例的完整输入（环境的外部字段，无环境名 —— 实例不知道自己是哪个环境，
/// 这条 v2 纪律保留：环境身份只属于管理面）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineConfig {
    pub listen: Listen,
    /// 已归一化的完整域名清单（空 = 全部严格校验）。
    pub insecure_hosts: Vec<String>,
    pub proxy_user: Option<String>,
    pub proxy_password: Option<String>,
    /// hosts 风格规则文本（\`core/spec/rules.md\` 的 BNF）；None = 不带规则。
    pub rules_text: Option<String>,
}

/// 编译后的不可变快照：ArcSwap 换入的就是它。
#[derive(Debug, Clone)]
pub struct CompiledConfig {
    pub listen: Listen,
    pub insecure_hosts: Vec<String>,
    /// 归一化 host → 目标 ip 文本（后出现者胜，由规则解析器裁定）。
    pub rules: BTreeMap<String, String>,
    /// 凭据对；None = 不启用鉴权。user 参与展示，secret 只在比对时读取。
    auth: Option<(String, Vec<u8>)>,
    pub config_hash: String,
}

impl CompiledConfig {
    pub fn auth_required(&self) -> bool {
        self.auth.is_some()
    }

    /// 规则查找：host 归一化后精确命中（与注入器同源：小写、去尾点）。
    pub fn target_for(&self, host: &str) -> Option<&str> {
        self.rules.get(&normalize_host(host)).map(String::as_str)
    }

    /// 按域名放宽判定：SNI 优先、无 SNI 回退上连地址、SNI 存在未命中不回退。
    /// 判定函数与 v2 同一份实现（\`envboard-rules::insecure::matches\`）。
    pub fn is_insecure(&self, sni: Option<&str>, address: Option<&str>) -> bool {
        insecure_matches(&self.insecure_hosts, sni, address)
    }
}

/// 编译：解析规则、校验凭据成对、算 config_hash。失败即 Err，无静默。
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

    // config_hash 覆盖"生效内容"本身：鉴权只进 secret 的摘要，明文不落任何记录。
    #[derive(Serialize)]
    struct HashShape<'a> {
        listen: &'a Listen,
        insecure_hosts: &'a [String],
        rules: &'a BTreeMap<String, String>,
        auth: Option<(&'a str, String)>,
    }
    let shape = HashShape {
        listen: &config.listen,
        insecure_hosts: &config.insecure_hosts,
        rules: &rules,
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

    let (auth_user, auth_secret) = match auth {
        Some((user, secret)) => (Some(user), Some(secret)),
        None => (None, None),
    };
    Ok(CompiledConfig {
        listen: config.listen,
        insecure_hosts: config.insecure_hosts.clone(),
        rules,
        auth: auth_user.zip(auth_secret),
        config_hash,
    })
}

impl CompiledConfig {
    /// 比对凭据：user 与 secret 都走定长时间比较（见 auth 模块）。
    pub fn check_credentials(&self, user: &str, password: &[u8]) -> bool {
        match &self.auth {
            None => true,
            Some((expected_user, expected)) => {
                envboard_core_api::sha256::digest(user.as_bytes())
                    == sha256::digest(expected_user.as_bytes())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn base() -> EngineConfig {
        EngineConfig {
            listen: Listen::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 16_401),
            insecure_hosts: Vec::new(),
            proxy_user: None,
            proxy_password: None,
            rules_text: None,
        }
    }

    use std::net::IpAddr;

    #[test]
    fn credentials_must_come_in_pairs() {
        let mut config = base();
        config.proxy_user = Some("u".into());
        let error = compile(&config).expect_err("half a credential pair must fail");
        assert!(error.message.contains("pairs"), "{}", error.message);
    }

    #[test]
    fn hash_tracks_every_effective_field() {
        let first = compile(&base()).unwrap();
        let mut config = base();
        config.rules_text = Some("10.0.0.1 svc.a\n".into());
        let second = compile(&config).unwrap();
        assert_ne!(first.config_hash, second.config_hash);
        config.insecure_hosts = vec!["svc.b".into()];
        let third = compile(&config).unwrap();
        assert_ne!(second.config_hash, third.config_hash);
        config.proxy_password = Some("p".into());
        config.proxy_user = Some("u".into());
        let fourth = compile(&config).unwrap();
        assert_ne!(third.config_hash, fourth.config_hash);
        // 同内容重编译 → 同哈希（幂等）。
        let again = compile(&config).unwrap();
        assert_eq!(fourth.config_hash, again.config_hash);
    }

    #[test]
    fn rule_lookup_normalizes_like_the_injector() {
        let mut config = base();
        config.rules_text = Some("10.0.0.1 Svc.A.Example\n".into());
        let compiled = compile(&config).unwrap();
        assert_eq!(compiled.target_for("svc.a.example."), Some("10.0.0.1"));
        assert_eq!(compiled.target_for("other.example"), None);
    }
}

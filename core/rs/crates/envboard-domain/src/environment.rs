//! 环境（`Environment`）领域模型 —— 纯逻辑，不碰 fs / 网络 / 宿主。
//!
//! 契约见 `core/spec/capabilities.md`（§领域不变量）。v2 把 v1 的 8 字段收成 4 个：
//! `name` / `listen` / `rules` / `description`。被删除的字段（`dns_servers`、
//! `hosts`、`domain_suffix`、`color`…）现在会**响亮失败**，不会被当成无用字段收下。

use std::net::IpAddr;

use envboard_core_api::{DEFAULT_LISTEN_HOST, Error, Listen};
use serde_json::{Map, Value};

/// 对外 JSON 形状里的字段名（未知字段判定用同一张表）。
pub const KNOWN_FIELDS: &[&str] = &["name", "listen", "rules", "description"];

/// 契约里的路径前缀 —— 所有 `field` 都从它开始。
pub const PATH: &str = "environment";

/// 环境名上限：`^[a-z][a-z0-9_-]{0,31}$`。
pub const NAME_MAX_LEN: usize = 32;

/// `description` 上限（字符数），见 `core/spec/capabilities.md`。
pub const DESCRIPTION_MAX_CHARS: usize = 200;

/// 一个环境 = 一个监听端口 + 一份可选的规则绑定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    name: String,
    listen: Listen,
    rules: Option<String>,
    description: String,
}

impl Environment {
    pub fn new(
        name: impl Into<String>,
        listen: Listen,
        rules: Option<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            listen,
            rules,
            description: description.into(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn listen(&self) -> Listen {
        self.listen
    }

    pub fn rules(&self) -> Option<&str> {
        self.rules.as_deref()
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    /// 归一化后的完整记录 —— 可直接持久化，也是契约 fixture 比对的形状。
    pub fn to_json(&self) -> Value {
        let mut listen = Map::new();
        listen.insert("host".into(), Value::String(self.listen.host.to_string()));
        listen.insert("port".into(), Value::from(self.listen.port));

        let mut out = Map::new();
        out.insert("name".into(), Value::String(self.name.clone()));
        out.insert("listen".into(), Value::Object(listen));
        out.insert(
            "rules".into(),
            match &self.rules {
                Some(name) => Value::String(name.clone()),
                None => Value::Null,
            },
        );
        out.insert(
            "description".into(),
            Value::String(self.description.clone()),
        );
        Value::Object(out)
    }

    /// 校验并归一化一份环境定义。
    ///
    /// 手写而不是用 `#[derive(Deserialize)]` + `deny_unknown_fields`，是因为契约要求
    /// 错误里带**精确的点分字段路径**（`environment.listen.port`），而 serde 的
    /// 路径信息不足以拼出它。顺序是固定的，测试按顺序断言。
    pub fn from_json(value: &Value) -> Result<Self, Error> {
        let object = value.as_object().ok_or_else(|| {
            Error::invalid_config(PATH, format!("{PATH} must be an object, got {value}"))
        })?;

        for key in object.keys() {
            if !KNOWN_FIELDS.contains(&key.as_str()) {
                return Err(Error::invalid_config(
                    format!("{PATH}.{key}"),
                    format!(
                        "unknown field {key:?}: the v2 model has only {KNOWN_FIELDS:?} \
                         (fields removed in v2 are rejected loudly, not ignored)"
                    ),
                ));
            }
        }

        let name = normalize_name(&require_string(object, "name", &format!("{PATH}.name"))?);
        validate_name(&name)?;

        let (host, port) = parse_listen(object.get("listen"))?;
        let rules = normalize_rules(object.get("rules"))?;
        let description = parse_description(object.get("description"))?;

        Ok(Self {
            name,
            listen: Listen::new(host, port),
            rules,
            description,
        })
    }

    /// PATCH 语义：未提及的字段保持不变，**显式 `null` = 清空**，合并后**重新校验**。
    ///
    /// `listen` 是整体替换而不是深合并 —— host 与 port 是同一个身份的两个部分，
    /// 深合并会让"只改 host"意外保留旧端口。
    pub fn merged(&self, patch: &Value) -> Result<Self, Error> {
        let patch = patch.as_object().ok_or_else(|| {
            Error::invalid_config(PATH, format!("patch must be an object, got {patch}"))
        })?;

        let mut merged = self.to_json().as_object().cloned().unwrap_or_default();
        for (key, value) in patch {
            if !KNOWN_FIELDS.contains(&key.as_str()) {
                return Err(Error::invalid_config(
                    format!("{PATH}.{key}"),
                    format!("unknown field {key:?} in patch"),
                ));
            }
            merged.insert(key.clone(), value.clone());
        }
        Self::from_json(&Value::Object(merged))
    }
}

/// 环境名归一化：先 trim，再整体小写化。校验在 [`validate_name`] 里。
pub fn normalize_name(raw: &str) -> String {
    raw.trim().to_lowercase()
}

/// `^[a-z][a-z0-9_-]{0,31}$` —— 归一化之后仍不合规即失败。
pub fn validate_name(name: &str) -> Result<(), Error> {
    let field = format!("{PATH}.name");
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => {
            return Err(Error::invalid_config(
                field,
                format!("invalid environment name {name:?}: must match ^[a-z][a-z0-9_-]{{0,31}}$"),
            ));
        }
    }
    if name.chars().count() > NAME_MAX_LEN {
        return Err(Error::invalid_config(
            field,
            format!("invalid environment name {name:?}: longer than {NAME_MAX_LEN} characters"),
        ));
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
        return Err(Error::invalid_config(
            field,
            format!("invalid environment name {name:?}: must match ^[a-z][a-z0-9_-]{{0,31}}$"),
        ));
    }
    Ok(())
}

/// 规则名归一化 + 校验：`null`/缺省 = 不覆盖；否则必须是白名单名字。
///
/// **这是路径穿越的唯一防线**：调用方给的是**名字**，不是路径。管理器负责拼
/// `<rules_dir>/<name>.rules`，因此这里拒绝一切分隔符、`..` 与绝对路径。
pub fn normalize_rules(value: Option<&Value>) -> Result<Option<String>, Error> {
    let field = format!("{PATH}.rules");
    let Some(value) = value else { return Ok(None) };
    if value.is_null() {
        return Ok(None);
    }
    let raw = value.as_str().ok_or_else(|| {
        Error::invalid_config(
            &field,
            format!("rules must be a string or null, got {value}"),
        )
    })?;
    let name = raw.trim().to_lowercase();
    validate_rules_name(&name)?;
    Ok(Some(name))
}

/// 规则名白名单（沿用 v1 的 `RULES_NAME_RE`，与 `NAME_RE` 同形）。
pub fn validate_rules_name(name: &str) -> Result<(), Error> {
    let field = format!("{PATH}.rules");
    let mut chars = name.chars();
    let ok_first = matches!(chars.next(), Some(first) if first.is_ascii_lowercase());
    let ok_rest = chars
        .clone()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if !ok_first || !ok_rest || name.chars().count() > NAME_MAX_LEN {
        return Err(Error::invalid_config(
            field,
            format!(
                "invalid rules name {name:?}: must match ^[a-z][a-z0-9_-]{{0,31}}$ \
                 (a name, not a path — no separators, no '..')"
            ),
        ));
    }
    Ok(())
}

fn require_string(object: &Map<String, Value>, key: &str, field: &str) -> Result<String, Error> {
    match object.get(key) {
        Some(Value::String(value)) => Ok(value.clone()),
        Some(other) => Err(Error::invalid_config(
            field,
            format!("{key} must be a string, got {other}"),
        )),
        None => Err(Error::invalid_config(field, format!("{key} is required"))),
    }
}

/// `listen` 解析：`host` 缺省取 [`DEFAULT_LISTEN_HOST`]，`port` 必填。
fn parse_listen(value: Option<&Value>) -> Result<(IpAddr, u16), Error> {
    let host_field = format!("{PATH}.listen.host");
    let port_field = format!("{PATH}.listen.port");
    let Some(value) = value else {
        return Err(Error::invalid_config(port_field, "listen.port is required"));
    };
    let object = value.as_object().ok_or_else(|| {
        Error::invalid_config(format!("{PATH}.listen"), "listen must be an object")
    })?;

    let host = match object.get("host") {
        None | Some(Value::Null) => DEFAULT_LISTEN_HOST,
        Some(Value::String(raw)) => parse_ip_literal(raw).ok_or_else(|| {
            Error::invalid_config(
                &host_field,
                format!(
                    "listen host {raw:?} is not a bare IP literal \
                         (hostnames and 'ip:port' are rejected)"
                ),
            )
        })?,
        Some(other) => {
            return Err(Error::invalid_config(
                &host_field,
                format!("listen.host must be a string, got {other}"),
            ));
        }
    };

    let port = match object.get("port") {
        Some(Value::Number(number)) => {
            let raw = number.as_u64().ok_or_else(|| {
                Error::invalid_config(
                    &port_field,
                    format!("listen.port must be a positive integer, got {number}"),
                )
            })?;
            u16::try_from(raw).map_err(|_| {
                Error::invalid_config(
                    &port_field,
                    format!("listen.port {raw} is out of range (1..=65535)"),
                )
            })?
        }
        Some(other) => {
            return Err(Error::invalid_config(
                &port_field,
                format!("listen.port must be an integer, got {other}"),
            ));
        }
        None => {
            return Err(Error::invalid_config(
                &port_field,
                "listen.port is required",
            ));
        }
    };

    if port == 0 {
        return Err(Error::invalid_config(
            &port_field,
            "listen.port 0 is invalid: port allocation is port.allocate's job, \
             not '0 means auto'",
        ));
    }
    Ok((host, port))
}

fn parse_description(value: Option<&Value>) -> Result<String, Error> {
    let field = format!("{PATH}.description");
    let text = match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(raw)) => raw.trim().to_string(),
        Some(other) => {
            return Err(Error::invalid_config(
                &field,
                format!("description must be a string, got {other}"),
            ));
        }
    };
    if text.chars().count() > DESCRIPTION_MAX_CHARS {
        return Err(Error::invalid_config(
            field,
            format!("description is longer than {DESCRIPTION_MAX_CHARS} characters"),
        ));
    }
    Ok(text)
}

/// 裸 IP 字面量解析：见 [`envboard_core_api::parse_ip_literal`]。
///
/// 刻意**不接受**主机名与 `ip:port`：监听地址是环境的对外身份，
/// 让它依赖解析会让工作台显示的端口与客户端能连的地址分叉。
pub use envboard_core_api::parse_ip_literal;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalization_trims_and_lowercases() {
        assert_eq!(normalize_name("  Beta-2  "), "beta-2");
        assert_eq!(
            normalize_rules(Some(&json!("  BLUE "))).unwrap().as_deref(),
            Some("blue")
        );
    }

    #[test]
    fn removed_v1_fields_are_rejected_with_paths() {
        for (field, value) in [
            ("dns_servers", json!(["10.0.0.53"])),
            ("hosts", json!({"api.example.com": "10.0.0.11"})),
            ("domain_suffix", json!("beta.example.com")),
            ("color", json!("#4c8dff")),
        ] {
            let mut input = json!({"name": "prod", "listen": {"port": 16600}});
            input[field] = value;
            let error = Environment::from_json(&input).unwrap_err();
            assert_eq!(
                error.field.as_deref(),
                Some(format!("environment.{field}").as_str())
            );
        }
    }

    #[test]
    fn ipv6_listen_host_accepts_brackets_and_normalizes() {
        let env = Environment::from_json(&json!({
            "name": "six",
            "listen": {"host": "[::1]", "port": 16601}
        }))
        .unwrap();
        assert_eq!(env.listen().host.to_string(), "::1");
    }

    #[test]
    fn merge_keeps_unspecified_fields_and_clears_with_explicit_null() {
        let base = Environment::from_json(&json!({
            "name": "gray",
            "listen": {"port": 16600},
            "rules": "alpha",
            "description": "keep me"
        }))
        .unwrap();
        let patched = base.merged(&json!({"rules": null})).unwrap();
        assert_eq!(patched.rules(), None);
        assert_eq!(patched.description(), "keep me");
        assert_eq!(patched.listen().port, 16600);
    }

    #[test]
    fn merge_replaces_listen_as_a_whole() {
        let base =
            Environment::from_json(&json!({"name": "gray", "listen": {"port": 16600}})).unwrap();
        // 只给 host：端口丢了 → 必填失败（而不是悄悄保留旧端口）
        let error = base
            .merged(&json!({"listen": {"host": "0.0.0.0"}}))
            .unwrap_err();
        assert_eq!(error.field.as_deref(), Some("environment.listen.port"));
    }
}

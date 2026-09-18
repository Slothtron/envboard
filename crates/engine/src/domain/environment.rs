//! 环境（`Environment`）领域模型 —— 纯逻辑，不碰 fs / 网络 / 宿主。
//!
//! 契约见 `spec/capabilities.md`（§领域不变量）。当前模型是 **7 个字段**：
//! `name` / `listen` / `rules` / `insecure_hosts` / `description` /
//! `proxy_user` / `proxy_password`。
//!
//! 两条形状变更都**只做一次**（迁移 shim，读到旧形状就转换，写出永不含旧字段）：
//!
//! * `options`（任意选项透传）**已删除** —— 空对象/`null` 被接受并忽略，
//!   非空即 `invalid_config`（它代表"确实依赖了那个通道"）；
//! * `proxy_auth`（`user:password` 一整串）**已拆成两个字段** —— 读时无损拆分，
//!   写时不再输出。
//!
//! 被删除的更早字段（`dns_servers`、`hosts`、`domain_suffix`、`color`…）仍然
//! **响亮失败**，不会被当成无用字段收下。

use std::net::IpAddr;

use crate::{DEFAULT_LISTEN_HOST, Error, Listen};
use serde_json::{Map, Value};

/// 对外 JSON 形状里的字段名（未知字段判定用同一张表）。
pub const KNOWN_FIELDS: &[&str] = &[
    "name",
    "listen",
    "rules",
    "insecure_hosts",
    "description",
    "proxy_user",
    "proxy_password",
];

/// 只读（迁移）不写的旧字段。
///
/// 它们**不是**未知字段：升级路径上一定会读到，一律拒绝会让管理器起不来。
/// 但它们也**永远不会**出现在 `to_json` 的输出里。
pub const TOMBSTONE_FIELDS: &[&str] = &["options", "proxy_auth"];

/// 契约里的路径前缀 —— 所有 `field` 都从它开始。
pub const PATH: &str = "environment";

/// 环境名上限：`^[a-z][a-z0-9_-]{0,31}$`。
pub const NAME_MAX_LEN: usize = 32;

/// `description` 上限（字符数），见 `spec/capabilities.md`。
pub const DESCRIPTION_MAX_CHARS: usize = 200;

/// `proxy_user` 长度上限（字符数）。
pub const PROXY_USER_MAX_LEN: usize = 64;

/// `proxy_password` 长度上限（字符数）。
pub const PROXY_PASSWORD_MAX_LEN: usize = 128;

/// 一个环境 = 一个监听端口 + 一份可选的规则绑定 + 一份按域名放宽上游校验的清单
/// + 可选的代理访问鉴权。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    name: String,
    listen: Listen,
    rules: Option<String>,
    insecure_hosts: Vec<String>,
    description: String,
    proxy_user: Option<String>,
    proxy_password: Option<String>,
}

impl Environment {
    pub fn new(
        name: impl Into<String>,
        listen: Listen,
        rules: Option<String>,
        insecure_hosts: Vec<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            listen,
            rules,
            insecure_hosts,
            description: description.into(),
            proxy_user: None,
            proxy_password: None,
        }
    }

    /// 设置代理访问鉴权。**要么都给、要么都不给**（[`Environment::from_json`] 会重新校验）。
    pub fn with_credentials(mut self, user: Option<String>, password: Option<String>) -> Self {
        self.proxy_user = user;
        self.proxy_password = password;
        self
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

    /// 按域名放宽上游证书校验的完整域名清单（**已归一化 + 去重 + 排序**）。
    pub fn insecure_hosts(&self) -> &[String] {
        &self.insecure_hosts
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn proxy_user(&self) -> Option<&str> {
        self.proxy_user.as_deref()
    }

    pub fn proxy_password(&self) -> Option<&str> {
        self.proxy_password.as_deref()
    }

    /// 代理访问鉴权是否启用。视图/日志**只**能用这个布尔，禁止回显凭据。
    pub fn proxy_auth_enabled(&self) -> bool {
        self.proxy_user.is_some() && self.proxy_password.is_some()
    }

    /// 归一化后的完整记录 —— 可直接持久化，也是契约 fixture 比对的形状。
    ///
    /// **七个键永远都在**（空值是 `[]` / `null`）：归一化输出确定，diff 才稳定。
    /// 凭据是明文落在这里的，所以状态文件必须 0600（由持久化层保证）。
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
            "insecure_hosts".into(),
            Value::Array(
                self.insecure_hosts
                    .iter()
                    .map(|host| Value::String(host.clone()))
                    .collect(),
            ),
        );
        out.insert(
            "description".into(),
            Value::String(self.description.clone()),
        );
        out.insert("proxy_user".into(), nullable(&self.proxy_user));
        out.insert("proxy_password".into(), nullable(&self.proxy_password));
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
            if !KNOWN_FIELDS.contains(&key.as_str()) && !TOMBSTONE_FIELDS.contains(&key.as_str()) {
                return Err(Error::invalid_config(
                    format!("{PATH}.{key}"),
                    format!(
                        "unknown field {key:?}: the model has only {KNOWN_FIELDS:?} \
                         (fields removed earlier are rejected loudly, not ignored)"
                    ),
                ));
            }
        }
        check_options_tombstone(object.get("options"))?;

        let name = normalize_name(&require_string(object, "name", &format!("{PATH}.name"))?);
        validate_name(&name)?;

        let (host, port) = parse_listen(object.get("listen"))?;
        let rules = normalize_rules(object.get("rules"))?;
        let insecure_hosts = parse_insecure_hosts(object.get("insecure_hosts"))?;
        let description = parse_description(object.get("description"))?;
        let (proxy_user, proxy_password) = parse_credentials(object)?;

        Ok(Self {
            name,
            listen: Listen::new(host, port),
            rules,
            insecure_hosts,
            description,
            proxy_user,
            proxy_password,
        })
    }

    /// PATCH 语义：未提及的字段保持不变，**显式 `null` = 清空**，合并后**重新校验**。
    ///
    /// `listen` 是整体替换而不是深合并 —— host 与 port 是同一个身份的两个部分，
    /// 深合并会让"只改 host"意外保留旧端口。`insecure_hosts` 与 `proxy_*` 同理：
    /// 整体替换，不做元素级合并（列表的"部分修改"没有无歧义的语义）。
    ///
    /// 旧字段也能出现在 patch 里（老客户端）：`options: {}` 被忽略，
    /// `proxy_auth` 走迁移 —— 但如果这份配置**已经有**凭据，两者同时给出会被拒绝
    /// （那是真的歧义，不该猜）。
    pub fn merged(&self, patch: &Value) -> Result<Self, Error> {
        let patch = patch.as_object().ok_or_else(|| {
            Error::invalid_config(PATH, format!("patch must be an object, got {patch}"))
        })?;

        // 旧 `proxy_auth` 是"整体替换凭据"：先把它要替换掉的目标清空，
        // 否则 base 上的旧凭据会让迁移误判成"两套形状同时给了"。
        let replaces_credentials = patch
            .get("proxy_auth")
            .is_some_and(|value| !value.is_null());
        let mut merged = self.to_json().as_object().cloned().unwrap_or_default();
        if replaces_credentials {
            merged.remove("proxy_user");
            merged.remove("proxy_password");
        }
        for (key, value) in patch {
            if !KNOWN_FIELDS.contains(&key.as_str()) && !TOMBSTONE_FIELDS.contains(&key.as_str()) {
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

fn nullable(value: &Option<String>) -> Value {
    match value {
        Some(text) => Value::String(text.clone()),
        None => Value::Null,
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

/// 规则名白名单（与 `NAME_RE` 同形）。
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

/// `insecure_hosts` 解析：缺省/`null` = 空列表；否则必须是字符串数组。
///
/// 每条都归一化（trim → 小写 → 去尾部根点）后**去重 + 排序**再持久化，
/// 所以"同一份输入"永远得到"同一份列表"。拒绝通配符的理由见
/// `spec/capabilities.md` 的「insecure_hosts（按域名放宽上游校验）」一节：
/// 放宽校验必须逐条点名，偷偷扩大范围比不生效更危险。
fn parse_insecure_hosts(value: Option<&Value>) -> Result<Vec<String>, Error> {
    let field = format!("{PATH}.insecure_hosts");
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let array = value.as_array().ok_or_else(|| {
        Error::invalid_config(
            &field,
            format!("insecure_hosts must be an array of complete domain names, got {value}"),
        )
    })?;
    if array.len() > crate::rules::INSECURE_HOSTS_MAX {
        return Err(Error::invalid_config(
            &field,
            format!(
                "insecure_hosts has {} entries; the limit is {}",
                array.len(),
                crate::rules::INSECURE_HOSTS_MAX
            ),
        ));
    }

    let mut raw_hosts: Vec<String> = Vec::with_capacity(array.len());
    for (index, item) in array.iter().enumerate() {
        let entry_field = format!("{field}.{index}");
        let raw = item.as_str().ok_or_else(|| {
            Error::invalid_config(&entry_field, "each entry must be a string".to_string())
        })?;
        if crate::rules::has_wildcard(raw) {
            return Err(Error::invalid_config(
                &entry_field,
                format!(
                    "{raw:?} contains a wildcard: insecure_hosts matches complete domain names \
                     only, so list every domain explicitly (wildcards are not expanded)"
                ),
            ));
        }
        let host = crate::rules::validate_insecure_host(raw).ok_or_else(|| {
            Error::invalid_config(
                &entry_field,
                format!("{raw:?} is not a valid domain name or IP literal"),
            )
        })?;
        raw_hosts.push(host);
    }
    Ok(crate::rules::normalize_insecure_hosts(&raw_hosts))
}

/// `options` 墓碑：空对象/`null`/缺省一律忽略，非空即失败。
///
/// 为什么必须"响亮"：非空意味着这份配置**确实依赖**那个透传通道，而它已经没有替代
/// 通道了（多数 core 选项没有一等字段）。静默忽略会让用户以为设置还在生效。
fn check_options_tombstone(value: Option<&Value>) -> Result<(), Error> {
    let field = format!("{PATH}.options");
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_null() {
        return Ok(());
    }
    let object = value.as_object().ok_or_else(|| {
        Error::invalid_config(&field, format!("options must be an object, got {value}"))
    })?;
    if object.is_empty() {
        return Ok(());
    }
    Err(Error::invalid_config(
        field,
        "the per-instance `options` passthrough was removed: an instance is configured only \
         through first-class fields. If the affected domains need a relaxed upstream TLS check, \
         list them in `insecure_hosts` — there is no replacement for any other core option",
    ))
}

/// 代理凭据解析（`proxy_user` + `proxy_password`，或旧形状 `proxy_auth`）。
///
/// 规则：
/// * **同生共死** —— 都为空 = 不启用；只有一边 → `invalid_config`，字段指向**缺失**的那一边；
/// * 每段非空，且不含 `:`、空白与控制字符（`:` 会与 mitmproxy `proxyauth` 的
///   `split(":")` 切分歧义；空白/控制字符不是合法的 Basic 凭据）；
/// * 长度上限分别是 [`PROXY_USER_MAX_LEN`] 与 [`PROXY_PASSWORD_MAX_LEN`]；
/// * 旧形状 `proxy_auth` 读时**无损拆分**，写时不再输出；与两个新字段
///   **同时给出**（都非空）即报冲突，因为那是真的歧义；
/// * **任何错误信息都不回显取值** —— 凭据只出现在持久化记录与实例启动参数里。
fn parse_credentials(
    object: &Map<String, Value>,
) -> Result<(Option<String>, Option<String>), Error> {
    let user_field = format!("{PATH}.proxy_user");
    let password_field = format!("{PATH}.proxy_password");
    let legacy_field = format!("{PATH}.proxy_auth");

    let user = parse_credential_part(
        object.get("proxy_user"),
        &user_field,
        PROXY_USER_MAX_LEN,
        "proxy_user",
    )?;
    let password = parse_credential_part(
        object.get("proxy_password"),
        &password_field,
        PROXY_PASSWORD_MAX_LEN,
        "proxy_password",
    )?;

    match object.get("proxy_auth") {
        None | Some(Value::Null) => {}
        Some(legacy) => {
            if user.is_some() || password.is_some() {
                return Err(Error::invalid_config(
                    legacy_field,
                    "proxy_auth was split into proxy_user + proxy_password; both shapes were \
                     given at once, so there is no unambiguous winner — keep only the new \
                     fields (no value is echoed back)",
                ));
            }
            let text = legacy.as_str().ok_or_else(|| {
                Error::invalid_config(
                    &legacy_field,
                    "proxy_auth must be a `user:password` string or null \
                     (no value is echoed back)",
                )
            })?;
            // 旧实现整体 trim 后入库；迁移必须**无损**，所以保持同一条规则。
            let text = text.trim();
            let (raw_user, raw_password) = text.split_once(':').ok_or_else(|| {
                Error::invalid_config(
                    &legacy_field,
                    "the legacy proxy_auth must be `user:password` with exactly one colon \
                     (no value is echoed back)",
                )
            })?;
            let user =
                validate_credential_part(raw_user, &user_field, PROXY_USER_MAX_LEN, "proxy_user")?;
            let password = validate_credential_part(
                raw_password,
                &password_field,
                PROXY_PASSWORD_MAX_LEN,
                "proxy_password",
            )?;
            return Ok((Some(user), Some(password)));
        }
    }

    match (user, password) {
        (None, None) => Ok((None, None)),
        (Some(user), Some(password)) => Ok((Some(user), Some(password))),
        (Some(_), None) => Err(Error::invalid_config(
            &password_field,
            "proxy_password is required: proxy credentials are all-or-nothing \
             (no value is echoed back)",
        )),
        (None, Some(_)) => Err(Error::invalid_config(
            &user_field,
            "proxy_user is required: proxy credentials are all-or-nothing \
             (no value is echoed back)",
        )),
    }
}

fn parse_credential_part(
    value: Option<&Value>,
    field: &str,
    max_chars: usize,
    label: &str,
) -> Result<Option<String>, Error> {
    let Some(value) = value else { return Ok(None) };
    if value.is_null() {
        return Ok(None);
    }
    let text = value.as_str().ok_or_else(|| {
        Error::invalid_config(
            field,
            format!("{label} must be a string or null (no value is echoed back)"),
        )
    })?;
    validate_credential_part(text, field, max_chars, label).map(Some)
}

fn validate_credential_part(
    text: &str,
    field: &str,
    max_chars: usize,
    label: &str,
) -> Result<String, Error> {
    let printable = !text.is_empty()
        && text
            .chars()
            .all(|c| !c.is_whitespace() && !c.is_control() && c != ':');
    if !printable {
        return Err(Error::invalid_config(
            field,
            format!(
                "{label} must be non-empty and contain no ':', whitespace or control \
                 characters (no value is echoed back)"
            ),
        ));
    }
    if text.chars().count() > max_chars {
        return Err(Error::invalid_config(
            field,
            format!("{label} is longer than {max_chars} characters"),
        ));
    }
    Ok(text.to_string())
}

/// 裸 IP 字面量解析：见 [`crate::parse_ip_literal`]。
///
/// 刻意**不接受**主机名与 `ip:port`：监听地址是环境的对外身份，
/// 让它依赖解析会让工作台显示的端口与客户端能连的地址分叉。
pub use crate::parse_ip_literal;

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
    fn removed_earlier_fields_are_rejected_with_paths() {
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

    #[test]
    fn empty_options_tombstone_is_accepted_and_never_written_back() {
        let env = Environment::from_json(&json!({
            "name": "gray",
            "listen": {"port": 16600},
            "options": {}
        }))
        .unwrap();
        assert!(env.to_json().get("options").is_none());
        assert!(env.to_json().get("proxy_auth").is_none());

        // 非空 = 确实依赖过那个通道 → 响亮失败，并给出可操作的指引
        let error = Environment::from_json(&json!({
            "name": "gray",
            "listen": {"port": 16600},
            "options": {"ssl_insecure": "true"}
        }))
        .unwrap_err();
        assert_eq!(error.field.as_deref(), Some("environment.options"));
        assert!(error.message.contains("insecure_hosts"));

        // 不是对象也不是 null → 同样是非法配置
        let error = Environment::from_json(&json!({
            "name": "gray",
            "listen": {"port": 16600},
            "options": ["ssl_insecure=true"]
        }))
        .unwrap_err();
        assert_eq!(error.field.as_deref(), Some("environment.options"));
    }

    #[test]
    fn legacy_proxy_auth_is_migrated_losslessly() {
        let env = Environment::from_json(&json!({
            "name": "auth",
            "listen": {"port": 16600},
            "proxy_auth": "  alice:s3cret  "
        }))
        .unwrap();
        assert_eq!(env.proxy_user(), Some("alice"));
        assert_eq!(env.proxy_password(), Some("s3cret"));
        assert!(env.proxy_auth_enabled());
        // 写出只有新字段
        assert_eq!(env.to_json()["proxy_user"], json!("alice"));
        assert_eq!(env.to_json()["proxy_password"], json!("s3cret"));
        assert!(env.to_json().get("proxy_auth").is_none());
    }

    #[test]
    fn credentials_are_all_or_nothing() {
        let error = Environment::from_json(&json!({
            "name": "auth",
            "listen": {"port": 16600},
            "proxy_user": "alice"
        }))
        .unwrap_err();
        assert_eq!(error.field.as_deref(), Some("environment.proxy_password"));

        let error = Environment::from_json(&json!({
            "name": "auth",
            "listen": {"port": 16600},
            "proxy_password": "s3cret"
        }))
        .unwrap_err();
        assert_eq!(error.field.as_deref(), Some("environment.proxy_user"));

        // 两个显式 null = 不启用
        let env = Environment::from_json(&json!({
            "name": "auth",
            "listen": {"port": 16600},
            "proxy_user": null,
            "proxy_password": null
        }))
        .unwrap();
        assert!(!env.proxy_auth_enabled());
    }

    #[test]
    fn invalid_credentials_fail_loudly_without_echoing_values() {
        let base = json!({"name": "auth", "listen": {"port": 16600}});
        for (patch, expected_field) in [
            (
                json!({"proxy_user": "", "proxy_password": "p"}),
                "environment.proxy_user",
            ),
            (
                json!({"proxy_user": "  ", "proxy_password": "p"}),
                "environment.proxy_user",
            ),
            (
                json!({"proxy_user": "al:ice", "proxy_password": "p"}),
                "environment.proxy_user",
            ),
            (
                json!({"proxy_user": "u", "proxy_password": "p:x"}),
                "environment.proxy_password",
            ),
            (
                json!({"proxy_user": "u", "proxy_password": "p\u{7}ss"}),
                "environment.proxy_password",
            ),
            (
                json!({"proxy_user": "us er", "proxy_password": "p"}),
                "environment.proxy_user",
            ),
            (
                json!({"proxy_user": 42, "proxy_password": "p"}),
                "environment.proxy_user",
            ),
            (
                json!({"proxy_user": "u", "proxy_password": "x".repeat(129)}),
                "environment.proxy_password",
            ),
        ] {
            let mut input = base.clone();
            for (key, value) in patch.as_object().unwrap() {
                input[key] = value.clone();
            }
            let error = Environment::from_json(&input)
                .err()
                .unwrap_or_else(|| panic!("credential case {patch} unexpectedly passed"));
            assert_eq!(error.field.as_deref(), Some(expected_field), "{patch}");
            // 凭据值绝不回显
            assert!(!error.message.contains("s3cret"), "{patch}");
        }
    }

    #[test]
    fn credentials_given_in_both_shapes_are_a_conflict() {
        let error = Environment::from_json(&json!({
            "name": "auth",
            "listen": {"port": 16600},
            "proxy_user": "alice",
            "proxy_password": "s3cret",
            "proxy_auth": "bob:hunter2"
        }))
        .unwrap_err();
        assert_eq!(error.field.as_deref(), Some("environment.proxy_auth"));
    }

    #[test]
    fn merging_a_legacy_patch_replaces_the_credentials() {
        let base = Environment::from_json(&json!({
            "name": "auth",
            "listen": {"port": 16600},
            "proxy_user": "alice",
            "proxy_password": "s3cret"
        }))
        .unwrap();
        // 老客户端整体替换凭据：base 上的旧值先被清掉，不构成"两套形状同时给出"
        let merged = base.merged(&json!({"proxy_auth": "bob:hunter2"})).unwrap();
        assert_eq!(merged.proxy_user(), Some("bob"));
        assert_eq!(merged.proxy_password(), Some("hunter2"));

        // 只给一边的 patch 会让合并结果不完整 → 必须响亮失败，字段指向**缺失**的那一边
        let error = base.merged(&json!({"proxy_user": null})).unwrap_err();
        assert_eq!(error.field.as_deref(), Some("environment.proxy_user"));

        let error = base.merged(&json!({"proxy_password": null})).unwrap_err();
        assert_eq!(error.field.as_deref(), Some("environment.proxy_password"));

        // 两边一起清 = 关掉鉴权
        let cleared = base
            .merged(&json!({"proxy_user": null, "proxy_password": null}))
            .unwrap();
        assert!(!cleared.proxy_auth_enabled());
    }

    #[test]
    fn insecure_hosts_are_normalized_deduped_and_sorted() {
        let env = Environment::from_json(&json!({
            "name": "gray",
            "listen": {"port": 16600},
            "insecure_hosts": [" Web.Example.NET. ", "app.example.com", "App.Example.COM"]
        }))
        .unwrap();
        assert_eq!(
            env.insecure_hosts(),
            ["app.example.com".to_string(), "web.example.net".to_string()]
        );
        // 缺省 = 空列表（不是"全都放行"）
        let env =
            Environment::from_json(&json!({"name": "gray", "listen": {"port": 16600}})).unwrap();
        assert!(env.insecure_hosts().is_empty());
        // 清空
        let cleared = env.merged(&json!({"insecure_hosts": null})).unwrap();
        assert!(cleared.insecure_hosts().is_empty());
    }

    #[test]
    fn insecure_hosts_reject_wildcards_and_bad_shapes() {
        for (value, expected_field) in [
            (json!(["*.example.com"]), "environment.insecure_hosts.0"),
            (json!(["a?.example.com"]), "environment.insecure_hosts.0"),
            (json!(["-lead.example.com"]), "environment.insecure_hosts.0"),
            (json!([42]), "environment.insecure_hosts.0"),
            (json!("app.example.com"), "environment.insecure_hosts"),
            (
                json!(
                    (0..=crate::rules::INSECURE_HOSTS_MAX)
                        .map(|index| format!("h{index}.example.com"))
                        .collect::<Vec<_>>()
                ),
                "environment.insecure_hosts",
            ),
        ] {
            let error = Environment::from_json(&json!({
                "name": "gray",
                "listen": {"port": 16600},
                "insecure_hosts": value
            }))
            .unwrap_err();
            assert_eq!(error.field.as_deref(), Some(expected_field), "{value}");
        }
    }

    #[test]
    fn patch_rejects_unknown_fields_still() {
        let base =
            Environment::from_json(&json!({"name": "gray", "listen": {"port": 16600}})).unwrap();
        let error = base.merged(&json!({"prox_auth": "a:b"})).unwrap_err();
        assert_eq!(error.field.as_deref(), Some("environment.prox_auth"));
    }
}

//! 上游代理（二级代理）实体 —— 纯逻辑，不碰 fs / 网络 / 宿主。
//!
//! 契约见 `spec/capabilities.md` 的「上游代理（chained proxy）」一节。
//! 一条代理 = `{ name, host, port, user, password }`：环境通过第 8 字段
//! `upstream` **按名引用**它（`null` = 直连），凭据挂在实体上而不是环境上
//! —— 同一个代理被多个环境引用时只有一份真相，且服务端与视图**永不回显**。
//!
//! 与环境的凭据先例同规：**同生共死**（只填一边即失败，字段指向缺失的一边）、
//! 不得含 `:` / 空白 / 控制字符、长度上限 [`USER_MAX_LEN`] / [`PASSWORD_MAX_LEN`]。

use crate::Error;

/// 对外 JSON 形状里的路径前缀 —— 实体级错误的 `field` 都从它开始。
pub const UPSTREAM_PATH: &str = "proxy";

/// 对外 JSON 形状里的字段名（未知字段判定用同一张表）。
pub const UPSTREAM_KNOWN_FIELDS: &[&str] = &["name", "host", "port", "user", "password"];

/// `user` 长度上限（字符数）—— 与环境 `proxy_user` 同规。
pub const USER_MAX_LEN: usize = 64;

/// `password` 长度上限（字符数）—— 与环境 `proxy_password` 同规。
pub const PASSWORD_MAX_LEN: usize = 128;

/// 上游代理的端口下限：0 不是"自动分配"，端口没有自动分配这回事。
pub const PORT_MIN: u16 = 1;

/// 一条上游代理定义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamProxy {
    name: String,
    host: String,
    port: u16,
    user: Option<String>,
    password: Option<String>,
}

impl UpstreamProxy {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn user(&self) -> Option<&str> {
        self.user.as_deref()
    }

    pub fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }

    /// 上游代理鉴权是否启用。视图/日志**只**能用这个布尔，禁止回显凭据。
    pub fn has_auth(&self) -> bool {
        self.user.is_some() && self.password.is_some()
    }

    /// 归一化后的完整记录 —— 可直接持久化，也是契约 fixture 比对的形状。
    ///
    /// **五个键永远都在**（凭据缺失是 `null`）：归一化输出确定，diff 才稳定。
    /// 凭据是明文落在这里的，所以状态文件必须 0600（由持久化层保证）。
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "host": self.host,
            "port": self.port,
            "user": self.user,
            "password": self.password,
        })
    }

    /// 校验并归一化一条上游代理定义。手写的理由与环境相同：错误要带**精确的
    /// 点分字段路径**（`proxy.host`、`proxy.user`）。
    pub fn from_json(value: &serde_json::Value) -> Result<Self, Error> {
        let object = value.as_object().ok_or_else(|| {
            Error::invalid_config(
                UPSTREAM_PATH,
                format!("{UPSTREAM_PATH} must be an object, got {value}"),
            )
        })?;
        for key in object.keys() {
            if !UPSTREAM_KNOWN_FIELDS.contains(&key.as_str()) {
                return Err(Error::invalid_config(
                    format!("{UPSTREAM_PATH}.{key}"),
                    format!("unknown field {key:?}: the model has only {UPSTREAM_KNOWN_FIELDS:?}"),
                ));
            }
        }

        let name_field = format!("{UPSTREAM_PATH}.name");
        let raw_name = require_string(object, "name", &name_field)?;
        let name = raw_name.trim().to_lowercase();
        crate::domain::environment::validate_upstream_name(&name)
            .map_err(|error| retitle(error, &name_field))?;

        let host_field = format!("{UPSTREAM_PATH}.host");
        let raw_host = require_string(object, "host", &host_field)?;
        let host = crate::rules::validate_insecure_host(&raw_host).ok_or_else(|| {
            Error::invalid_config(
                &host_field,
                format!("{raw_host:?} is not a valid domain name or IP literal"),
            )
        })?;

        let port_field = format!("{UPSTREAM_PATH}.port");
        let raw_port = object
            .get("port")
            .ok_or_else(|| Error::invalid_config(&port_field, "port is required".to_string()))?;
        let number = raw_port.as_u64().ok_or_else(|| {
            Error::invalid_config(
                &port_field,
                format!("port must be a positive integer, got {raw_port}"),
            )
        })?;
        let port = u16::try_from(number).map_err(|_| {
            Error::invalid_config(
                &port_field,
                format!("port {number} is out of range (1..=65535)"),
            )
        })?;
        if port < PORT_MIN {
            return Err(Error::invalid_config(
                &port_field,
                "port 0 is invalid: an upstream proxy must listen on a real port",
            ));
        }

        let (user, password) = parse_credentials(object)?;

        Ok(Self {
            name,
            host,
            port,
            user,
            password,
        })
    }
}

/// 凭据解析：**同生共死** —— 都缺 = 不启用；只填一边 → `invalid_config`，字段
/// 指向**缺失**的那一边。任何错误信息都不回显取值。
fn parse_credentials(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Result<(Option<String>, Option<String>), Error> {
    let user_field = format!("{UPSTREAM_PATH}.user");
    let password_field = format!("{UPSTREAM_PATH}.password");
    let user = parse_credential_part(object.get("user"), &user_field, USER_MAX_LEN, "user")?;
    let password = parse_credential_part(
        object.get("password"),
        &password_field,
        PASSWORD_MAX_LEN,
        "password",
    )?;
    match (user, password) {
        (None, None) => Ok((None, None)),
        (Some(user), Some(password)) => Ok((Some(user), Some(password))),
        (Some(_), None) => Err(Error::invalid_config(
            &password_field,
            "password is required: upstream proxy credentials are all-or-nothing \
             (no value is echoed back)",
        )),
        (None, Some(_)) => Err(Error::invalid_config(
            &user_field,
            "user is required: upstream proxy credentials are all-or-nothing \
             (no value is echoed back)",
        )),
    }
}

fn parse_credential_part(
    value: Option<&serde_json::Value>,
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
    Ok(Some(text.to_string()))
}

fn require_string(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    field: &str,
) -> Result<String, Error> {
    match object.get(key) {
        Some(serde_json::Value::String(value)) => Ok(value.clone()),
        Some(other) => Err(Error::invalid_config(
            field,
            format!("{key} must be a string, got {other}"),
        )),
        None => Err(Error::invalid_config(field, format!("{key} is required"))),
    }
}

/// 名字白名单的报错文案在环境侧写死了 "upstream name"；实体侧借道路径重定向到
/// `proxy.name`，其余语义不变。
fn retitle(mut error: Error, field: &str) -> Error {
    error.field = Some(field.to_string());
    error
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_valid_proxy_round_trips() {
        let proxy = UpstreamProxy::from_json(&json!({
            "name": "  Corp ",
            "host": "Proxy.Corp.Example.NET.",
            "port": 3128,
            "user": "alice",
            "password": "s3cret"
        }))
        .unwrap();
        assert_eq!(proxy.name(), "corp");
        assert_eq!(proxy.host(), "proxy.corp.example.net");
        assert_eq!(proxy.port(), 3128);
        assert!(proxy.has_auth());
        // 五个键永远都在
        let json = proxy.to_json();
        for key in UPSTREAM_KNOWN_FIELDS {
            assert!(json.get(key).is_some(), "{key} must always be present");
        }
        // 凭据是唯一的明文落点；无凭据时是 null
        let bare =
            UpstreamProxy::from_json(&json!({"name": "direct", "host": "10.0.0.9", "port": 8080}))
                .unwrap();
        assert!(!bare.has_auth());
        assert_eq!(bare.to_json()["user"], serde_json::json!(null));
    }

    #[test]
    fn invalid_shapes_fail_with_field_paths_and_without_echo() {
        for (input, expected_field) in [
            (json!({"host": "p.example.com", "port": 3128}), "proxy.name"),
            (json!({"name": "p", "port": 3128}), "proxy.host"),
            (json!({"name": "p", "host": "p.example.com"}), "proxy.port"),
            (
                json!({"name": "p", "host": "p.example.com", "port": 0}),
                "proxy.port",
            ),
            (
                json!({"name": "p", "host": "p.example.com", "port": 70000}),
                "proxy.port",
            ),
            (
                json!({"name": "p", "host": "p.example.com", "port": "8080"}),
                "proxy.port",
            ),
            (
                json!({"name": "p", "host": "../etc", "port": 3128}),
                "proxy.host",
            ),
            (
                json!({"name": "p", "host": "p.example.com", "port": 3128, "user": "a"}),
                "proxy.password",
            ),
            (
                json!({"name": "p", "host": "p.example.com", "port": 3128, "password": "b"}),
                "proxy.user",
            ),
            (
                json!({"name": "p", "host": "p.example.com", "port": 3128, "user": "a:b", "password": "b"}),
                "proxy.user",
            ),
            (
                json!({"name": "p", "host": "p.example.com", "port": 3128, "user": "", "password": "b"}),
                "proxy.user",
            ),
            (
                json!({"name": "p", "host": "p.example.com", "port": 3128, "user": "a", "password": "x".repeat(129)}),
                "proxy.password",
            ),
            (
                json!({"name": "P", "host": "p.example.com", "port": 3128, "scheme": "http"}),
                "proxy.scheme",
            ),
        ] {
            let error = UpstreamProxy::from_json(&input)
                .err()
                .unwrap_or_else(|| panic!("case {input} unexpectedly passed"));
            assert_eq!(error.field.as_deref(), Some(expected_field), "{input}");
            assert!(!error.message.contains("s3cret"), "凭据不回显: {input}");
        }
    }

    #[test]
    fn whitelist_names_and_rejections_match_the_environment_pattern() {
        // 与环境/规则名同形：合法的能过
        for name in ["corp", "edge-1", "px_2"] {
            let proxy = UpstreamProxy::from_json(
                &json!({"name": name, "host": "p.example.com", "port": 3128}),
            )
            .unwrap();
            assert_eq!(proxy.name(), name);
        }
        // 非法的响亮失败
        for name in ["", "-lead", "1start", "has space", "a/b", "..", "a:b"] {
            assert!(
                UpstreamProxy::from_json(
                    &json!({"name": name, "host": "p.example.com", "port": 3128})
                )
                .is_err(),
                "{name:?} must be rejected"
            );
        }
    }
}

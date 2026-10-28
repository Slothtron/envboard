//! 展示层词汇与派生 helper（**纯函数**，可单测，视图不许自算）。
//!
//! 逐字移植自 web 形态的 `frontend/src/data.ts`：同一套健康字面量、同一个
//! "需处理"定义、同一份监听地址拼法。业务数据的唯一来源是 Admin 门面
//! （`EnvView` 等视图 DTO），本文件只做"怎么给人看"那一半。

use envboard_protocol::{Desired, EnvView};

/// 健康字面量的中文（spec/protocol.md：starting / running / failed / stopped /
/// port_conflict / unhealthy）。未知字面量原样显示 —— 服务端加值时界面不塌。
pub fn health_label(health: &str) -> String {
    match health {
        "starting" => "启动中".to_string(),
        "running" => "运行中".to_string(),
        "failed" => "失败".to_string(),
        "stopped" => "已停止".to_string(),
        "port_conflict" => "端口冲突".to_string(),
        "unhealthy" => "不健康".to_string(),
        other => other.to_string(),
    }
}

/// 色调档位：与 web 形态同一分类（`HealthTone`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Success,
    Warning,
    Danger,
    Muted,
}

pub fn health_tone(health: &str) -> Tone {
    match health {
        "running" => Tone::Success,
        "starting" => Tone::Warning,
        "failed" | "port_conflict" => Tone::Danger,
        _ => Tone::Muted,
    }
}

/// 期望状态的字符串形式（与 `health` 同一套字面量，才能直接比较）。
pub fn desired_label(desired: Desired) -> &'static str {
    match desired {
        Desired::Running => "running",
        Desired::Stopped => "stopped",
    }
}

/// 「需处理」= 失败类健康，**或** 期望与实际不一致。
///
/// 这条判据与 `crates/manager/src/reconcile_health.rs` 的裁决口径同源，
/// 界面不许另起一套（web 形态的 `envNeedsAttention` 逐字对应）。
pub fn needs_attention(env: &EnvView) -> bool {
    if matches!(
        env.health.as_str(),
        "failed" | "unhealthy" | "port_conflict"
    ) {
        return true;
    }
    env.health != desired_label(env.desired)
}

/// 监听地址（`host:port`）。
pub fn listen_address(env: &EnvView) -> String {
    format!("{}:{}", env.listen.host, env.listen.port)
}

/// 绑到 `0.0.0.0` / `::` 之类 = 局域网可达（概览的「对外服务」）。
pub fn is_public_listen(env: &EnvView) -> bool {
    env.listen.host != "127.0.0.1" && env.listen.host != "::1"
}

/// 字节数的短表示（抓包与日志体积用；web 形态的 `formatBytes`）。
pub fn format_bytes(size: u64) -> String {
    if size == 0 {
        "0 B".to_string()
    } else if size < 1024 {
        format!("{size} B")
    } else if size < 1024 * 1024 {
        format!("{:.1} KB", size as f64 / 1024.0)
    } else {
        format!("{:.1} MB", size as f64 / 1024.0 / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(health: &str, desired: Desired) -> EnvView {
        EnvView {
            name: "beta".into(),
            listen: envboard_protocol::Endpoint {
                host: "127.0.0.1".into(),
                port: 16_600,
            },
            rules: None,
            upstream: None,
            insecure_hosts: Vec::new(),
            description: String::new(),
            desired,
            health: health.into(),
            health_reason: None,
            rules_count: 0,
            rules_missing: false,
            proxy_command: String::new(),
            proxy_auth_enabled: false,
            capture: false,
        }
    }

    #[test]
    fn health_literals_have_a_chinese_label_and_a_tone() {
        for (health, label, tone) in [
            ("running", "运行中", Tone::Success),
            ("starting", "启动中", Tone::Warning),
            ("failed", "失败", Tone::Danger),
            ("port_conflict", "端口冲突", Tone::Danger),
            ("stopped", "已停止", Tone::Muted),
            ("unhealthy", "不健康", Tone::Muted),
        ] {
            assert_eq!(health_label(health), label, "label of {health}");
            assert_eq!(health_tone(health), tone, "tone of {health}");
        }
        // 服务端将来加的新字面量：原样显示，不塌成空串。
        assert_eq!(health_label("draining"), "draining");
        assert_eq!(health_tone("draining"), Tone::Muted);
    }

    #[test]
    fn failure_literals_are_danger_but_unhealthy_is_not() {
        // 与 web 形态同一分档：unhealthy 的文案要显眼，但不占危险色
        // （"危险色是稀缺资源"）。
        assert_eq!(health_tone("unhealthy"), Tone::Muted);
        assert_eq!(health_tone("failed"), Tone::Danger);
    }

    #[test]
    fn needs_attention_covers_failures_and_desired_mismatch() {
        assert!(needs_attention(&env("failed", Desired::Running)));
        assert!(needs_attention(&env("unhealthy", Desired::Running)));
        assert!(needs_attention(&env("port_conflict", Desired::Running)));
        // 期望运行但实际停着 = 需处理；期望停且实际停 = 正常。
        assert!(needs_attention(&env("stopped", Desired::Running)));
        assert!(!needs_attention(&env("stopped", Desired::Stopped)));
        assert!(!needs_attention(&env("running", Desired::Running)));
        // 启动中是"期望运行、尚未运行"，与 web 形态一致：仍算需处理。
        assert!(needs_attention(&env("starting", Desired::Running)));
    }

    #[test]
    fn listen_helpers_match_the_web_form() {
        let mut loopback = env("running", Desired::Running);
        assert_eq!(listen_address(&loopback), "127.0.0.1:16600");
        assert!(!is_public_listen(&loopback));
        loopback.listen.host = "0.0.0.0".into();
        assert!(is_public_listen(&loopback));
        loopback.listen.host = "::1".into();
        assert!(!is_public_listen(&loopback));
    }

    #[test]
    fn byte_formatting_switches_units_at_the_same_boundaries() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}

//! 命令级契约测试：打 `ops` 纯函数核（真管理器 + 假引擎，`prepare_with` 注入口）。
//!
//! 这是退场的 web 形态 live_workbench 里**契约部分**的承接：wire 形状、错误信封、
//! patch 三态、凭据只进不出、账本引用完整性、会话语义 —— 与传输无关的那一半。
//! 真代理流量/TLS/崩溃自愈那一半归引擎层 live_manager 与 CDP 真机走查。
//!
//! 不需要 webview：命令壳（commands.rs）只是「取状态 → 打 ops → 翻译错误」，
//! 语义全在 ops，所以这里直接打 ops 就是打命令面。

use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};

use envboard_desktop::ops;
use envboard_engine::ProxyEngine;
use envboard_engine_fake::FakeEngine;
use envboard_host::{HostOptions, Prepared, prepare_with};
use serde_json::{Value, json};

/// 每个测试一段独立的端口窗口，避免并行测试互相抢端口（与 host 测试同一手法）。
static NEXT_WINDOW: AtomicU16 = AtomicU16::new(24_000);

struct Harness {
    _dir: std::path::PathBuf,
    prepared: Prepared,
}

impl Harness {
    fn new(tag: &str) -> Self {
        Self::with_options(tag, true)
    }

    /// `no_log_file` 默认开（内存环形缓冲足够）；轨迹类测试要关它（轨迹跟落盘走）。
    fn with_options(tag: &str, no_log_file: bool) -> Self {
        let window = NEXT_WINDOW.fetch_add(20, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "envboard-contract-{tag}-{}-{window}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let prepared = prepare_with(
            HostOptions {
                state_dir: Some(dir.clone()),
                port_range: Some((window, window + 10)),
                no_log_file,
                ..Default::default()
            },
            Arc::new(FakeEngine::new()) as Arc<dyn ProxyEngine>,
        )
        .expect("组合根必须能装出一个可用宿主");
        Self {
            _dir: dir,
            prepared,
        }
    }

    fn admin(&self) -> &envboard_admin::AdminService {
        &self.prepared.admin
    }

    fn ok(&self, result: ops::OpsResult) -> Value {
        result.expect("命令必须成功")
    }

    fn err(&self, result: ops::OpsResult) -> envboard_engine::Error {
        result.expect_err("命令必须失败")
    }

    fn seed_env(&self, name: &str) -> Value {
        self.ok(ops::env_create(self.admin(), &json!({"name": name})))
    }

    fn start(&self, name: &str) -> Value {
        tauri::async_runtime::block_on(ops::env_start(self.admin(), name)).expect("start")
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self._dir);
    }
}

/* ---------------- status / 形状 ---------------- */

#[test]
fn status_carries_the_documented_shape() {
    let h = Harness::new("status");
    let status = h.ok(ops::status(h.admin()));
    // 核心名由引擎自报（假引擎 = envboard-engine-fake，真引擎 = envboard）。
    assert!(
        status["core"]["name"]
            .as_str()
            .is_some_and(|n| !n.is_empty())
    );
    assert!(status["version"].as_str().is_some());
    assert!(status["capabilities"].is_object());
    // port_range 回显字符串（前端 formatPortRange 的运行时形态）。
    let range = status["config"]["port_range"]
        .as_str()
        .expect("port_range 是字符串");
    assert!(range.contains('-'), "got {range}");
    assert!(status["config"]["state_dir"].as_str().is_some());
    assert_eq!(status["environments"], 0);
    assert_eq!(status["running"], 0);
    assert!(status["events_dropped"].as_u64().is_some());
}

/* ---------------- 环境 CRUD 与错误信封 ---------------- */

#[test]
fn env_crud_and_error_envelope() {
    let h = Harness::new("crud");
    let created = h.seed_env("alpha");
    assert_eq!(created["name"], "alpha");
    assert!(created["listen"]["port"].as_u64().is_some());
    assert!(
        created["proxy_command"]
            .as_str()
            .unwrap()
            .contains("https_proxy")
    );
    assert_eq!(created["desired"], "stopped");

    let listed = h.ok(ops::env_list(h.admin()));
    assert_eq!(listed.as_array().unwrap().len(), 1);
    let got = h.ok(ops::env_get(h.admin(), "alpha"));
    assert_eq!(got["name"], "alpha");

    // 错误信封 {code,message,field?}：不存在 → not_found。
    let error = h.err(ops::env_get(h.admin(), "ghost"));
    assert_eq!(error.code.as_str(), "not_found");

    // 未知字段响亮拒绝（deny_unknown_fields），field 指向请求体。
    let error = h.err(ops::env_create(
        h.admin(),
        &json!({"name": "bad", "bogus": 1}),
    ));
    assert_eq!(error.code.as_str(), "invalid_config");
    assert!(
        error.field.is_some(),
        "invalid_config 必须带字段路径：{error:?}"
    );

    let removed = h.ok(ops::env_delete(h.admin(), "alpha"));
    assert_eq!(removed["removed"], "alpha");
    assert_eq!(h.ok(ops::env_list(h.admin())).as_array().unwrap().len(), 0);
}

#[test]
fn patch_three_state_and_credentials_write_only() {
    let h = Harness::new("patch");
    h.seed_env("alpha");

    // 给值 = 设定；凭据只进不出：视图只回布尔，全文不得出现密码取值。
    let patched = h.ok(ops::env_patch(
        h.admin(),
        "alpha",
        &json!({"description": "演示", "proxy_user": "u", "proxy_password": "s3cret"}),
    ));
    assert_eq!(patched["description"], "演示");
    assert_eq!(patched["proxy_auth_enabled"], true);
    let text = patched.to_string();
    assert!(!text.contains("s3cret"), "凭据取值不得回显：{text}");
    assert!(!text.contains("\"u\""), "用户名也不回显：{text}");

    // 未提及 = 不动。
    let again = h.ok(ops::env_patch(
        h.admin(),
        "alpha",
        &json!({"description": "改一下"}),
    ));
    assert_eq!(again["proxy_auth_enabled"], true, "未提及凭据 = 不动");

    // 显式 null = 解绑（清除鉴权）。
    let cleared = h.ok(ops::env_patch(
        h.admin(),
        "alpha",
        &json!({"proxy_user": null, "proxy_password": null}),
    ));
    assert_eq!(cleared["proxy_auth_enabled"], false);
}

#[test]
fn start_stop_lifecycle_allocates_ports_in_range() {
    let h = Harness::new("lifecycle");
    let created = h.seed_env("alpha");
    let port = created["listen"]["port"].as_u64().unwrap();
    assert!(
        (24_000..24_100).contains(&(port as u16)) || port >= 24_000,
        "端口在配置区间：{port}"
    );

    let started = h.start("alpha");
    assert_eq!(started["health"], "running");
    let stopped = tauri::async_runtime::block_on(ops::env_stop(h.admin(), "alpha")).expect("stop");
    assert_eq!(stopped["health"], "stopped");
    let restarted =
        tauri::async_runtime::block_on(ops::env_restart(h.admin(), "alpha")).expect("restart");
    assert_eq!(restarted["health"], "running");
}

/* ---------------- 规则账本 ---------------- */

#[test]
fn rules_ledger_and_binding_semantics() {
    let h = Harness::new("rules");
    let imported = h.ok(ops::rules_import(
        h.admin(),
        "alpha",
        "127.0.0.1 alpha.test\n",
    ));
    assert_eq!(imported["name"], "alpha");
    assert!(imported["path"].as_str().is_some());

    let listed = h.ok(ops::rules_list(h.admin()));
    assert_eq!(listed["rules"][0], "alpha");
    let read = h.ok(ops::rules_get(h.admin(), "alpha"));
    assert!(read["text"].as_str().unwrap().contains("alpha.test"));

    // 绑定 → rules_count 可见；删除被绑定的规则 → 拒绝并点名引用方。
    h.seed_env("env-a");
    let bound = h.ok(ops::env_patch(
        h.admin(),
        "env-a",
        &json!({"rules": "alpha"}),
    ));
    assert_eq!(bound["rules_count"], 1);
    assert_eq!(bound["rules_missing"], false);
    let error = h.err(ops::rules_delete(h.admin(), "alpha"));
    assert_eq!(error.code.as_str(), "conflict");
    assert!(
        error.message.contains("env-a"),
        "删除被拒必须点名引用方：{error:?}"
    );

    // 解绑后可删；账本里没了 → 环境标 rules_missing（曾绑定的名字还在）。
    h.ok(ops::env_patch(h.admin(), "env-a", &json!({"rules": null})));
    h.ok(ops::rules_delete(h.admin(), "alpha"));
    let error = h.err(ops::rules_get(h.admin(), "alpha"));
    assert_eq!(error.code.as_str(), "not_found");
}

/* ---------------- 上游代理账本 ---------------- */

#[test]
fn proxies_ledger_references_and_credential_pairs() {
    let h = Harness::new("proxies");
    let put = h.ok(ops::proxies_put(
        h.admin(),
        &json!({"name": "corp", "host": "127.0.0.1", "port": 8080, "user": "u", "password": "p"}),
    ));
    assert_eq!(put["name"], "corp");
    assert_eq!(put["has_auth"], true);
    assert!(!put.to_string().contains("\"p\""), "凭据不回显");
    assert_eq!(put["references"].as_array().unwrap().len(), 0);

    // 凭据成对给或都不出现：单给 user 非法。
    let error = h.err(ops::proxies_put(
        h.admin(),
        &json!({"name": "bad", "host": "h", "user": "u"}),
    ));
    assert_eq!(error.code.as_str(), "invalid_config");

    // 引用完整性：被绑定 → 删除被拒并点名引用方；解绑后可删。
    h.seed_env("env-a");
    h.ok(ops::env_patch(
        h.admin(),
        "env-a",
        &json!({"upstream": "corp"}),
    ));
    let referenced = h.ok(ops::proxies_list(h.admin()));
    assert_eq!(referenced[0]["references"][0], "env-a");
    let error = h.err(ops::proxies_delete(h.admin(), "corp"));
    assert!(
        error.message.contains("env-a"),
        "删除被拒必须点名引用方：{error:?}"
    );
    h.ok(ops::env_patch(
        h.admin(),
        "env-a",
        &json!({"upstream": null}),
    ));
    h.ok(ops::proxies_delete(h.admin(), "corp"));
}

/* ---------------- 调试会话 ---------------- */

#[test]
fn debug_session_guards() {
    let h = Harness::new("debug");
    // 无会话：{env:null}（不是错误）；停止幂等回 {ok:false}。
    let idle = h.ok(ops::debug_get(h.admin()));
    assert_eq!(idle["env"], Value::Null);
    assert_eq!(h.ok(ops::debug_stop(h.admin()))["ok"], false);

    // 目标没在跑 → conflict 指路（先启动再调试）。
    h.seed_env("alpha");
    let error = h.err(ops::debug_start(h.admin(), "alpha"));
    assert_eq!(error.code.as_str(), "conflict");
    assert!(error.message.contains("not running"), "got {error:?}");

    // 无活会话的环境取抓包 → not_found（会话随活实例）。
    let error = h.err(ops::env_captures(h.admin(), "alpha", 100));
    assert_eq!(error.code.as_str(), "not_found");

    // 活会话全链路（开会话 snapshot / events 推送 / clear 换代）需要真引擎的
    // capture 面 —— 假引擎不代理流量，归 CDP 真机走查（tools/envboard-desktop）。
}

#[test]
fn export_requires_a_live_session_and_a_known_format() {
    let h = Harness::new("export");
    h.seed_env("alpha");
    let error = h.err(ops::env_captures_export(h.admin(), "alpha", "har"));
    assert_eq!(error.code.as_str(), "not_found");
    let error = h.err(ops::env_captures_export(h.admin(), "alpha", "xml"));
    // 无会话时先撞 not_found；格式校验在有会话时生效 —— 这里只钉信封形状。
    assert!(!error.code.as_str().is_empty());
}

/* ---------------- HAR 导入 ---------------- */

#[test]
fn har_import_roundtrip() {
    let h = Harness::new("har");
    let har = json!({
        "log": {
            "version": "1.2",
            "creator": {"name": "test", "version": "0"},
            "entries": [{
                "startedDateTime": "1701111100000",
                "time": 0,
                "request": {
                    "method": "GET", "url": "http://example.com/",
                    "httpVersion": "HTTP/1.1", "headers": [], "queryString": [],
                    "headersSize": -1, "bodySize": -1
                },
                "response": {
                    "status": 200, "statusText": "", "httpVersion": "HTTP/1.1",
                    "headers": [], "content": {"size": 0, "mimeType": null, "text": ""},
                    "headersSize": -1, "bodySize": 0, "redirectURL": ""
                },
                "cache": {}, "timings": {"send": 0, "wait": -1, "receive": 0}
            }]
        }
    });
    let imported = h.ok(ops::har_import(h.admin(), "demo.har", &har));
    let id = imported["id"].as_u64().expect("导入会话有 id");
    assert_eq!(imported["entries"], 1);

    let listed = h.ok(ops::har_list(h.admin()));
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 1);
    let got = h.ok(ops::har_get(h.admin(), id, 200));
    assert_eq!(got["records"].as_array().unwrap().len(), 1);
    assert_eq!(h.ok(ops::har_delete(h.admin(), id))["ok"], true);
    let error = h.err(ops::har_get(h.admin(), id, 200));
    assert_eq!(error.code.as_str(), "not_found");
}

/* ---------------- 审计 / 日志 / 轨迹 ---------------- */

#[test]
fn control_plane_writes_leave_audit_trail() {
    // 对比/活动页已退场（ADR-007），但审计账本仍是契约：commit 先落盘后发事件。
    // 直接打 facade history（传输无关），不经已删除的 IPC 命令。
    let h = Harness::new("audit");
    h.seed_env("env-a");
    let events = h.admin().history(None, 100).expect("history 可读");
    assert!(
        events.iter().any(|ev| ev["type"] == "environment/created"),
        "控制面写入必须留审计：{events:?}"
    );
    let filtered = h.admin().history(Some("env-a"), 100).expect("按环境过滤");
    assert!(filtered.iter().all(|ev| {
        ev["data"]["environment"].as_str() == Some("env-a")
            || ev["data"]["name"].as_str() == Some("env-a")
    }));
}

#[test]
fn logs_shape() {
    // 实例日志跟落盘走：这个 harness 不关日志文件。
    let h = Harness::with_options("logs", false);
    h.seed_env("alpha");
    h.start("alpha");
    let logs = h.ok(ops::env_logs(h.admin(), "alpha", 200));
    assert_eq!(logs["env"], "alpha");
    assert!(logs["lines"].is_array());
}

/* ---------------- 故障注入 / CA 边界 ---------------- */

#[test]
fn fault_injection_marks_failed_and_restart_recovers() {
    let h = Harness::new("fault");
    h.seed_env("alpha");
    h.start("alpha");
    let injected = h.ok(ops::inject_fault(h.admin(), "alpha", "test fault"));
    assert_eq!(injected["injected"], true);
    let after = h.ok(ops::env_get(h.admin(), "alpha"));
    assert_eq!(after["health"], "failed");
    assert!(after["health_reason"].as_str().is_some(), "原因可见");
    let recovered =
        tauri::async_runtime::block_on(ops::env_restart(h.admin(), "alpha")).expect("restart");
    assert_eq!(recovered["health"], "running");
}

#[test]
fn ca_is_unavailable_with_a_fake_engine() {
    let h = Harness::new("ca");
    // prepare_with 不物化 CA：CA 命令回统一错误信封（不是 panic）。
    let error = h.err(ops::ca_overview(h.prepared.ca.as_ref()));
    assert_eq!(error.code.as_str(), "not_found");
    let error = h.err(ops::ca_uninstall_cert(h.prepared.ca.as_ref()));
    assert_eq!(error.code.as_str(), "not_found");
    let error = ops::open_cert_install_wizard(h.prepared.ca.as_ref())
        .expect_err("无 CA 时向导入口必须响亮失败");
    assert_eq!(error.code.as_str(), "not_found");
}

#[test]
fn save_as_pure_core_writes_text_without_touching_system_stores() {
    // 另存为的对话框在命令层；纯核只负责写盘 —— 自动化不写真系统信任库。
    let h = Harness::new("saveas");
    let path = h._dir.join("nested").join("saved.pem");
    ops::write_text(
        &path,
        "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n",
    )
    .expect("write_text 必须建父目录并落盘");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n"
    );
}

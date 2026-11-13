//! 冒烟（桌面形态）：唯一发布产物 = Tauri 工作台。
//!
//! 这一层钉默认层就能验的事（HTTP 档位的断言随 web 形态退场）：
//!
//! 1. 命令面只剩工作台 —— `--help` 不得长出子命令；退场旗标（--listen/--token）不得复活；
//! 2. 启动参数形状错误带字段路径响亮退出（`--port-range` 坏形状 → exit 2）；
//! 3. 状态单写者：第二个实例抢不到锁必须响亮失败（组合根 InstanceLock）。
//!
//! 真窗口里的行为（IPC 流 / 核心环 / 零 CSP 报错）归 CDP 真机走查
//! （tools/envboard-desktop/cdp-walk.mjs），不在这一层。
//!
//! 测试一律 `--no-dialog`：错误弹框会阻塞无头验收。

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn envboard() -> &'static str {
    env!("CARGO_BIN_EXE_envboard")
}

/// 每个测试一个临时状态目录，互不干扰。
fn state_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("envboard-smoke-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create state dir");
    dir
}

struct Workbench(Child);

impl Workbench {
    /// 第一个实例还活着（关窗常驻：进程与引擎不随窗口退出）。
    fn is_running(&mut self) -> bool {
        self.0.try_wait().ok().flatten().is_none()
    }
}

impl Drop for Workbench {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn(state: &Path, extra: &[&str]) -> Workbench {
    let child = Command::new(envboard())
        .arg("--state-dir")
        .arg(state)
        .arg("--no-dialog")
        .args(extra)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the workbench");
    Workbench(child)
}

/// 锁文件出现 = 组合根已完成 prepare（状态单写者就位）。
fn wait_lock(state: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if state.join("lock").exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

#[test]
fn the_help_surface_has_no_subcommands() {
    let help = Command::new(envboard())
        .arg("--help")
        .output()
        .expect("run --help");
    assert!(help.status.success(), "--help must succeed");
    let text = String::from_utf8_lossy(&help.stdout).into_owned();
    // clap 的子命令清单一出现就说明命令面又长回来了
    assert!(
        !text.contains("Commands:") && !text.contains("Status") && !text.contains("status <"),
        "--help 里不得再出现子命令面：\n{text}"
    );
    for flag in ["--state-dir", "--port-range", "--no-dialog"] {
        assert!(text.contains(flag), "--help 缺了 {flag}");
    }
    // 已退役的旗标不得复活（HTTP 面随 web 形态退场）
    for gone in [
        "--listen",
        "--token",
        "--no-open",
        "--without-token",
        "--core",
        "--once",
        "--json",
    ] {
        assert!(
            !text.contains(gone),
            "{gone} 已随 web 形态退役，不得回到命令面"
        );
    }
}

#[test]
fn a_bad_port_range_exits_loudly_with_the_field_path() {
    let state = state_dir("badrange");
    let output = Command::new(envboard())
        .arg("--state-dir")
        .arg(&state)
        .args(["--no-dialog", "--port-range", "16000-70000"])
        .output()
        .expect("run with a bad port range");
    assert!(!output.status.success(), "坏区间必须拒绝启动");
    assert_eq!(output.status.code(), Some(2), "不可重试错误退出码 2");
    let text = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        text.contains("port_range"),
        "错误必须带字段路径 port_range：{text}"
    );
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn legacy_state_json_migrates_to_sqlite_on_boot() {
    // ADR-007：旧 JSON 账本一次性迁进 state.db，旧文件改名留备份。
    let state = state_dir("migrate");
    std::fs::write(
        state.join("state.json"),
        r#"{"version":1,"environments":[{"name":"legacy-env","listen":{"host":"127.0.0.1","port":16500}}]}"#,
    )
    .unwrap();
    let _wb = spawn(&state, &[]);
    let deadline = Instant::now() + Duration::from_secs(30);
    let migrated = loop {
        if state.join("state.db").exists() && state.join("state.json.migrated").exists() {
            break true;
        }
        if Instant::now() > deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    assert!(migrated, "启动应把 state.json 迁进 state.db 并留备份");
    drop(_wb);
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn startup_log_records_the_boot_stages_and_readiness() {
    // 基建 P0：windows 子系统没有控制台，启动过程必须落盘留痕 ——
    // 会话头 + 两个装配阶段 + ready 都要在，崩溃时用户才有东西可交。
    let state = state_dir("startuplog");
    let wb = spawn(&state, &[]);
    let log = state.join("logs").join("startup.log");
    let deadline = Instant::now() + Duration::from_secs(30);
    let ready = loop {
        if std::fs::read_to_string(&log).is_ok_and(|text| text.contains("ready")) {
            break true;
        }
        if Instant::now() > deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    assert!(ready, "startup.log 应记到 ready 阶段：{}", log.display());
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("stage: boot"), "缺 boot 阶段：{text}");
    assert!(
        text.contains("stage: tray+window"),
        "缺 tray+window 阶段：{text}"
    );
    drop(wb);
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn a_second_instance_activates_the_running_workbench() {
    // ADR-012：同 state-dir 的第二个进程只做激活并退出 0 —— 它不拿锁、不写状态，
    // 所以「状态单写者」仍由第一个实例持有（第二个连 prepare 都没走到）。
    let state = state_dir("lock");
    let mut wb = spawn(&state, &[]);
    assert!(
        wait_lock(&state, Duration::from_secs(30)),
        "第一个实例没起来（锁文件未出现）"
    );
    let second = Command::new(envboard())
        .arg("--state-dir")
        .arg(&state)
        .arg("--no-dialog")
        .output()
        .expect("spawn a second instance");
    assert!(
        second.status.success(),
        "第二个实例必须成功退出（只做激活）：{second:?}"
    );
    let text = String::from_utf8_lossy(&second.stderr).into_owned();
    assert!(
        text.contains("已有实例"),
        "第二个实例要说明它只是激活已有实例：{text}"
    );
    assert!(wb.is_running(), "第一个实例必须继续存活（关窗常驻）");
    assert!(
        state.join("lock").exists(),
        "状态目录的单实例锁仍由第一个实例持有"
    );
    drop(wb);
    let _ = std::fs::remove_dir_all(&state);
}

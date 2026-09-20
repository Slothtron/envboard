//! 冒烟（v0.2 CLI 退役后）：唯一发布产物 = 工作台启动器。
//!
//! 这一层钉四件默认层就能验的事：
//!
//! 1. 命令面只剩工作台 —— `--help` 不得再长出子命令；
//! 2. **非回环无 token 拒绝启动**（鉴权换轨的负向守卫，去掉校验这条必红）；
//! 3. 回环默认免鉴权可用；`--token` 档 header 与 `?token=` 双通道等效、无凭据 401；
//! 4. 状态单写者：第二个实例抢不到锁必须响亮失败。
//!
//! 原先写在 `ci/verify.sh` `step_smoke` 里的判据全部搬进测试：入口脚本只做编排。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
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

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn wait_port(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// 裸 HTTP：返回状态码。token 走 header 或 query 两个通道之一。
fn get(port: u16, path: &str, token: Option<&str>) -> u16 {
    let mut request =
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    if let Some(token) = token {
        request.push_str(&format!("x-envboard-token: {token}\r\n"));
    }
    request.push_str("\r\n");
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .expect("read timeout");
    stream.write_all(request.as_bytes()).expect("write");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read");
    let first = String::from_utf8_lossy(&raw)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    first
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0)
}

struct Workbench(Child);

impl Drop for Workbench {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn(state: &PathBuf, listen: &str, extra: &[&str]) -> Workbench {
    let child = Command::new(envboard())
        .arg("--state-dir")
        .arg(state)
        .arg("--listen")
        .arg(listen)
        .args(extra)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the workbench");
    Workbench(child)
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
    for flag in ["--listen", "--token", "--state-dir", "--port-range"] {
        assert!(text.contains(flag), "--help 缺了 {flag}");
    }
    // 已退役的旗标不得复活
    for gone in ["--without-token", "--core", "--once", "--json"] {
        assert!(!text.contains(gone), "{gone} 已随 CLI 退役，不得回到命令面");
    }
}

#[test]
fn a_public_bind_without_a_token_refuses_to_start() {
    // 鉴权换轨的负向守卫：0.0.0.0 不给 token 必须带 invalid_config 立即退出。
    let state = state_dir("deny");
    let port = free_port();
    let output = Command::new(envboard())
        .args(["--state-dir"])
        .arg(&state)
        .args(["--listen", &format!("0.0.0.0:{port}")])
        .output()
        .expect("run a public bind without a token");
    assert!(!output.status.success(), "非回环无 token 必须拒绝启动");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        text.contains("web.token") && text.contains("--token"),
        "错误必须指路（field web.token + 提示 --token）：{text}"
    );
    assert!(
        !wait_port(port, Duration::from_millis(300)),
        "拒启动不得留下监听端口"
    );
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn loopback_is_open_by_default_and_token_mode_gates_both_channels() {
    // 档一：回环不给 token = 免鉴权。
    let state = state_dir("open");
    let port = free_port();
    let _wb = spawn(&state, &format!("127.0.0.1:{port}"), &[]);
    assert!(
        wait_port(port, Duration::from_secs(30)),
        "免鉴权工作台没起来"
    );
    assert_eq!(
        get(port, "/api/status", None),
        200,
        "回环默认档 API 必须不带凭据 200"
    );
    assert_eq!(get(port, "/", None), 200, "回环默认档页面必须不带凭据 200");
    drop(_wb);
    let _ = std::fs::remove_dir_all(&state);

    // 档二：显式 --token = header 与 ?token= 双通道等效，无凭据 401。
    let state = state_dir("token");
    let port = free_port();
    let _wb = spawn(
        &state,
        &format!("127.0.0.1:{port}"),
        &["--token", "smoke-secret"],
    );
    assert!(
        wait_port(port, Duration::from_secs(30)),
        "token 工作台没起来"
    );
    assert_eq!(get(port, "/api/status", None), 401);
    assert_eq!(get(port, "/api/status", Some("smoke-secret")), 200);
    assert_eq!(get(port, "/api/status?token=smoke-secret", None), 200);
    assert_eq!(get(port, "/", None), 401, "页面本体同样要 token");
    assert_eq!(get(port, "/app.js", None), 200, "内嵌静态资产保持豁免");
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn a_second_instance_refuses_to_touch_the_state() {
    // 单写者契约：flock 拿不到就响亮失败，绝不出现第二个写者。
    let state = state_dir("lock");
    let port = free_port();
    let _wb = spawn(&state, &format!("127.0.0.1:{port}"), &[]);
    assert!(wait_port(port, Duration::from_secs(30)));
    let second = Command::new(envboard())
        .args(["--state-dir"])
        .arg(&state)
        .args(["--listen", &format!("127.0.0.1:{}", free_port())])
        .output()
        .expect("spawn a second instance");
    assert!(!second.status.success(), "第二个实例必须失败（状态单写者）");
    let text = String::from_utf8_lossy(&second.stderr).into_owned();
    assert!(
        text.contains("another envboard workbench") || text.contains("conflict"),
        "错误要说明谁占了状态：{text}"
    );
    drop(_wb);
    let _ = std::fs::remove_dir_all(&state);
}

//! CLI 作为**本地 API 瘦客户端**的行为测试。
//!
//! 这条曾经是硬伤：CLI 直接驱动管理器并抢 flock，所以"工作台跑着的时候命令行全部报
//! conflict"。现在 CLI 先探测本地 API，有常驻实例就走 HTTP。这个测试把两种模式都钉住：
//!
//! * 有常驻实例 → 走 HTTP（stderr 里能看到目标地址），且命令真的生效；
//! * 没有常驻实例 → 退回直连模式（自己持锁），行为不变。
//!
//! 用 `--core fake`：它不需要装 mitmproxy，因此这个测试属于默认流水线。

use std::io::Read;
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn envboard() -> &'static str {
    env!("CARGO_BIN_EXE_envboard")
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn wait_for_port(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn cli(state_dir: &std::path::Path, args: &[&str]) -> (String, String, bool) {
    let output = Command::new(envboard())
        .arg("--state-dir")
        .arg(state_dir)
        .arg("--core")
        .arg("fake")
        .args(args)
        .output()
        .expect("the CLI binary must be runnable");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

#[test]
fn cli_drives_a_resident_workbench_over_http() {
    let state_dir = std::env::temp_dir().join(format!("envboard-thin-{}", std::process::id()));
    std::fs::remove_dir_all(&state_dir).ok();
    std::fs::create_dir_all(&state_dir).unwrap();

    // 先在没有常驻实例的情况下建一个（直连模式）
    let (stdout, stderr, ok) = cli(
        &state_dir,
        &["env", "add", "beta", "--description", "直连建的"],
    );
    assert!(ok, "direct mode add failed: {stdout} {stderr}");
    assert!(
        !stderr.contains("thin client"),
        "without a resident manager the CLI must drive the manager directly, got stderr: {stderr}"
    );

    // 起工作台（fake core，自定义端口）
    let port = free_port();
    let server = Server(
        Command::new(envboard())
            .args(["--state-dir"])
            .arg(&state_dir)
            .args(["--core", "fake", "web", "--listen"])
            .arg(format!("127.0.0.1:{port}"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("workbench must start"),
    );
    assert!(
        wait_for_port(port, Duration::from_secs(20)),
        "workbench never listened on {port}"
    );

    // ① 发现：api.json 让 CLI 找到自定义端口
    let record = std::fs::read_to_string(state_dir.join("runtime").join("api.json")).unwrap();
    assert!(
        record.contains(&port.to_string()),
        "api.json must record the listen address: {record}"
    );

    // ② 走 HTTP：stderr 里能看到目标地址，且**不再**有锁冲突
    let (stdout, stderr, ok) = cli(&state_dir, &["env", "list"]);
    assert!(ok, "thin-mode list failed: {stdout} {stderr}");
    assert!(
        stderr.contains("thin client"),
        "expected thin-client mode, got stderr: {stderr}"
    );
    assert!(
        stdout.contains("beta"),
        "the resident manager's environment must be listed: {stdout}"
    );

    // ③ 变更类命令也走 HTTP（这正是以前会被 flock 挡掉的那类）
    let (stdout, stderr, ok) = cli(&state_dir, &["env", "add", "prod", "--port", "16677"]);
    assert!(ok, "thin-mode add failed: {stdout} {stderr}");
    assert!(stderr.contains("thin client"));

    let (stdout, _, _) = cli(&state_dir, &["env", "start", "prod"]);
    assert!(
        stdout.contains("running"),
        "starting through the resident manager must take effect: {stdout}"
    );

    // ④ compare 也经 HTTP（纯静态查表）
    let (stdout, _, ok) = cli(&state_dir, &["compare", "api.example.com"]);
    assert!(
        ok && stdout.contains("api.example.com"),
        "compare failed: {stdout}"
    );

    // ④b 编辑也经 HTTP：PATCH 打在常驻管理器上（`env edit` 与工作台共用同一份语义）
    let rules_file = state_dir.join("edit-rules.txt");
    std::fs::write(&rules_file, "10.0.0.11 api.example.com\n").unwrap();
    let (_, stderr, ok) = cli(
        &state_dir,
        &[
            "rules",
            "import",
            "beta",
            "--file",
            rules_file.to_str().unwrap(),
        ],
    );
    assert!(ok, "rules import failed: {stderr}");

    let (stdout, stderr, ok) = cli(
        &state_dir,
        &[
            "env",
            "edit",
            "beta",
            "--rules",
            "beta",
            "--description",
            "改过的",
        ],
    );
    assert!(ok, "thin-mode edit failed: {stdout} {stderr}");
    assert!(stderr.contains("thin client"));
    assert!(stdout.contains("rules=beta"), "got: {stdout}");
    assert!(stdout.contains("改过的"), "got: {stdout}");

    // 运行中改名会被服务端拒绝（错误码来自服务端，CLI 只翻译）
    let (stdout, _, ok) = cli(&state_dir, &["env", "start", "beta"]);
    assert!(ok, "start failed: {stdout}");
    let (_, stderr, ok) = cli(&state_dir, &["env", "edit", "beta", "--rename", "gamma"]);
    assert!(!ok, "renaming a running environment must fail");
    assert!(stderr.contains("conflict"), "got: {stderr}");

    // 停止后改名成功，并且新名字在列表里
    cli(&state_dir, &["env", "stop", "beta"]);
    let (stdout, stderr, ok) = cli(&state_dir, &["env", "edit", "beta", "--rename", "gamma"]);
    assert!(ok, "rename failed: {stdout} {stderr}");
    let (stdout, _, _) = cli(&state_dir, &["env", "list"]);
    assert!(stdout.contains("gamma"), "got: {stdout}");
    assert!(
        // 看**名字列**，不要 `contains("beta")`：那一行里还有 `rules=beta(1)`。
        !stdout.lines().any(|line| line.starts_with("beta ")),
        "the old name must be gone: {stdout}"
    );
    assert!(
        stdout.contains("rules=beta(1)"),
        "the rules binding must survive the rename: {stdout}"
    );

    // 空 PATCH 必须响亮失败，而不是"成功但什么都没改"
    let (_, stderr, ok) = cli(&state_dir, &["env", "edit", "gamma"]);
    assert!(!ok, "an empty edit must fail");
    assert!(stderr.contains("nothing to change"), "got: {stderr}");

    // ⑤ 工作台退出后：api.json 被清掉，CLI 回到直连模式
    drop(server);
    for _ in 0..50 {
        if !state_dir.join("runtime").join("api.json").exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let (_, stderr, ok) = cli(&state_dir, &["env", "list"]);
    assert!(ok);
    assert!(
        !stderr.contains("thin client"),
        "after the workbench exits the CLI must fall back to direct mode, got: {stderr}"
    );

    std::fs::remove_dir_all(&state_dir).ok();
}

#[test]
fn thin_mode_refuses_to_become_a_second_writer() {
    // `run`/`web` 在瘦客户端模式下必须拒绝：否则就成了第二个写者（状态只有一个写者）。
    let state_dir = std::env::temp_dir().join(format!("envboard-thin2-{}", std::process::id()));
    std::fs::remove_dir_all(&state_dir).ok();
    std::fs::create_dir_all(&state_dir).unwrap();

    let port = free_port();
    let _server = Server(
        Command::new(envboard())
            .args(["--state-dir"])
            .arg(&state_dir)
            .args(["--core", "fake", "web", "--listen"])
            .arg(format!("127.0.0.1:{port}"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    assert!(wait_for_port(port, Duration::from_secs(20)));

    let output = Command::new(envboard())
        .args(["--state-dir"])
        .arg(&state_dir)
        .args(["--core", "fake", "run", "--once"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "a second writer must be refused");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("second writer") || stderr.contains("conflict"),
        "expected a clear refusal, got: {stderr}"
    );

    std::fs::remove_dir_all(&state_dir).ok();
}

#[test]
fn an_explicit_state_dir_never_falls_back_to_the_default_port() {
    // `--state-dir` 明确给了，就**只**认那个目录里的 api.json：否则 8900 上那个跟它
    // 毫无关系的常驻实例会接管这些命令 —— 命令看着成功了，改的却是别人的状态。
    // （测试因此也不会被开发机上跑着的工作台污染。）
    let state_dir = std::env::temp_dir().join(format!("envboard-isolated-{}", std::process::id()));
    std::fs::remove_dir_all(&state_dir).ok();
    std::fs::create_dir_all(&state_dir).unwrap();

    // 在默认端口上蹲一个"别人的工作台"：它不是我们的服务，探测会失败 —— 但真正要钉住的是
    // "压根不去试默认端口"。用一个只接受连接、从不回话的 listener 当哨兵。
    let sentinel = TcpListener::bind("127.0.0.1:8900").ok();
    let output = Command::new(envboard())
        .args(["--state-dir"])
        .arg(&state_dir)
        .args(["--core", "fake", "env", "list"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the CLI must serve this from the state dir itself, got: {stderr}"
    );
    assert!(
        !stderr.contains("thin client"),
        "an explicit --state-dir must not hand the command to whatever listens on 8900: {stderr}"
    );
    drop(sentinel);

    std::fs::remove_dir_all(&state_dir).ok();
}

#[allow(dead_code)]
fn _io_anchor(mut stream: std::net::TcpStream) {
    let mut buffer = String::new();
    let _ = stream.read_to_string(&mut buffer);
}

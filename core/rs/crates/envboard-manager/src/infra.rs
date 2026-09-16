//! 端口 trait 的**真实实现**（第四层：`infra`）—— 只有这里碰 socket、`/proc`、系统时间。
//!
//! 放在 `envboard-manager` 里而不是单独 crate：逻辑与它的端口实现一起演进，
//! 而"可测性"由 trait 边界保证（测试用 [`crate::ports`] 里的内存替身），
//! 不靠拆 crate：Rust 布局里没有 `infra` crate，这里按模块落地。

use std::net::{IpAddr, SocketAddr, TcpListener};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use envboard_core_api::{ClockPort, Error, ErrorCode, LogLevel, LoggerPort, ProcessIdentity};

use crate::ports::{FilePort, PortProbe, ProcessTable};

/// 用"试绑"判断端口是否空闲。
///
/// **两条语义必须与实例一致**（`core/spec/capabilities.md` §端口分配第 5 条）：
///
/// 1. 绑定地址就用实例的 `listen.host`（默认 `127.0.0.1`）—— 用 `0.0.0.0` 试绑会推出
///    不同的结论；
/// 2. **不设 `SO_REUSEADDR`**：本实现走 `std::net::TcpListener::bind`，标准库在 Unix 上
///    不设该选项；`tokio::net::TcpListener::bind` 会设，所以**不要**图省事换成 tokio 的。
///    设了它之后，"已被 0.0.0.0 上某个通配监听占用的端口"会绑定成功，被误判为空闲，
///    而实例真启动时才失败 —— 那正是最费解的一类故障。
///
/// 试绑成功即视为空闲并**立刻释放**（不持有到实例启动）。试绑与真正监听之间的
/// TOCTOU 窗口无法消除，由"启动失败 → 换端口重试一次 / 标 port_conflict"兜住。
#[derive(Debug, Default, Clone, Copy)]
pub struct SocketPortProbe;

impl PortProbe for SocketPortProbe {
    fn is_free(&self, host: IpAddr, port: u16) -> bool {
        TcpListener::bind(SocketAddr::new(host, port)).is_ok()
    }
}

/// 系统时钟。
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl ClockPort for SystemClock {
    fn now_unix(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// 写到 stderr 的日志实现（工作台/CLI 可以换成别家）。
#[derive(Debug, Default, Clone, Copy)]
pub struct StderrLogger;

impl LoggerPort for StderrLogger {
    fn log(&self, level: LogLevel, message: &str) {
        eprintln!("[{}] {message}", level.as_str());
    }
}

/// 真实文件系统（只用到"存在"与"读"两件事，见 [`FilePort`]）。
#[derive(Debug, Default, Clone, Copy)]
pub struct RealFiles;

impl FilePort for RealFiles {
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn read_to_string(&self, path: &Path) -> Result<String, Error> {
        std::fs::read_to_string(path).map_err(|error| {
            Error::new(
                ErrorCode::NotFound,
                format!("cannot read {}: {error}", path.display()),
            )
        })
    }
}

/// Linux `/proc` 进程表 —— 孤儿清理的判据来源。
///
/// `ProcessIdentity` 里带**启动时刻**不是装饰：cmdline 对同一份配置的实例完全相同，
/// 只比 PID + cmdline 会在 PID 复用后误杀无关进程。
#[derive(Debug, Default, Clone, Copy)]
pub struct ProcfsProcessTable;

impl ProcfsProcessTable {
    /// `/proc/<pid>/stat` 的第 3 个字段（state）与第 22 个字段（starttime，单位 jiffies）。
    ///
    /// 解析要小心：第 2 个字段是 `(comm)`，**comm 里可以含空格与括号**，
    /// 所以必须从**最后一个** `)` 之后开始数字段。
    fn stat_fields(pid: i32) -> Option<(char, u64)> {
        let raw = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after_comm = &raw[raw.rfind(')')? + 1..];
        let mut fields = after_comm.split_whitespace();
        // `)` 之后的第 1 项是 state（整体第 3 个字段），剩下的从 ppid（整体第 4 个）开始，
        // 因此 starttime 的偏移是 22 - 4。
        let state = fields.next()?.chars().next()?;
        let starttime = fields.nth(18)?.parse().ok()?;
        Some((state, starttime))
    }

    fn cmdline(pid: i32) -> Vec<String> {
        let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
            return Vec::new();
        };
        raw.split(|byte| *byte == 0)
            .filter(|chunk| !chunk.is_empty())
            .map(|chunk| String::from_utf8_lossy(chunk).to_string())
            .collect()
    }
}

impl ProcessTable for ProcfsProcessTable {
    fn identity(&self, pid: i32) -> Option<ProcessIdentity> {
        let (state, starttime) = Self::stat_fields(pid)?;
        // 僵尸（Z）与正在消亡（X/x）**不算活着**：它们的 /proc 条目还在、starttime 也没变，
        // 只看这两个量会把一个已经死掉、只等父进程回收的实例判成"在跑"。
        // 实测：`kill -9` 掉实例后工作台会一直报 health=running（见 core/spec/capabilities.md）。
        if matches!(state, 'Z' | 'X' | 'x') {
            return None;
        }
        Some(ProcessIdentity {
            pid,
            starttime,
            cmdline: Self::cmdline(pid),
        })
    }

    fn terminate(&self, identity: &ProcessIdentity) -> Result<(), Error> {
        // 先温和。调用方应当把它放在 spawn_blocking 里 —— 这里会短暂阻塞。
        //
        // SAFETY: `kill` 只读取入参；pid 来自调用方，且调用方已用
        // `identity()` 校验过（PID + 启动时刻 + cmdline 三者匹配）才走到这里。
        unsafe { libc::kill(identity.pid, libc::SIGTERM) };

        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(50));
            if self.identity(identity.pid).as_ref() != Some(identity) {
                return Ok(());
            }
        }

        // SAFETY: 同上。到这一步已经等满 2 秒，退化成强杀。
        unsafe { libc::kill(identity.pid, libc::SIGKILL) };
        std::thread::sleep(Duration::from_millis(50));
        if self.identity(identity.pid).as_ref() == Some(identity) {
            return Err(Error::new(
                ErrorCode::InternalError,
                format!("process {} survived SIGKILL", identity.pid),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn probe_sees_a_bound_port_as_occupied() {
        let probe = SocketPortProbe;
        // 先占一个端口：用标准库绑定并**保持**监听
        let listener =
            TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(!probe.is_free(IpAddr::V4(Ipv4Addr::LOCALHOST), port));
        drop(listener);
        assert!(probe.is_free(IpAddr::V4(Ipv4Addr::LOCALHOST), port));
    }

    #[test]
    fn probe_catches_a_wildcard_listener() {
        // 这条是 `SO_REUSEADDR` 那个坑的守门测试：别的程序在 0.0.0.0 上监听时，
        // 具体地址上的试绑**必须**失败。若哪天有人把实现换成 tokio 的 bind
        //（默认打开 SO_REUSEADDR），这条会红。
        let probe = SocketPortProbe;
        let wildcard = TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).unwrap();
        let port = wildcard.local_addr().unwrap().port();
        assert!(
            !probe.is_free(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
            "a wildcard listener must make the specific-address probe fail"
        );
        drop(wildcard);
    }

    #[test]
    fn current_process_has_an_identity() {
        let table = ProcfsProcessTable;
        let identity = table
            .identity(std::process::id() as i32)
            .expect("self must be readable");
        assert_eq!(identity.pid, std::process::id() as i32);
        assert!(identity.starttime > 0);
        assert!(!identity.cmdline.is_empty());
    }

    #[test]
    fn unknown_pid_has_no_identity() {
        // pid 上限之外的值必然不存在
        assert!(ProcfsProcessTable.identity(i32::MAX).is_none());
    }

    #[test]
    fn a_zombie_is_not_alive() {
        // 故意**不** wait()：子进程退出后成为僵尸，/proc 条目仍在且 starttime 未变。
        // 只看 pid + starttime 的判活会把这种情况误判成"在跑"，所以这里锁住状态位判据。
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .spawn()
            .expect("spawn sh");
        let pid = child.id() as i32;
        std::thread::sleep(Duration::from_millis(500));

        assert!(
            ProcfsProcessTable.identity(pid).is_none(),
            "a zombie must not be reported as a live process"
        );
        // 证明它确实只是僵尸（而不是被回收/根本没起来）
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).expect("zombie has /proc");
        let after_comm = &stat[stat.rfind(')').expect("comm") + 1..];
        assert_eq!(
            after_comm.split_whitespace().next(),
            Some("Z"),
            "the child should still be a zombie here"
        );

        let _ = child.wait();
    }

    #[test]
    fn a_live_process_has_an_identity() {
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id() as i32;
        std::thread::sleep(Duration::from_millis(200));

        let identity = ProcfsProcessTable
            .identity(pid)
            .expect("a running process must have an identity");
        assert_eq!(identity.pid, pid);
        assert!(identity.starttime > 0);
        assert!(
            identity.cmdline.iter().any(|arg| arg == "sleep"),
            "cmdline should be readable for a live process: {:?}",
            identity.cmdline
        );

        let _ = child.kill();
        let _ = child.wait();
    }
}

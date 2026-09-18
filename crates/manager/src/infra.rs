//! 端口 trait 的**真实实现**（第四层：`infra`）—— 只有这里碰 socket、`/proc`、系统时间。
//!
//! 放在 `envboard-manager` 里而不是单独 crate：逻辑与它的端口实现一起演进，
//! 而"可测性"由 trait 边界保证（测试用 [`crate::ports`] 里的内存替身），
//! 不靠拆 crate：Rust 布局里没有 `infra` crate，这里按模块落地。

use std::net::{IpAddr, SocketAddr, TcpListener};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use envboard_engine::{ClockPort, Error, ErrorCode, LogLevel, LoggerPort};

use crate::ports::{EventStorePort, FilePort, PortProbe};

/// 用"试绑"判断端口是否空闲。
///
/// **两条语义必须与实例一致**（`spec/capabilities.md` §端口分配第 5 条）：
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

/// 真实事件存储：append-only JSONL，0600，open-write-close（无长持 fd，
/// rename 轮转安全）。
#[derive(Debug, Default, Clone, Copy)]
pub struct RealEventStore;

impl EventStorePort for RealEventStore {
    fn append(&self, path: &Path, line: String) -> Result<(), Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                Error::new(
                    ErrorCode::InternalError,
                    format!("cannot create {}: {error}", parent.display()),
                )
            })?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|error| {
                Error::new(
                    ErrorCode::InternalError,
                    format!("cannot append {}: {error}", path.display()),
                )
            })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
        }
        let mut file = file;
        use std::io::Write as _;
        file.write_all(line.as_bytes()).map_err(|error| {
            Error::new(
                ErrorCode::InternalError,
                format!("cannot append {}: {error}", path.display()),
            )
        })
    }

    fn size(&self, path: &Path) -> Option<u64> {
        std::fs::metadata(path).ok().map(|meta| meta.len())
    }

    fn rotate(&self, path: &Path) -> Result<(), Error> {
        let rotated = path.with_extension("jsonl.1");
        std::fs::rename(path, &rotated).map_err(|error| {
            Error::new(
                ErrorCode::InternalError,
                format!("cannot rotate {}: {error}", path.display()),
            )
        })
    }

    fn read_from(&self, path: &Path, offset: u64) -> Result<Option<(u64, String)>, Error> {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut file) = std::fs::File::open(path) else {
            return Ok(None);
        };
        let len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        let offset = offset.min(len);
        file.seek(SeekFrom::Start(offset)).map_err(|error| {
            Error::new(ErrorCode::InternalError, format!("seek failed: {error}"))
        })?;
        let budget = (len - offset).min(256 * 1024);
        let mut text = String::new();
        file.take(budget)
            .read_to_string(&mut text)
            .map_err(|error| {
                Error::new(ErrorCode::InternalError, format!("read failed: {error}"))
            })?;
        // 半行截断：只交付完整行，残端留给下一次读。
        let complete = text.rfind('\n').map(|at| at + 1).unwrap_or(0);
        text.truncate(complete);
        Ok(Some((offset + complete as u64, text)))
    }

    fn read_tail(&self, path: &Path, max_bytes: u64) -> Result<String, Error> {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut file) = std::fs::File::open(path) else {
            return Ok(String::new());
        };
        let len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        let start = len.saturating_sub(max_bytes);
        file.seek(SeekFrom::Start(start)).map_err(|error| {
            Error::new(ErrorCode::InternalError, format!("seek failed: {error}"))
        })?;
        let mut text = String::new();
        file.take(max_bytes)
            .read_to_string(&mut text)
            .map_err(|error| {
                Error::new(ErrorCode::InternalError, format!("read failed: {error}"))
            })?;
        // 半行截断：从第一个完整换行之后开始。
        if start > 0
            && let Some(at) = text.find('\n')
        {
            text.drain(..=at);
        }
        Ok(text)
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
}

//! 启动诊断（基建 P0）：`startup.log` + 全局 panic hook + 原生错误框。
//!
//! 发布构建是 windows 子系统、没有控制台：Tauri 装配之前/之中的早期失败如果不
//! 落盘、不弹框，用户看到的就是「双击了没反应」。本模块提供三条兜底出口：
//!
//! 1. `<state_dir>/logs/startup.log`（`--log-dir` 可改）：会话头带版本 / pid /
//!    形态，阶段逐行追加，超 1 MiB 轮转一代（`startup.log.1`）；
//! 2. 全局 panic hook：panic 现场（线程 / 位置 / payload / backtrace）落盘并
//!    flush，再交回原 hook —— stderr 行为不变；
//! 3. 原生 `MessageBoxW`：Tauri 事件循环不可用（build 失败 / setup panic）时
//!    唯一可见的出口，框里带日志路径。
//!
//! 诊断自身绝不挡启动：任何 IO 失败都静默降级（只剩 stderr）。`startup.log`
//! 是壳级诊断，与每环境实例日志是两回事，不受 `--no-log-file` 影响 ——
//! 崩溃现场是最后一条线索，不能随「不要实例日志」一起关掉。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// 进程级日志槽：state-dir 解析成功后由 [`open`] 填入。panic hook 可能在任意
/// 线程开火，所以用锁；中毒也取回内值继续写（panic 不能被上一个 panic 哑掉）。
static LOG: Mutex<Option<Log>> = Mutex::new(None);

/// 轮转阈值：startup.log 跨会话追加，只留一代历史。
const MAX_BYTES: u64 = 1024 * 1024;

struct Log {
    path: PathBuf,
    file: File,
}

/// 开一个新会话：在 `<logs_dir>/startup.log` 写会话头并接管全局日志槽。
/// 返回日志路径（错误框里给用户指路）；失败返回 `None`，启动照常。
pub fn open(logs_dir: &Path, session: &str) -> Option<PathBuf> {
    let path = logs_dir.join("startup.log");
    let file = open_rotated(&path)?;
    let mut log = Log {
        path: path.clone(),
        file,
    };
    let _ = writeln!(log.file, "--- {} {session} ---", timestamp());
    let _ = log.file.flush();
    *lock() = Some(log);
    Some(path)
}

/// 记一行阶段信息：stderr 与日志各一份（debug 构建 / 终端拉起时同样可读）。
pub fn note(message: &str) {
    eprintln!("envboard: {message}");
    append(&format!("{} {message}", timestamp()));
}

/// 当前日志路径（错误框引用）；还没开槽时 `None`。
pub fn path() -> Option<PathBuf> {
    lock().as_ref().map(|log| log.path.clone())
}

/// 退出前 flush（`RunEvent::Exit`）。
pub fn flush() {
    if let Some(log) = lock().as_mut() {
        let _ = log.file.flush();
    }
}

/// 全局 panic hook：现场落盘 + flush，然后交回原 hook。只装一次（`run` 入口）。
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("<non-string panic payload>");
        let location = info
            .location()
            .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
            .unwrap_or_else(|| "<unknown location>".to_owned());
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");
        append(&format!(
            "{} PANIC thread={name} at {location}: {message}",
            timestamp()
        ));
        append(&format!(
            "backtrace:\n{}",
            std::backtrace::Backtrace::force_capture()
        ));
        previous(info);
    }));
}

/// 原生错误框：不依赖 Tauri 事件循环，build 失败 / setup panic 后仍能弹。
/// 阻塞到用户点掉 —— 调用方随后就退出进程，不会卡住事件循环。
#[cfg(windows)]
pub fn fatal_box(title: &str, message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, MessageBoxW,
    };

    let text: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    let caption: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: 两个宽字符串在调用返回前一直有效；null hwnd = 无属主窗口。
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST,
        );
    }
}

/// 非 Windows：没有原生框，退回 stderr（该形态本就有控制台）。
#[cfg(not(windows))]
pub fn fatal_box(title: &str, message: &str) {
    eprintln!("envboard: {title}: {message}");
}

fn append(line: &str) {
    if let Some(log) = lock().as_mut() {
        let _ = writeln!(log.file, "{line}");
        let _ = log.file.flush();
    }
}

fn lock() -> std::sync::MutexGuard<'static, Option<Log>> {
    LOG.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 追加打开；超阈值先轮转成 `startup.log.1`（Windows 的 rename 不覆盖，先删）。
fn open_rotated(path: &Path) -> Option<File> {
    if std::fs::metadata(path).is_ok_and(|meta| meta.len() > MAX_BYTES) {
        let old = path.with_extension("log.1");
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(path, &old);
    }
    std::fs::create_dir_all(path.parent()?).ok()?;
    OpenOptions::new().create(true).append(true).open(path).ok()
}

fn timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_secs())
        .unwrap_or(0);
    format_utc(secs)
}

/// Unix 秒 → `YYYY-MM-DD HH:MM:SSZ`（UTC）。不引时间 crate：civil_from_days
/// 算法（Howard Hinnant）十几行就够，诊断时间戳不需要时区与本地化。
fn format_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全局日志槽与 panic hook 是进程级状态：碰它们的测试串行跑。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("envboard-diag-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn utc_timestamps_match_known_epochs() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00Z");
        assert_eq!(format_utc(86_399), "1970-01-01 23:59:59Z");
        // 闰日（2000 是闰年）与世纪平年（2100 不是闰年）各钉一个。
        assert_eq!(format_utc(951_782_400), "2000-02-29 00:00:00Z");
        assert_eq!(format_utc(4_102_444_800), "2100-01-01 00:00:00Z");
    }

    #[test]
    fn notes_land_in_the_session_log() {
        let _guard = TEST_LOCK.lock().unwrap();
        let dir = scratch("note");
        let path = open(&dir, "test session").expect("open log");
        note("stage: boot");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("test session"), "会话头要在：{text}");
        assert!(text.contains("stage: boot"), "阶段行要在：{text}");
        assert!(path.ends_with("startup.log"));
        *lock() = None;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversized_logs_rotate_once() {
        let _guard = TEST_LOCK.lock().unwrap();
        let dir = scratch("rotate");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("startup.log"),
            vec![b'x'; (MAX_BYTES + 1) as usize],
        )
        .unwrap();
        let path = open(&dir, "after rotation").expect("open log");
        assert!(dir.join("startup.log.1").exists(), "旧日志要留一代");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("after rotation"));
        assert!(!text.contains('x'), "新会话不该带着旧内容");
        *lock() = None;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unwritable_location_degrades_instead_of_failing() {
        let _guard = TEST_LOCK.lock().unwrap();
        let dir = scratch("blocked");
        std::fs::create_dir_all(&dir).unwrap();
        // 用一个普通文件占住 logs_dir 的位置：建目录必然失败。
        let blocked = dir.join("blocked");
        std::fs::write(&blocked, b"not a dir").unwrap();
        assert!(open(&blocked, "session").is_none());
        // 没开槽时 note/path 也不许 panic。
        note("no slot, no problem");
        assert!(path().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn panic_hook_records_location_and_payload() {
        let _guard = TEST_LOCK.lock().unwrap();
        let dir = scratch("panic");
        let path = open(&dir, "panic test").expect("open log");
        let previous = std::panic::take_hook();
        install_panic_hook();
        let outcome = std::panic::catch_unwind(|| panic!("diagnostic probe"));
        // 还原 hook，别把测试链留给后面的测试。
        let _ = std::panic::take_hook();
        std::panic::set_hook(previous);
        assert!(outcome.is_err());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("diagnostic probe"), "payload 要在：{text}");
        assert!(text.contains("diagnostics.rs"), "file:line 要在：{text}");
        assert!(text.contains("backtrace:"), "backtrace 要在");
        *lock() = None;
        let _ = std::fs::remove_dir_all(&dir);
    }
}

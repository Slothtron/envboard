//! 实例日志**文件**的读取与轮转。
//!
//! 为什么"文件"归管理器管，而"管道/环形缓冲"归 core 管：日志文件落在
//! `<state_dir>/logs`（管理器的状态目录），core 只是按 [`InstanceSpec`] 告诉它的路径
//! 把子进程的 stdout/stderr 接过去。**一个文件只有一个读者/轮转者**，所以读尾部与轮转
//! 都在这里实现一次，core 那边不重复一份。
//!
//! 两条纪律：
//!
//! 1. **读有上限**：只从文件末尾读 [`TAIL_WINDOW_BYTES`]，不把可能几百 MB 的日志整个
//!    读进内存（旧实现是无上限 `read_to_string`）。
//! 2. **轮转必须 copytruncate，不能 rename**：子进程还持着这个 inode 的 fd 在写，
//!    rename 之后它会继续写那个已经被移走的文件（新文件永远是空的）。所以是
//!    "把尾部搬去 `.1`，再把原文件截断为 0" —— inode 不变，写者无感。
//!
//! [`InstanceSpec`]: envboard_engine::InstanceSpec

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// 读尾部时的窗口上限：一次最多读这么多字节再切行。
///
/// 8 KiB 的单行上限（`spawn_log_reader` 同款）意味着这个窗口至少能放下几百行。
pub const TAIL_WINDOW_BYTES: u64 = 256 * 1024;

/// 轮转后保留的那一份的路径：`<env>.log` → `<env>.log.1`。
pub fn rotated_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".1");
    path.with_file_name(name)
}

/// 读文件末尾的若干行（**有界**）。文件不存在或读不动时返回空，不报错 ——
/// "还没有日志"不是错误。
pub fn tail(path: &Path, lines: usize) -> Vec<String> {
    let Some(text) = read_tail_window(path, TAIL_WINDOW_BYTES) else {
        return Vec::new();
    };
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines.max(1));
    all[start..]
        .iter()
        .map(|line| (*line).to_string())
        .collect()
}

/// 尾部若干行，不够时**接着往前一份拼接**（轮转之后"最近的 200 行"往往跨两个文件）。
pub fn tail_with_rotated(path: &Path, lines: usize) -> Vec<String> {
    let mut collected = tail(path, lines);
    if collected.len() >= lines.max(1) {
        return collected;
    }
    let rotated = rotated_path(path);
    let mut older = tail(&rotated, lines.max(1) - collected.len());
    if older.is_empty() {
        return collected;
    }
    older.append(&mut collected);
    older
}

/// 超过上限就轮转；返回是否真的轮转过。
///
/// 上限 `cap_bytes == 0` 表示不轮转。
pub fn rotate_if_needed(path: &Path, cap_bytes: u64) -> std::io::Result<bool> {
    if cap_bytes == 0 {
        return Ok(false);
    }
    let Ok(metadata) = std::fs::metadata(path) else {
        return Ok(false);
    };
    if metadata.len() <= cap_bytes {
        return Ok(false);
    }

    // 把**尾部**留下（最近的现场最有用），写成 `.1`，然后截断原文件。
    let keep_bytes = cap_bytes / 2;
    if let Some(text) = read_tail_window(path, keep_bytes) {
        let rotated = rotated_path(path);
        // 原子替换：先写临时文件再 rename，读者不会看到半个文件。
        let tmp = rotated.with_extension("log.1.tmp");
        {
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
        }
        std::fs::rename(&tmp, &rotated)?;
    }

    // **截断而不是删除**：子进程还持着这个 inode 的 fd，删除会让它继续写一个已经
    // 不存在的文件，日志从此消失。截断后它的下一次 `write`（O_APPEND）从 0 开始。
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)?
        .set_len(0)?;
    Ok(true)
}

/// 从文件末尾读最多 `window` 字节，并丢掉被截断的首行（它可能只有半行）。
fn read_tail_window(path: &Path, window: u64) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(window);
    let partial_start = start > 0;
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buffer = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut buffer).ok()?;
    let text = String::from_utf8_lossy(&buffer).into_owned();
    if !partial_start {
        return Some(text);
    }
    // 窗口是从中间切进来的：第一行大概率只有半截，丢掉它（否则会渲染出半行）。
    match text.split_once('\n') {
        Some((_, rest)) => Some(rest.to_string()),
        None => Some(String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;

    fn write_lines(path: &Path, count: usize, prefix: &str) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for index in 0..count {
            writeln!(file, "{prefix}-{index}").unwrap();
        }
    }

    /// 临时目录句柄：**Drop 时自己删干净**。
    ///
    /// 早先每个测试各自 `remove_dir_all` 收尾，漏掉一个就会在 `/tmp` 里堆一堆
    /// `envboard-logs-*`；用 Drop 保证"无论断言在哪一步失败"都会被清掉。
    struct TempDir(PathBuf);

    impl TempDir {
        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_dir(tag: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!(
            "envboard-logs-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    #[test]
    fn tail_reads_the_last_lines() {
        let dir = temp_dir("tail");
        let path = dir.join("alpha.log");
        write_lines(&path, 500, "line");

        let lines = tail(&path, 3);
        assert_eq!(lines, vec!["line-497", "line-498", "line-499"]);
    }

    #[test]
    fn tail_is_bounded_by_the_window() {
        let dir = temp_dir("window");
        let path = dir.join("big.log");
        // 远超窗口的内容：不能整个读进内存，只能拿到窗口内的末尾行。
        let chunk = "x".repeat(1024);
        let mut file = std::fs::File::create(&path).unwrap();
        for index in 0..(TAIL_WINDOW_BYTES / 1024) * 2 {
            writeln!(file, "{chunk}-{index}").unwrap();
        }
        drop(file);

        let lines = tail(&path, 5);
        assert_eq!(lines.len(), 5);
        assert!(lines.iter().all(|line| !line.contains("… [line truncated")));
        // 窗口是从中间切进来的，首行必须被丢掉（否则会拿到半行）
        assert!(
            lines.iter().all(|line| line.starts_with('x')),
            "every returned line should be a whole line"
        );
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        assert!(tail(Path::new("/nonexistent/envboard/alpha.log"), 10).is_empty());
    }

    #[test]
    fn rotation_keeps_the_live_inode_and_bounds_both_files() {
        let dir = temp_dir("rotate");
        let path = dir.join("alpha.log");
        // 约 100 KB > 64 KiB 的上限，必然触发轮转
        write_lines(&path, 10_000, "line");
        let inode_before = std::fs::metadata(&path).unwrap();
        assert!(inode_before.len() > 64 * 1024);

        assert!(rotate_if_needed(&path, 64 * 1024).unwrap());

        let rotated = rotated_path(&path);
        assert!(rotated.exists(), "the rotated file must exist");
        let live = std::fs::metadata(&path).unwrap();
        // **崩溃现场守门断言**：截断（而不是删除/改名）→ inode 不变，
        // 子进程手里的 fd 仍然指向同一个文件。
        #[cfg(unix)]
        assert_eq!(
            live.ino(),
            inode_before.ino(),
            "rotation must truncate in place, not replace the file"
        );
        assert_eq!(live.len(), 0, "the live file must be truncated");
        assert!(
            std::fs::metadata(&rotated).unwrap().len() <= 32 * 1024,
            "the rotated file must hold only the kept tail"
        );

        // 轮转之后再写，日志立刻继续累积（写者无感）
        write_lines(&path, 2, "after");
        assert_eq!(tail(&path, 2), vec!["after-0", "after-1"]);
        // 跨轮转读：不够时往前一份拼
        let combined = tail_with_rotated(&path, 5);
        assert_eq!(combined.len(), 5);
        assert_eq!(&combined[3..], &["after-0", "after-1"]);
    }

    #[test]
    fn rotation_is_skipped_below_the_cap_and_when_disabled() {
        let dir = temp_dir("nocap");
        let path = dir.join("alpha.log");
        write_lines(&path, 10, "line");

        assert!(!rotate_if_needed(&path, 1024 * 1024).unwrap());
        assert!(!rotate_if_needed(&path, 0).unwrap());
        assert!(!rotated_path(&path).exists());
    }
}

//! 导出落盘：HAR 直接从内存写到「下载」目录。
//!
//! 为什么没有「另存为」框：桌面形态走的是浏览器 Blob 下载，本来也不经过文件对话框，
//! 落到系统下载目录就已经是同一件事；NSH 这一侧没有原生文件对话框原语，
//! 为一个按钮去接 COM 的 `IFileSaveDialog` 不划算（ADR-4 未决项按这个口径结）。
//!
//! 重名不覆盖：`<env>-session-<id>.har` 已存在就退到 `…-2.har`、`…-3.har`，
//! 用户连点两次不会把上一份悄悄顶掉。

use std::fs;
use std::path::{Path, PathBuf};

use envboard_engine::Error;

/// 写到哪：优先 `%USERPROFILE%\Downloads`（真实存在才用），否则 `%TEMP%`。
pub fn downloads_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    if let Some(dir) = home.map(|dir| dir.join("Downloads")) {
        if dir.is_dir() {
            return dir;
        }
    }
    std::env::var_os("TEMP")
        .map(PathBuf::from)
        .filter(|dir| dir.is_dir())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 写一份新文件，返回实际落地的绝对路径（同名已存在时自动加序号）。
pub fn write_unique(dir: &Path, stem: &str, ext: &str, text: &str) -> Result<PathBuf, Error> {
    for attempt in 1..=99u32 {
        let name = if attempt == 1 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem}-{attempt}.{ext}")
        };
        let path = dir.join(name);
        if path.exists() {
            continue;
        }
        return fs::write(&path, text).map(|()| path.clone()).map_err(|error| {
            Error::internal_error(format!("写文件失败：{}（{error}）", path.display()))
        });
    }
    Err(Error::internal_error(format!(
        "{} 里同名文件已有 99 个，先清理一下再导出",
        dir.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_export_does_not_overwrite_the_first() {
        let dir = std::env::temp_dir().join(format!(
            "envboard-har-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).expect("建测试目录失败");

        let first = write_unique(&dir, "proxy-a-session-7", "har", "{\"one\":1}").expect("第一次写失败");
        let second = write_unique(&dir, "proxy-a-session-7", "har", "{\"two\":2}").expect("第二次写失败");
        assert!(first.ends_with("proxy-a-session-7.har"));
        assert!(second.ends_with("proxy-a-session-7-2.har"));
        assert_eq!(fs::read_to_string(&first).unwrap_or_default(), "{\"one\":1}");
        assert_eq!(fs::read_to_string(&second).unwrap_or_default(), "{\"two\":2}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn downloads_dir_is_absolute_when_it_exists() {
        let dir = downloads_dir();
        assert!(dir.is_absolute() || dir == Path::new("."));
    }
}

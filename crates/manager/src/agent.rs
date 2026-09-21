//! 管理器落盘工件的写入纪律：**原子写 + 0600**。
//!
//! v2 这里还有注入器的 config.json 组装与规则软链维护 —— 随子进程模型一起退场；
//! 留下的只有物化文件（规则正文）共用的写入口。

use std::path::Path;

use envboard_engine::Error;

/// 原子写文件 + 0600（物化规则用的是同一套纪律）。
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("rules.tmp");
    std::fs::write(&tmp, bytes)?;
    restrict_mode(&tmp)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn restrict_mode(path: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_private_lands_the_bytes_atomically() {
        let dir = std::env::temp_dir().join(format!("envboard-agent-{}", std::process::id()));
        let path = dir.join("nested").join("beta.rules");
        write_private(&path, b"content").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"content");
        assert!(!dir.join("nested").join("beta.rules.tmp").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}

//! 注入器的物化。
//!
//! 注入器是**单文件、纯标准库**的 Python 脚本，构建期用 `include_str!` 内嵌进二进制，
//! 启动时物化到 `<state_dir>/agent/`。这样：
//!
//! * 发布物只有二进制，不依赖"碰巧 clone 到的仓库路径"；
//! * 注入器版本与二进制版本**天然绑定**（内容不一致就覆盖）；
//! * `pyproject.toml` / wheel / `src` 布局在 Python 这一侧不再是必需品。

use std::path::{Path, PathBuf};

use envboard_core_api::Error;

/// 物化后的文件名。
pub const INJECTOR_NAME: &str = "envboard_mitmproxy.py";

/// 注入器源码 —— 单一事实来源是 `adapters/mitmproxy/envboard_mitmproxy.py`。
///
/// `include_str!` 引用包目录之外的相对路径，代价是该 crate 单独 `cargo package` 时缺文件；
/// 我们不发布 crates（发布物是 `envboard` 二进制），所以接受这个取舍；
/// 写在这里，免得后来者以为是疏漏。
const INJECTOR_SOURCE: &str =
    include_str!("../../../../../adapters/mitmproxy/envboard_mitmproxy.py");

pub fn materialized_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join(INJECTOR_NAME)
}

/// 物化注入器：内容不一致才写（按字节比较，不做哈希文件）。
///
/// 返回物化后的路径，供 `mitmdump -s` 使用。
pub fn materialize(agent_dir: &Path) -> Result<PathBuf, Error> {
    std::fs::create_dir_all(agent_dir)?;
    let path = materialized_path(agent_dir);

    let up_to_date = std::fs::read(&path)
        .map(|existing| existing == INJECTOR_SOURCE.as_bytes())
        .unwrap_or(false);

    if !up_to_date {
        // 先写临时文件再 rename：避免 mitmdump 正好读到半个文件
        let tmp = path.with_extension("py.tmp");
        std::fs::write(&tmp, INJECTOR_SOURCE.as_bytes())?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok(path)
}

/// 注入器源码的 sha256 —— 与状态文件里的 `agent_version` 一起用于排查版本错配。
pub fn source_len() -> usize {
    INJECTOR_SOURCE.len()
}

pub fn source_contains(needle: &str) -> bool {
    INJECTOR_SOURCE.contains(needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn materialises_and_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("envboard-agent-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();

        let path = materialize(&dir).unwrap();
        assert!(path.exists());
        let first = std::fs::read(&path).unwrap();
        assert!(
            first.len() > 1_000,
            "the injector should be a real file, got {} bytes",
            first.len()
        );

        // 内容被改坏 → 下一次物化必须修复它（版本绑定）
        std::fs::write(&path, b"broken").unwrap();
        let path_again = materialize(&dir).unwrap();
        assert_eq!(std::fs::read(&path_again).unwrap(), first);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn injector_stays_single_file_and_dependency_free() {
        // 这条断言是"可随时丢掉"的技术保障：单文件 + 只用标准库（外加 mitmproxy 本身）。
        assert!(source_contains("def server_connect"));
        // 配置通道是"自己目录旁边的固定路径"，不再是 `--set` 选项
        assert!(source_contains("CONFIG_FILE_NAME"));
        assert!(source_contains("RULES_LINK_NAME"));
        assert!(source_contains("def tls_start_server"));
        assert!(source_contains("VERIFY_NONE"));
        // 被删掉的选项族不许回来：它们就是"用户可控 --set"的入口
        assert!(!source_contains("add_option"));
        assert!(!source_contains("envboard_expect"));
        // 不该出现对 v1 那些模块的依赖
        assert!(!source_contains("from envboard."));
        assert!(!source_contains("import mitmproxy_rs"));
    }
}

//! 每环境 agent 目录的物化：配置文件 + 规则软链。
//!
//! 目录形状（`<state_dir>/agent/<env>/`）：
//!
//! ```text
//! envboard_mitmproxy.py   注入器（由 core 物化，版本随二进制走）
//! config.json             ★ Rust ↔ 注入器之间**唯一**的配置通道（本模块写）
//! envboard.rules          固定名软链 → <rules_dir>/<name>.rules（本模块维护）
//! ```
//!
//! 两条纪律：
//!
//! 1. **`config.json` 的内容是环境的确定性函数**（不含时间戳）—— 于是"要不要重写"
//!    可以按字节比较，"期望哈希"任何时候都能重算，也不会因为每轮 reconcile 都写一次
//!    而让运行中的实例反复热重载；
//! 2. **软链替换是原子的**（同目录建临时链再 `rename`）—— 注入器不会读到半个链。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use envboard_core_api::{CONFIG_FILE_NAME, Error, ErrorCode, RULES_LINK_NAME};
use serde::{Deserialize, Serialize};

use envboard_core_api::sha256;

/// `config.json` 的格式版本。结构变化时递增，注入器拒绝不认识的高版本。
pub const AGENT_CONFIG_VERSION: u32 = 1;

/// `<agent_dir>/config.json` 的形状（v1）。
///
/// 字段顺序即序列化顺序：`serde_json` 按声明顺序输出结构体字段，所以同一份输入
/// 永远得到同一串字节，哈希才可比。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentConfig {
    pub version: u32,
    pub env: String,
    /// 状态文件绝对路径 —— 由管理器指定，注入器不拼环境名。
    pub status_file: String,
    /// 绑定的规则名（**名字，不是路径**）；`null` = 不覆盖任何域名。
    ///
    /// 注入器按固定名软链读规则，这个字段只做三件事：日志展示、绑定状态回执、
    /// **链接完整性校验**（链解析出的文件名与它不一致 = 响亮报错，不猜用哪一个）。
    pub rules: Option<String>,
    /// 按域名放宽上游证书校验的完整域名清单。
    pub insecure_hosts: Vec<String>,
    /// 管理器下发、必须被宿主接受的 `--set` 键值（当前只有 `proxyauth`）。
    pub launch_expected: BTreeMap<String, String>,
    /// 注入器轮询间隔（秒）。热重载的收敛窗口也按它算。
    pub reload_interval_secs: u64,
    /// 是否给流加注解（`envboard: <env>`）。
    pub annotate: bool,
}

impl AgentConfig {
    /// 规范化序列化：结构体字段顺序固定，所以字节确定。
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        serde_json::to_vec_pretty(self).map_err(Error::from)
    }

    /// 期望的配置指纹（sha256 十六进制）—— 与注入器回执的 `config_hash` 可直接比较。
    pub fn content_hash(bytes: &[u8]) -> String {
        sha256::hex(bytes)
    }
}

/// 写 `config.json`：**内容不一致才写**，原子替换 + 0600。
///
/// 返回 `(内容哈希, 是否发生了写入)`。哈希按"这份内容"算，无论有没有落盘 ——
/// 调用方用它做收敛判定。
pub fn write_config(agent_dir: &Path, config: &AgentConfig) -> Result<(String, bool), Error> {
    let bytes = config.to_bytes()?;
    let hash = AgentConfig::content_hash(&bytes);
    let path = agent_dir.join(CONFIG_FILE_NAME);

    let up_to_date = std::fs::read(&path)
        .map(|existing| existing == bytes)
        .unwrap_or(false);
    if up_to_date {
        return Ok((hash, false));
    }

    std::fs::create_dir_all(agent_dir)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    restrict_mode(&tmp)?;
    std::fs::rename(&tmp, &path)?;
    Ok((hash, true))
}

/// 把 `<agent_dir>/envboard.rules` 指到当前绑定的规则文件。
///
/// * `rules = None` → 删链（该环境不覆盖任何域名）；
/// * 目标文件不存在 → **不建悬空链**，只把旧链清掉：悬空链与"链不存在"在注入器
///   眼里是同一件事（不覆盖），而视图会把"规则缺失（已忽略）"显示出来。
///
/// 返回是否发生了改动（用于决定要不要记一条日志）。
pub fn sync_rules_link(
    agent_dir: &Path,
    rules: Option<&str>,
    rules_dir: &Path,
) -> Result<bool, Error> {
    let link = agent_dir.join(RULES_LINK_NAME);
    let desired = rules.map(|name| rules_dir.join(envboard_rules::file_name_for(name)));
    let current = std::fs::read_link(&link).ok();

    if current == desired {
        return Ok(false);
    }
    match desired {
        None => {
            remove_link(&link)?;
            Ok(true)
        }
        Some(target) if !target.exists() => {
            remove_link(&link)?;
            Ok(true)
        }
        Some(target) => {
            std::fs::create_dir_all(agent_dir)?;
            replace_link(&link, &target)?;
            Ok(true)
        }
    }
}

/// 原子替换软链：同目录建临时链，再 `rename` 覆盖。
pub fn replace_link(link: &Path, target: &Path) -> Result<(), Error> {
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp = link.as_os_str().to_owned();
    tmp.push(".tmp-link");
    let tmp = PathBuf::from(tmp);
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp).map_err(|error| {
        Error::new(
            ErrorCode::StoreFailure,
            format!(
                "cannot create the rules symlink {} -> {} ({error}); \
                 per-environment rule binding needs symlink support on this filesystem",
                link.display(),
                target.display()
            ),
        )
    })?;
    std::fs::rename(&tmp, link)?;
    Ok(())
}

/// 删掉一条软链（不存在也算成功）。
pub fn remove_link(link: &Path) -> Result<(), Error> {
    match std::fs::remove_file(link) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::from(error)),
    }
}

/// 链当前指向哪个规则名（用于视图与自检）：`None` = 没链 / 悬空。
pub fn linked_rules_name(link: &Path) -> Option<String> {
    let target = std::fs::read_link(link).ok()?;
    if !target.exists() {
        return None;
    }
    target
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(envboard_rules::RULES_SUFFIX))
        .map(str::to_string)
}

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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("envboard-agent-{}-{tag}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn config(rules: Option<&str>) -> AgentConfig {
        AgentConfig {
            version: AGENT_CONFIG_VERSION,
            env: "beta".into(),
            status_file: "/tmp/beta.status.json".into(),
            rules: rules.map(str::to_string),
            insecure_hosts: vec!["365.kdocs.cn".into()],
            launch_expected: BTreeMap::from([("proxyauth".to_string(), "u:p".to_string())]),
            reload_interval_secs: 5,
            annotate: true,
        }
    }

    #[test]
    fn config_bytes_are_deterministic_and_written_once() {
        let dir = temp_dir("config");
        let agent_dir = dir.join("beta");
        let (first_hash, wrote) = write_config(&agent_dir, &config(Some("beta"))).unwrap();
        assert!(wrote, "first write must land");
        let (second_hash, wrote_again) = write_config(&agent_dir, &config(Some("beta"))).unwrap();
        assert!(!wrote_again, "identical content must not be rewritten");
        assert_eq!(first_hash, second_hash);

        // 内容变了 → 哈希变 + 重写
        let (third_hash, wrote_third) = write_config(&agent_dir, &config(Some("gamma"))).unwrap();
        assert!(wrote_third);
        assert_ne!(second_hash, third_hash);

        // 0600：配置里有代理凭据
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(agent_dir.join(CONFIG_FILE_NAME))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_link_follows_the_binding_and_is_never_left_dangling() {
        let dir = temp_dir("link");
        let rules_dir = dir.join("rules");
        std::fs::create_dir_all(&rules_dir).unwrap();
        let agent_dir = dir.join("beta");
        std::fs::create_dir_all(&agent_dir).unwrap();

        // 目标不存在 → 不建悬空链
        assert!(sync_rules_link(&agent_dir, Some("beta"), &rules_dir).unwrap());
        let link = agent_dir.join(RULES_LINK_NAME);
        assert!(!link.exists());
        assert_eq!(linked_rules_name(&link), None);

        // 目标出现 → 建链，且可解析出名字
        std::fs::write(rules_dir.join("beta.rules"), b"# envboard rules file\n").unwrap();
        assert!(sync_rules_link(&agent_dir, Some("beta"), &rules_dir).unwrap());
        assert_eq!(linked_rules_name(&link).as_deref(), Some("beta"));

        // 换绑定 → 原子换链
        std::fs::write(rules_dir.join("gamma.rules"), b"# envboard rules file\n").unwrap();
        assert!(sync_rules_link(&agent_dir, Some("gamma"), &rules_dir).unwrap());
        assert_eq!(linked_rules_name(&link).as_deref(), Some("gamma"));
        // 幂等：再同步一次不算改动
        assert!(!sync_rules_link(&agent_dir, Some("gamma"), &rules_dir).unwrap());

        // 解绑 → 删链
        assert!(sync_rules_link(&agent_dir, None, &rules_dir).unwrap());
        assert!(!link.exists());

        std::fs::remove_dir_all(&dir).ok();
    }
}

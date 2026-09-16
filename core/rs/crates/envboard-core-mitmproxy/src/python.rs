//! mitmproxy 的解释器发现与自检。
//!
//! **"从 core.bin 所在环境推导解释器"这句话对 shim 脚本无定义** —— 本机 mitmproxy 是
//! `uv tool` 装的，`~/.local/bin/mitmdump` 只是个 shebang 脚本，而环境里默认的
//! `python3`（mise 管理）**`import mitmproxy` 直接失败**。所以这里按序探测、
//! 每一步都真跑一次 `import` 自检，并把实际探测到的路径与版本写进错误信息。

use std::path::{Path, PathBuf};

use envboard_core_api::{Error, ErrorCode};

/// 一次成功的解释器探测结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonInterpreter {
    pub path: PathBuf,
    /// 解释器自己的版本（如 `3.12.14`）。
    pub python_version: String,
    /// 该解释器里 mitmproxy 的版本（如 `12.2.3`）。
    pub mitmproxy_version: String,
}

/// `mitmdump --version` 的解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreVersion {
    pub name: String,
    pub version: String,
}

/// 解析 `mitmdump --version` 的输出（首行形如 `Mitmproxy: 12.2.3`）。
pub fn parse_core_version(output: &str) -> Option<CoreVersion> {
    let first = output.lines().next()?.trim();
    let (name, version) = first.split_once(':')?;
    Some(CoreVersion {
        name: name.trim().to_string(),
        version: version.trim().to_string(),
    })
}

/// 把 `core.bin` 解析成**绝对路径**。
///
/// 为什么必须做：探测解释器要读 `core.bin` 的 shebang，而裸命令名（默认值
/// `mitmdump`）本身没有可读的文件 —— 实测就是这么失败的：四个候选全都不存在，
/// 报"找不到带 mitmproxy 的解释器"，而它其实就在 PATH 上。
/// 顺带让 spawn 更可预测（不依赖子进程的 PATH）。
pub fn resolve_command(command: &Path) -> Option<PathBuf> {
    let has_separator = command.components().count() > 1;
    if has_separator {
        return command.exists().then(|| command.to_path_buf());
    }
    let name = command.to_str()?;
    which(name)
}

/// 探测顺序（命中即用，但都必须通过自检）：
///
/// 1. 显式配置的 `core.python`；
/// 2. `core.bin` 的 shebang（若是脚本）；
/// 3. uv-tool 布局：`<prefix>/bin/<name>` → `<prefix>/bin/python`；
/// 4. `core.bin` 同目录下的 `python3` / `python`。
pub fn discover(core_bin: &Path, configured: Option<&Path>) -> Result<PythonInterpreter, Error> {
    // 先把裸命令名解析成绝对路径，否则下面三条线索都无从谈起。
    let resolved = resolve_command(core_bin).unwrap_or_else(|| core_bin.to_path_buf());

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = configured {
        candidates.push(path.to_path_buf());
    }
    candidates.extend(from_shebang(&resolved));
    candidates.extend(from_tool_layout(&resolved));
    candidates.extend(siblings(&resolved));

    let mut tried: Vec<String> = Vec::new();
    for candidate in candidates {
        match probe(&candidate) {
            Ok(interpreter) => return Ok(interpreter),
            Err(reason) => tried.push(format!("{} → {reason}", candidate.display())),
        }
    }

    Err(Error::new(
        ErrorCode::InvalidConfig,
        format!(
            "cannot find a Python interpreter that has mitmproxy installed (needed for CA \
             pre-materialisation). Tried: [{}]. Set `core.python` explicitly — the ambient \
             python3 usually does NOT have mitmproxy (that is the common failure)",
            tried.join("; ")
        ),
    ))
}

/// 跑一次真实自检：能 import mitmproxy，并报告版本。
pub fn probe(interpreter: &Path) -> Result<PythonInterpreter, String> {
    if !interpreter.exists() {
        return Err("does not exist".to_string());
    }
    let script = "import importlib.metadata as m; import mitmproxy; import sys; \
                  print(sys.version.split()[0]); print(m.version('mitmproxy'))";
    let output = std::process::Command::new(interpreter)
        .arg("-c")
        .arg(script)
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut lines = stdout
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty());
            match (lines.next(), lines.next()) {
                (Some(python_version), Some(mitmproxy_version)) => Ok(PythonInterpreter {
                    path: interpreter.to_path_buf(),
                    python_version: python_version.to_string(),
                    mitmproxy_version: mitmproxy_version.to_string(),
                }),
                _ => Err(format!("unexpected self-check output: {stdout:?}")),
            }
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!(
                "`import mitmproxy` failed: {}",
                stderr.trim().lines().last().unwrap_or("")
            ))
        }
        Err(error) => Err(format!("cannot execute: {error}")),
    }
}

fn from_shebang(core_bin: &Path) -> Vec<PathBuf> {
    let Ok(head) = std::fs::read(core_bin) else {
        return Vec::new();
    };
    let head = &head[..head.len().min(256)];
    let Ok(text) = std::str::from_utf8(head) else {
        return Vec::new();
    };
    let Some(first) = text.lines().next() else {
        return Vec::new();
    };
    let Some(rest) = first.strip_prefix("#!") else {
        return Vec::new();
    };
    let rest = rest.trim();

    if let Some(command) = rest.strip_prefix("/usr/bin/env ") {
        // `#!/usr/bin/env python3` —— 顺着 PATH 找
        return which(command.split_whitespace().next().unwrap_or("python3"))
            .into_iter()
            .collect();
    }
    vec![PathBuf::from(
        rest.split_whitespace().next().unwrap_or(rest),
    )]
}

fn from_tool_layout(core_bin: &Path) -> Vec<PathBuf> {
    // uv tool: <prefix>/bin/mitmdump + <prefix>/bin/python
    core_bin
        .parent()
        .map(|bin| vec![bin.join("python"), bin.join("python3")])
        .unwrap_or_default()
}

fn siblings(core_bin: &Path) -> Vec<PathBuf> {
    // 二进制形态的 core：解释器通常不在同目录，但值得一试
    core_bin
        .parent()
        .map(|dir| vec![dir.join("python3"), dir.join("python")])
        .unwrap_or_default()
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_version_line() {
        let output = "Mitmproxy: 12.2.3\nPython:    3.12.14\nOpenSSL:   4.0.1\n";
        let parsed = parse_core_version(output).unwrap();
        assert_eq!(parsed.name, "Mitmproxy");
        assert_eq!(parsed.version, "12.2.3");
    }

    #[test]
    fn shebang_drives_discovery_for_shim_scripts() {
        let dir = std::env::temp_dir().join(format!("envboard-shebang-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shim = dir.join("mitmdump");
        // 指向当前解释器：一定能跑起来，便于断言"探测真的执行了"
        std::fs::write(
            &shim,
            format!("#!{}\n", std::env::current_exe().unwrap().display()),
        )
        .unwrap();
        let candidates = from_shebang(&shim);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0], std::env::current_exe().unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn probe_reports_a_clear_error_without_mitmproxy() {
        // 用一个**没有 mitmproxy** 的解释器：只要它存在，就必须报"import 失败"而不是崩
        let interpreter = which("python3").expect("python3 must be on PATH");
        match probe(&interpreter) {
            Ok(found) => {
                // 若真装了就断言版本非空；否则走 Err 分支
                assert!(!found.mitmproxy_version.is_empty());
                assert!(!found.python_version.is_empty());
            }
            Err(reason) => assert!(reason.contains("import") || reason.contains("execute")),
        }
    }

    #[test]
    fn bare_command_names_are_resolved_through_path() {
        // 这条是那次真实失败的守门测试：默认 `core.bin = mitmdump` 是裸名字，
        // 不解析就永远是"找不到解释器"。
        let resolved = resolve_command(Path::new("sh")).expect("sh must be on PATH");
        assert!(resolved.is_absolute());
        assert!(resolve_command(Path::new("/definitely/not/here")).is_none());
        assert!(resolve_command(Path::new("definitely-not-a-command-xyz")).is_none());
    }

    #[test]
    fn tool_layout_looks_next_to_the_binary() {
        let candidates = from_tool_layout(Path::new("/opt/tool/bin/mitmdump"));
        assert_eq!(candidates[0], PathBuf::from("/opt/tool/bin/python"));
    }
}

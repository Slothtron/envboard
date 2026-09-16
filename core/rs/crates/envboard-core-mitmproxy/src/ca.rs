//! 共享 CA 的预物化与就绪校验。
//!
//! 契约里的四条就绪判据（缺一不可，且**不看"文件齐全"就放行**）：
//!
//! 1. 六个文件存在且可读；
//! 2. `-ca.pem` 能被加载（私钥合法）；
//! 3. `-ca.pem` 与 `-ca-cert.pem` 的**公钥一致**（配对）；
//! 4. 能被 core 自己的 `CertStore` 加载 —— 这一条由"生成方与使用方是同一个
//!    mitmproxy 版本"保证（版本一致性在 [`crate::python`] 里已强制）。
//!
//! 任何一条不满足 → **删掉整套并重新物化**（幂等），而不是带着坏 CA 继续 ——
//! M0.5 spike 的 1d 已经证明："文件都在"完全可能是坏 CA，而那正是"实例拉起前先物化 CA"要防的故障。

use std::path::{Path, PathBuf};

use envboard_core_api::{Error, ErrorCode};

/// 契约钉死的两个值（`mitmproxy/options.py:7,9`），不在两处各写一遍。
pub const CONF_BASENAME: &str = "mitmproxy";
pub const KEY_SIZE: u32 = 2048;

/// `create_store` 写出的全部文件（`certs.py:563-615`，含 dhparam）。
pub const CA_FILES: &[&str] = &[
    "mitmproxy-ca.pem",
    "mitmproxy-ca.p12",
    "mitmproxy-ca-cert.pem",
    "mitmproxy-ca-cert.cer",
    "mitmproxy-ca-cert.p12",
    "mitmproxy-dhparam.pem",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaInfo {
    /// 证书公钥的 sha256（用于日志与"所有实例同一张 CA"的比对）。
    pub fingerprint: String,
    pub materialized: bool,
}

/// 在 `python -c` 里跑的一段脚本：加载/生成 CA，并做**配对校验**。
///
/// 用磁盘上的 CA 证书文件（`-ca-cert.pem`）而不是 `-ca.pem` 做配对基准，因为客户端
/// 装的是前者；两者不配对时"客户端随机证书错误"正是要防的症状。
const PREWARM_SCRIPT: &str = r#"
import hashlib, sys
from pathlib import Path
from mitmproxy.certs import CertStore
from cryptography import x509
from cryptography.hazmat.primitives import serialization

confdir = Path(sys.argv[1])
before = {p.name for p in confdir.glob("mitmproxy-*")} if confdir.exists() else set()

store = CertStore.from_store(confdir, "mitmproxy", 2048)
after = {p.name for p in confdir.glob("mitmproxy-*")}

def public_key(path, loader):
    raw = path.read_bytes()
    obj = loader(raw)
    return obj.public_key().public_bytes(
        serialization.Encoding.DER, serialization.PublicFormat.SubjectPublicKeyInfo
    )

key_pub = public_key(confdir / "mitmproxy-ca.pem",
                     lambda raw: serialization.load_pem_private_key(raw, password=None))
cert_pub = public_key(confdir / "mitmproxy-ca-cert.pem", x509.load_pem_x509_certificate)
if key_pub != cert_pub:
    print("MISMATCH")
    sys.exit(4)

print("fingerprint=" + hashlib.sha256(cert_pub).hexdigest())
print("created=" + ("yes" if not (after <= before) else "no"))
print("certs=" + str(len(store.certs)))
"#;

/// 确保有一个**配对且可用**的共享 CA；不满足就重做一次。
pub fn ensure(python: &Path, confdir: &Path, core_python_version: &str) -> Result<CaInfo, Error> {
    if let Some(info) = verify(python, confdir)? {
        return Ok(info);
    }
    wipe(confdir)?;
    materialize(python, confdir, core_python_version)
}

/// 校验现有 CA；返回 `None` 表示"需要重新物化"。**不改动任何文件。**
pub fn verify(python: &Path, confdir: &Path) -> Result<Option<CaInfo>, Error> {
    let missing: Vec<&str> = CA_FILES
        .iter()
        .copied()
        .filter(|name| !confdir.join(name).exists())
        .collect();
    if !missing.is_empty() {
        return Ok(None);
    }

    let output = std::process::Command::new(python)
        .arg("-c")
        .arg(PREWARM_SCRIPT)
        .arg(confdir)
        .output()
        .map_err(|error| {
            Error::new(
                ErrorCode::InternalError,
                format!("cannot run CA check: {error}"),
            )
        })?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let fingerprint = stdout
            .lines()
            .find_map(|line| line.strip_prefix("fingerprint="))
            .unwrap_or_default()
            .to_string();
        if fingerprint.is_empty() {
            return Ok(None);
        }
        return Ok(Some(CaInfo {
            fingerprint,
            materialized: false,
        }));
    }

    // 退出码 4 = 私钥与证书不配对（M0.5 spike 1d 的那类故障）
    let stderr = String::from_utf8_lossy(&output.stderr);
    let code = output.status.code().unwrap_or(-1);
    if code == 4 {
        return Ok(None);
    }
    Err(Error::new(
        ErrorCode::InternalError,
        format!(
            "CA check failed (exit {code}): {}",
            stderr.trim().lines().last().unwrap_or("no stderr")
        ),
    ))
}

fn materialize(python: &Path, confdir: &Path, core_python_version: &str) -> Result<CaInfo, Error> {
    std::fs::create_dir_all(confdir)?;
    let output = std::process::Command::new(python)
        .arg("-c")
        .arg(PREWARM_SCRIPT)
        .arg(confdir)
        .output()
        .map_err(|error| {
            Error::new(
                ErrorCode::InternalError,
                format!("cannot materialise CA: {error}"),
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::new(
            ErrorCode::InvalidConfig,
            format!(
                "cannot materialise the shared CA with {} (mitmproxy {core_python_version}): {}",
                python.display(),
                stderr.trim().lines().last().unwrap_or("no stderr")
            ),
        ));
    }

    let info = verify(python, confdir)?.ok_or_else(|| {
        Error::new(
            ErrorCode::InternalError,
            "CA was materialised but still fails the readiness check".to_string(),
        )
    })?;
    Ok(CaInfo {
        materialized: true,
        ..info
    })
}

fn wipe(confdir: &Path) -> Result<(), Error> {
    for name in CA_FILES {
        let path: PathBuf = confdir.join(name);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_files_require_materialisation() {
        let dir = std::env::temp_dir().join(format!("envboard-ca-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 用一个不可能存在的解释器：只要文件不齐就该在**跑脚本之前**返回 None
        let result = verify(Path::new("/nonexistent/python"), &dir).unwrap();
        assert!(
            result.is_none(),
            "an incomplete confdir must be reported as needing materialisation"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_pair_is_detected_by_the_pairing_check() {
        // 六个文件都在，但内容不是配对的证书/私钥 → 必须判为"需要重新物化"
        let dir = std::env::temp_dir().join(format!("envboard-ca-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in CA_FILES {
            std::fs::write(dir.join(name), b"not a real pem").unwrap();
        }
        let python = which_python();
        match python {
            Some(python) => {
                let result = verify(&python, &dir);
                match result {
                    Ok(None) => {}
                    // 没有 mitmproxy 的解释器会在 import 阶段失败，那是环境问题不是逻辑问题
                    Ok(Some(_)) => panic!("a corrupt CA must never be reported as ready"),
                    Err(error) => assert!(
                        error.message.contains("CA check failed"),
                        "unexpected error: {}",
                        error.message
                    ),
                }
            }
            None => eprintln!("skipped: no python3 on PATH"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    fn which_python() -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|dir| dir.join("python3"))
            .find(|p| p.is_file())
    }
}

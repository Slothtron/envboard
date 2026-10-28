//! [`HostOptions`]：组合根的入参 —— 命令行到 [`ManagerConfig`] 的翻译落点。
//!
//! 纪律与宿主二进制原来的一致：**读环境变量是适配器的职责**（core 禁止直接读
//! 环境变量），所以 `ENVBOARD_STATE_DIR` / `HOME` 的回退链落在本文件，而不是
//! 渗进 manager 或 domain。CLI 参数本体（`clap`）留在各二进制的 `main.rs`：
//! 参数名与帮助文本属于界面形态，不属于组合根。

use std::path::PathBuf;

use envboard_engine::Error;
use envboard_manager::ManagerConfig;

/// 起一个宿主（任意界面形态）所需的全部选择。
///
/// 字段全是 `Option`：默认值由 [`ManagerConfig::new`] 从 `state_dir` 推出，
/// 这里不重复任何一条默认（重复就会漂移）。
#[derive(Debug, Clone, Default)]
pub struct HostOptions {
    /// 状态目录。`None` 时走回退链：`$ENVBOARD_STATE_DIR` → `$HOME/.envboard`。
    pub state_dir: Option<PathBuf>,
    /// 随机分配端口的区间（`--port-range 16000-16999` 的翻译结果）。
    pub port_range: Option<(u16, u16)>,
    /// 抓包缓冲字节预算（每环境；`0` = 引擎默认）。
    pub capture_budget: usize,
    /// 实例日志落盘目录；`None` = 用默认（`<state_dir>/logs`）。
    pub log_dir: Option<PathBuf>,
    /// 显式要求**不落盘**（日志只留在内存的有界环形缓冲）。与 `log_dir` 同时给出时
    /// 以本标志为准 —— 它表达的是"我就是不要文件"。
    pub no_log_file: bool,
    /// 单个环境日志文件的轮转上限（字节；`0` = 不轮转）。
    pub max_log_bytes: Option<u64>,
}

impl HostOptions {
    /// 解析状态目录：显式参数 → `$ENVBOARD_STATE_DIR` → `$HOME/.envboard`。
    ///
    /// 三者都没有才响亮失败（原来这条判据在宿主 `main.rs` 里，语义不变）。
    pub fn resolve_state_dir(&self) -> Result<PathBuf, Error> {
        self.state_dir
            .clone()
            .or_else(|| std::env::var_os("ENVBOARD_STATE_DIR").map(PathBuf::from))
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".envboard")))
            .ok_or_else(|| {
                Error::invalid_config(
                    "state_dir",
                    "cannot determine a state dir; pass --state-dir",
                )
            })
    }

    /// 翻译成经过校验的 [`ManagerConfig`]。
    ///
    /// `port_range` 的**文本**解析（`"min-max"`）在调用方的 CLI 层做完，这里只接收
    /// 已经成形的区间 —— 参数怎么拼属于界面形态，区间是否合法属于 domain。
    pub fn into_config(self) -> Result<ManagerConfig, Error> {
        let mut config = ManagerConfig::new(self.resolve_state_dir()?);
        config.capture_budget = self.capture_budget;
        if let Some(dir) = self.log_dir {
            config.log_dir = Some(dir);
        }
        if self.no_log_file {
            config.log_dir = None;
        }
        if let Some(bytes) = self.max_log_bytes {
            config.max_log_bytes = bytes;
        }
        if let Some(range) = self.port_range {
            config.port_range = range;
        }
        config.validate()?;
        Ok(config)
    }
}

/// 解析 `--port-range` 的文本形状（`"16000-16999"`）。
///
/// 供各宿主二进制的 CLI 层复用：形状错误归 `invalid_config`、字段路径指向
/// `port_range`，与原宿主 `main.rs` 的判据逐字一致。
pub fn parse_port_range(raw: &str) -> Result<(u16, u16), Error> {
    let (min, max) = raw.split_once('-').ok_or_else(|| {
        Error::invalid_config("port_range", format!("expected <min>-<max>, got {raw:?}"))
    })?;
    let min: u16 = min
        .trim()
        .parse()
        .map_err(|_| Error::invalid_config("port_range", format!("bad min port in {raw:?}")))?;
    let max: u16 = max
        .trim()
        .parse()
        .map_err(|_| Error::invalid_config("port_range", format!("bad max port in {raw:?}")))?;
    Ok((min, max))
}

#[cfg(test)]
mod tests {
    use super::parse_port_range;

    #[test]
    fn port_range_accepts_the_documented_shape() {
        assert_eq!(parse_port_range("16000-16999").unwrap(), (16000, 16999));
        // 空白容忍：`" 16000 - 16999 "` 是同一个意思。
        assert_eq!(parse_port_range(" 16000 - 16999 ").unwrap(), (16000, 16999));
    }

    #[test]
    fn port_range_rejects_bad_shapes_with_the_field_path_set() {
        for raw in ["16000", "16000:", "abc-16999", "16000-70000"] {
            let error = parse_port_range(raw).expect_err("shape must be rejected");
            assert_eq!(error.field.as_deref(), Some("port_range"), "got {error:?}");
        }
    }

    #[test]
    fn no_log_file_beats_an_explicit_log_dir() {
        let options = super::HostOptions {
            state_dir: Some(std::path::PathBuf::from("/tmp/envboard-options-test")),
            log_dir: Some(std::path::PathBuf::from("/tmp/envboard-options-logs")),
            no_log_file: true,
            ..Default::default()
        };
        assert!(
            options.into_config().unwrap().log_dir.is_none(),
            "`--no-log-file` 表达的就是不要文件，必须压过 --log-dir"
        );
    }
}

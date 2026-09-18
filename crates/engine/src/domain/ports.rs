//! 端口选择（`port.allocate`）—— 纯决策，不做 I/O。
//!
//! 契约见 `spec/capabilities.md`（§端口分配）。这里只裁"选择规则"：
//! 候选序列的生成（在区间内随机打乱）、试绑与账本由管理器负责，
//! **随机源不进契约**，否则 golden 无法确定。

use std::collections::BTreeSet;

use crate::{Error, ErrorCode};

/// 默认随机分配区间。为什么是 16xxx 见（避开存量服务与系统临时端口段）。
pub const DEFAULT_PORT_RANGE: (u16, u16) = (16_000, 16_999);

/// 连续候选都不可用时的重试上限（默认）。
pub const DEFAULT_MAX_ATTEMPTS: usize = 32;

/// 契约里 `field` 用哪条路径。
pub const PORT_FIELD: &str = "environment.listen.port";

/// 非致命提示 —— 会出现在决策结果里，由工作台显示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Warning {
    /// 已持久化的端口落在当前 `port_range` 之外：**保留原端口**，只告警。
    OutOfRange,
}

impl Warning {
    pub fn as_str(self) -> &'static str {
        match self {
            Warning::OutOfRange => "out_of_range",
        }
    }
}

/// 一次分配的输入。字段顺序即契约 fixture 的形状。
#[derive(Debug, Clone)]
pub struct PortRequest {
    /// 已持久化的端口（`None` = 新建环境）。
    pub existing: Option<u16>,
    /// 显式指定（`--port`）。
    pub requested: Option<u16>,
    /// 候选序列：已排除"本管理器已分配端口"，并已随机打乱。
    pub candidates: Vec<u16>,
    /// 当前配置的区间，仅用于判定 `out_of_range`。
    pub range: (u16, u16),
    pub max_attempts: usize,
}

impl Default for PortRequest {
    fn default() -> Self {
        Self {
            existing: None,
            requested: None,
            candidates: Vec::new(),
            range: DEFAULT_PORT_RANGE,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        }
    }
}

/// 把物化的占用集合变成判定函数 —— 契约测试与单测用（真实实现要 bind，见下）。
pub fn occupancy_from(ports: &BTreeSet<u16>) -> impl FnMut(u16) -> bool + '_ {
    move |port| ports.contains(&port)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortDecision {
    pub port: u16,
    pub warnings: Vec<Warning>,
}

/// 选择规则（顺序即优先级）：
///
/// 1. **已有持久化端口** → 一律复用。它**是否被占用都不看**：端口是环境的对外身份，
///    自己上一次监听的残留本来就该算"占用"。真被外人占走的情形由"启动失败 →
///    `port_conflict`"处理，而不是在这里悄悄换端口。
/// 2. **显式指定** → 被占用即失败（绝不静默改），空闲则采用（允许落在区间外，
///    因为它的用途就是人工固定）。
/// 3. **无指定** → 按候选顺序取第一个空闲端口，最多尝试 `max_attempts` 次；
///    用尽仍失败 → [`ErrorCode::PortRangeExhausted`]（响亮失败）。
///
/// **`is_occupied` 是注入的，而且必须被"按需"调用**（不要先物化整张占用表）：
/// 真实的判定是一次 `bind`，在一台 WSL2 mirrored 模式的机器上实测**单次约 21ms**
/// （连 `bind` 到端口 0 也是），于是"先把 16000–16999 全探一遍"要花约 21 秒。
/// 惰性探测把常见情形压到 1 次，最坏情形仍是 `max_attempts` 次。
pub fn select_port(
    request: &PortRequest,
    is_occupied: &mut dyn FnMut(u16) -> bool,
) -> Result<PortDecision, Error> {
    if let Some(existing) = request.existing {
        let warnings = if is_within(existing, request.range) {
            Vec::new()
        } else {
            vec![Warning::OutOfRange]
        };
        return Ok(PortDecision {
            port: existing,
            warnings,
        });
    }

    if let Some(requested) = request.requested {
        if is_occupied(requested) {
            return Err(Error::at(
                ErrorCode::InvalidConfig,
                PORT_FIELD,
                format!(
                    "port {requested} is already in use; pick another --port or free it \
                     (the manager never silently reassigns an explicit port)"
                ),
            ));
        }
        return Ok(PortDecision {
            port: requested,
            warnings: Vec::new(),
        });
    }

    let mut attempts = 0usize;
    for candidate in request.candidates.iter() {
        if attempts >= request.max_attempts {
            break;
        }
        attempts += 1;
        if !is_occupied(*candidate) {
            return Ok(PortDecision {
                port: *candidate,
                warnings: Vec::new(),
            });
        }
    }

    let (min, max) = request.range;
    Err(Error::new(
        ErrorCode::PortRangeExhausted,
        format!(
            "no free port in {min}-{max} after {attempts} attempt(s); free a port or pass \
             --port explicitly"
        ),
    ))
}

fn is_within(port: u16, range: (u16, u16)) -> bool {
    port >= range.0 && port <= range.1
}

/// 生成候选序列：区间内排除"本管理器已分配端口"，然后**确定性打乱**。
///
/// 打乱用调用方给的种子，因此测试可以复现；生产侧由管理器用时间 + pid 播种。
/// 不引入 rand 依赖：几十行以内的 xorshift 足够，而且省掉一层离线构建风险。
pub fn candidate_ports(range: (u16, u16), allocated: &BTreeSet<u16>, seed: u64) -> Vec<u16> {
    let mut candidates: Vec<u16> = (range.0..=range.1)
        .filter(|port| !allocated.contains(port))
        .collect();
    shuffle(&mut candidates, seed);
    candidates
}

/// Fisher–Yates，xorshift64* 作为随机源。
fn shuffle(items: &mut [u16], seed: u64) {
    let mut state = if seed == 0 {
        0x9E37_79B9_7F4A_7C15
    } else {
        seed
    };
    for index in (1..items.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let pick = (state % (index as u64 + 1)) as usize;
        items.swap(index, pick);
    }
}

/// 管理器播种用：时间 + pid，避免两次运行拿到同一序列。
pub fn seed_from(now_unix_nanos: u128, pid: u32) -> u64 {
    let mixed = (now_unix_nanos as u64) ^ ((pid as u64) << 32);
    if mixed == 0 {
        0x2545_F491_4F6C_DD1D
    } else {
        mixed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taken(ports: &[u16]) -> BTreeSet<u16> {
        ports.iter().copied().collect()
    }

    #[test]
    fn picks_first_free_candidate() {
        let request = PortRequest {
            candidates: vec![16_005, 16_001, 16_200],
            ..PortRequest::default()
        };
        let busy = taken(&[16_005, 16_400]);
        let mut occupied = occupancy_from(&busy);
        assert_eq!(select_port(&request, &mut occupied).unwrap().port, 16_001);
    }

    #[test]
    fn probing_stops_at_the_first_free_candidate() {
        // 惰性探测的守门测试：每次 bind 都有真实代价（本机实测约 21ms），
        // 所以"先物化整张占用表"是不可接受的退化 —— 这里断言探测次数。
        let request = PortRequest {
            candidates: (16_000..16_100).collect(),
            ..PortRequest::default()
        };
        let mut probes = 0usize;
        let mut occupied = |port: u16| {
            probes += 1;
            port <= 16_003 // 前四个被占
        };
        assert_eq!(select_port(&request, &mut occupied).unwrap().port, 16_004);
        assert_eq!(
            probes, 5,
            "must probe lazily, not materialize the whole range"
        );
    }

    #[test]
    fn explicit_port_is_respected_even_outside_range() {
        let request = PortRequest {
            requested: Some(16_600),
            range: (16_500, 16_600),
            ..PortRequest::default()
        };
        let mut free = |_: u16| false;
        assert_eq!(select_port(&request, &mut free).unwrap().port, 16_600);

        let outside = PortRequest {
            requested: Some(8_999),
            ..PortRequest::default()
        };
        assert_eq!(select_port(&outside, &mut free).unwrap().port, 8_999);
    }

    #[test]
    fn explicit_port_in_use_fails_loudly() {
        let request = PortRequest {
            requested: Some(16_600),
            ..PortRequest::default()
        };
        let busy = taken(&[16_600]);
        let mut occupied = occupancy_from(&busy);
        let error = select_port(&request, &mut occupied).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidConfig);
        assert_eq!(error.field.as_deref(), Some(PORT_FIELD));
    }

    #[test]
    fn persisted_port_wins_without_any_probing() {
        let request = PortRequest {
            existing: Some(16_301),
            candidates: vec![16_500],
            ..PortRequest::default()
        };
        let mut probes = 0usize;
        let mut occupied = |_: u16| {
            probes += 1;
            true // 就算"所有端口都被占"，已持久化的端口照样复用
        };
        let decision = select_port(&request, &mut occupied).unwrap();
        assert_eq!(decision.port, 16_301);
        assert!(decision.warnings.is_empty());
        assert_eq!(probes, 0, "a persisted port must not trigger any probe");
    }

    #[test]
    fn persisted_port_outside_range_is_kept_with_a_warning() {
        let request = PortRequest {
            existing: Some(16_301),
            range: (16_500, 16_999),
            ..PortRequest::default()
        };
        let mut free = |_: u16| false;
        let decision = select_port(&request, &mut free).unwrap();
        assert_eq!(decision.port, 16_301);
        assert_eq!(decision.warnings, vec![Warning::OutOfRange]);
    }

    #[test]
    fn exhaustion_stops_at_max_attempts_even_if_a_later_candidate_is_free() {
        let candidates: Vec<u16> = (16_000..16_033).collect();
        let request = PortRequest {
            candidates,
            ..PortRequest::default()
        };
        let busy = taken(&(16_000..16_032).collect::<Vec<_>>());
        let mut occupied = occupancy_from(&busy);
        let error = select_port(&request, &mut occupied).unwrap_err();
        assert_eq!(error.code, ErrorCode::PortRangeExhausted);
    }

    #[test]
    fn candidate_generation_excludes_allocated_and_is_reproducible() {
        let allocated = taken(&[16_000, 16_999]);
        let first = candidate_ports(DEFAULT_PORT_RANGE, &allocated, 42);
        let second = candidate_ports(DEFAULT_PORT_RANGE, &allocated, 42);
        assert_eq!(first, second, "same seed must give the same order");
        assert_eq!(first.len(), 998);
        assert!(!first.contains(&16_000) && !first.contains(&16_999));
        assert_ne!(first, candidate_ports(DEFAULT_PORT_RANGE, &allocated, 43));
    }
}

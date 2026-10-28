//! 发布物内容校验：**唯一的那颗二进制里真的带着进程内引擎**。
//!
//! 本仓的发布物是 `envboard` 一个 GUI 二进制（crate 不发布，`publish = false` 由
//! deps 门禁第 9 条钉着）。仓库文件面的纪律（必需文件、历史残留、测试资产）在
//! `tests/policy-tests/tests/artifacts.rs`；这里只判**构建产物**那一半 ——
//! 它必须住在产物 crate 的集成测试里，因为只有这里拿得到 `CARGO_BIN_EXE_envboard`。
//!
//! 判据用引擎运行时的必然足迹：每个实例一条线程，线程名以 `envboard-engine-` 开头。
//! 字符串在不在二进制里，说明这份二进制是不是真的把引擎编了进来 ——
//! web 形态时代这条用来抓"发布的还是上一代"，桌面形态同样需要它
//! （组合根要是被悄悄换成 HTTP 客户端，这条就会红）。
//!
//! 顺序上不需要"先 cargo build"：cargo 保证跑集成测试前已经构建好本 crate 的 bin。

/// 引擎线程名的前缀（`crates/engine` 里给每条实例线程起的名字）。
const ENGINE_MARKER: &[u8] = b"envboard-engine-";

#[test]
fn the_shipped_binary_carries_the_in_process_engine() {
    let path = env!("CARGO_BIN_EXE_envboard");
    let binary = std::fs::read(path).unwrap_or_else(|error| panic!("{path} 读不出来：{error}"));
    assert!(
        binary
            .windows(ENGINE_MARKER.len())
            .any(|w| w == ENGINE_MARKER),
        "二进制 {path} 里找不到进程内引擎的线程名标记 —— 界面直连的到底是不是组合根？"
    );
}

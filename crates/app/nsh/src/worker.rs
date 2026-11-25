//! S4（方案 §4.1 F）：常驻 worker，不再「一个动作一条 std 线程」。
//!
//! 两条车道，都是**长驻**线程：
//!
//! * `write` —— 引擎写入（`start` / `stop` / `create` / `remove` / `reallocate`）、
//!   同步 Win32 动作（CA 装/卸）与 HAR 落盘。FIFO：用户点的先后就是账本变化的先后，
//!   这里**不**允许后来者取消前一个（一次停了一个刚起的实例会留下半个状态）。
//! * `read` —— 详情读取。带**代次**：提交时发一个新号，worker 在开跑前发现已有更新的
//!   代次就跳过这条（任务连同它的 `oneshot::Sender` 一起被丢掉，UI 侧就是「取消了」）。
//!
//! 为什么不是严格一条线程：验收要求「点行 ≤200ms 出骨架」，而 HAR 导出是秒级重活；
//! 一条线程会让「导出途中随手点一行」的骨架排在导出后面。两条常驻线程仍然满足本节的
//! 目的——线程数有界、不按动作创建、pending 可取消——代价是多 1 条线程。
//!
//! 还没 `install()`（单测、golden、宿主没起来）时提交会退回「临时线起一条」：
//! 行为不变，只是少了队列与取消。这样 worker 不做成硬依赖。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, OnceLock};

/// 一个待执行的任务。`tag` 只有读车道会用。
struct Task {
    run: Box<dyn FnOnce() + Send>,
    tag: Option<u64>,
}

struct Lane {
    tx: mpsc::Sender<Task>,
}

static WRITE: OnceLock<Lane> = OnceLock::new();
static READ: OnceLock<Lane> = OnceLock::new();
/// 读车道最近一次提交的代次（worker 用它判「这条是不是已经过期」）。
static READ_LATEST: AtomicU64 = AtomicU64::new(0);

/// 起两条常驻线程。重复调用是空操作（`boot()` 里调一次即可）。
pub fn install() {
    if WRITE.get().is_some() {
        return;
    }
    let (wtx, wrx) = mpsc::channel::<Task>();
    let (rtx, rrx) = mpsc::channel::<Task>();
    std::thread::Builder::new()
        .name("envboard-write".to_owned())
        .spawn(move || run_lane(wrx, None))
        .expect("spawn the write worker");
    std::thread::Builder::new()
        .name("envboard-read".to_owned())
        .spawn(move || run_lane(rrx, Some(&READ_LATEST)))
        .expect("spawn the read worker");
    let _ = WRITE.set(Lane { tx: wtx });
    let _ = READ.set(Lane { tx: rtx });
    log::info!("worker：写 / 读两条常驻线程已起");
}

fn run_lane(rx: mpsc::Receiver<Task>, latest: Option<&'static AtomicU64>) {
    for task in rx {
        if let (Some(tag), Some(latest)) = (task.tag, latest) {
            if is_stale(tag, latest) {
                // 已经有人提交了更新的读：这条不跑，sender 随任务一起落地，
                // UI 侧收到的是一个 Err（＝这次读取被取消）。
                continue;
            }
        }
        (task.run)();
    }
}

/// 写动作：FIFO，不取消。
pub fn submit_write(run: impl FnOnce() + Send + 'static) {
    match WRITE.get() {
        Some(lane) => {
            let _ = lane.tx.send(Task {
                run: Box::new(run),
                tag: None,
            });
        }
        None => {
            std::thread::spawn(run);
        }
    }
}

/// 读动作：带代次；更新的读一到，还没开跑的旧读就作废。
pub fn submit_read(run: impl FnOnce() + Send + 'static) {
    let tag = READ_LATEST.fetch_add(1, Ordering::AcqRel) + 1;
    match READ.get() {
        Some(lane) => {
            let _ = lane.tx.send(Task {
                run: Box::new(run),
                tag: Some(tag),
            });
        }
        None => {
            std::thread::spawn(run);
        }
    }
}

/// 某个代次在提交时是否已经过期（给单测与将来的调试用）。
fn is_stale(tag: u64, latest: &AtomicU64) -> bool {
    tag != latest.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    #[test]
    fn newer_read_supersedes_an_older_one() {
        let latest = Arc::new(AtomicU64::new(5));
        assert!(is_stale(4, &latest), "4 < 5：更早的读必须作废");
        assert!(!is_stale(5, &latest), "最新那一代仍要跑");
    }

    #[test]
    fn install_is_idempotent_and_runs_jobs() {
        install();
        install(); // 第二次必须是无害空操作
        let (tx, rx) = mpsc::channel();
        submit_write(move || {
            let _ = tx.send("write-ran");
        });
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "write-ran",
            "写车道的任务要真的被执行"
        );

        let (tx, rx) = mpsc::channel();
        submit_read(move || {
            let _ = tx.send("read-ran");
        });
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "read-ran",
            "读车道的任务要真的被执行"
        );
    }

    /// 读车道的最新代次被后来的提交顶掉时，早先那条**不执行**（sender 被丢）。
    #[test]
    fn a_superseded_read_is_skipped() {
        install();
        // 先占住读线程（300ms），让后面两条真的排进队列；它们都在第一条跑着时提交，
        // 于是第二条开跑前代次已经被第三条顶掉 —— 不需要靠睡眠抢时序。
        submit_read(|| std::thread::sleep(Duration::from_millis(300)));
        let (tx, rx) = mpsc::channel();
        submit_read(move || {
            let _ = tx.send("should-not-run");
        });
        let (tx2, rx2) = mpsc::channel();
        submit_read(move || {
            let _ = tx2.send("third");
        });
        assert_eq!(
            rx2.recv_timeout(Duration::from_secs(2)).unwrap(),
            "third",
            "最新那一代要跑"
        );
        // 第二条要么被 worker 跳过（sender 丢 → Err），要么根本没排上：
        // 两种情况都说明它没有产出结果。
        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "被顶掉的读不该有结果"
        );
    }
}

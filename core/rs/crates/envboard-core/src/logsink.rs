//! 有界日志总线：数据面的 write_line 永不阻塞；满了丢弃并计数。
//!
//! v2 的教训是实测出来的：默认详细度下 213 个请求写满 64 KiB 管道，之后
//! 所有客户端一起挂。v3 形态从"子进程写管道"换成"任务往 SyncSender 投递"，
//! 那条纪律的实现就落在这里 —— try_send 立即返回，丢弃计数上报告。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};

use envboard_core_api::ports::LineWriter;

/// 队列容量：单机调试代理的突发日志以千行计足够吸收；超出即丢（并计数）。
pub const QUEUE_LINES: usize = 1024;

pub struct BoundedLinePump {
    tx: SyncSender<String>,
    dropped: AtomicU64,
}

impl std::fmt::Debug for BoundedLinePump {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoundedLinePump")
            .field("dropped", &self.dropped())
            .finish_non_exhaustive()
    }
}

impl BoundedLinePump {
    /// 起一条泵线程（阻塞写交给它；泵线程随最后一个句柄析构自然收尾）。
    pub fn start(writer: Arc<dyn LineWriter>) -> Arc<Self> {
        let (tx, rx) = sync_channel::<String>(QUEUE_LINES);
        let pump = Arc::new(BoundedLinePump {
            tx,
            dropped: AtomicU64::new(0),
        });
        std::thread::Builder::new()
            .name("envboard-log-pump".to_string())
            .spawn(move || {
                while let Ok(line) = rx.recv() {
                    // 写端失败（磁盘满等）不回传、不 panic：丢行是可见行为，
                    // 卡数据面不是。真错误面在管理面的日志文件维护。
                    writer.write_line(&line);
                }
            })
            .expect("log pump thread");
        pump
    }

    /// 交给引擎的写端。
    pub fn writer(self: &Arc<Self>) -> Arc<BusWriter> {
        Arc::new(BusWriter { pump: self.clone() })
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

#[derive(Debug)]
pub struct BusWriter {
    pump: Arc<BoundedLinePump>,
}

impl LineWriter for BusWriter {
    fn write_line(&self, line: &str) {
        if self.pump.tx.try_send(line.to_string()).is_err() {
            self.pump.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    #[derive(Debug, Default)]
    struct SlowWriter(Mutex<Vec<String>>);

    impl LineWriter for SlowWriter {
        fn write_line(&self, line: &str) {
            std::thread::sleep(Duration::from_millis(2));
            self.0.lock().unwrap().push(line.to_string());
        }
    }

    #[test]
    fn writers_never_block_and_drops_are_counted() {
        let pump = BoundedLinePump::start(Arc::new(SlowWriter::default()));
        let writer = pump.writer();
        let start = Instant::now();
        for index in 0..(QUEUE_LINES * 3) {
            writer.write_line(&format!("line-{index}"));
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(400),
            "投递必须立即返回: {elapsed:?}"
        );
        assert!(
            pump.dropped() >= (QUEUE_LINES * 2) as u64 - 40,
            "队列满必须丢并计数: {}",
            pump.dropped()
        );
        std::thread::sleep(Duration::from_millis(300));
        drop(pump);
    }
}

//! 抓包读侧的「有新行」版本号（S3）。
//!
//! 同进程、单消费者：NSH 抓包页起一个 100ms 的轻定时器，每拍只看一眼版本号；
//! 变了才去读一段尾部窗口，没变就什么都不做（空闲时连 `tail` 调用都不发）。
//!
//! 为什么是版本号而不是逐条广播：读侧拿到「有变化」之后本来就要读一段窗口
//! （一次读能补上多条），逐条排队只是把缓冲再抄一份进通道，还要做去重与背压。
//! 版本号只增不减、按环境分开；清空/开关这类「没有新记录但界面该重读」的变化
//! 也走同一个计数，读侧只有一条判断：变了就读。

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// 每环境的抓包数据版本（0 = 从未变化过）。
#[derive(Default)]
pub struct CaptureWatch {
    /// 全局单调计数（跨环境唯一，日志里对照方便）。
    next: AtomicU64,
    versions: Mutex<HashMap<String, u64>>,
}

impl CaptureWatch {
    /// 标记某环境的抓包数据有变化（新记录入队、清空、开关）。返回新版本号。
    ///
    /// 并发下两拍可能反序落到 map 上（先拿到 6 的先插、拿到 5 的后插），所以只在新值
    /// 更大时才写：版本号对读侧是「变没变」，倒退会让读侧漏掉一次重读。
    pub fn bump(&self, env: &str) -> u64 {
        let version = self.next.fetch_add(1, Ordering::AcqRel) + 1;
        let mut versions = self.versions.lock().unwrap();
        let entry = versions.entry(env.to_owned()).or_insert(0);
        if version > *entry {
            *entry = version;
        }
        version
    }

    /// 当前版本。读侧只比较「与上次看到的是否相同」。
    pub fn version(&self, env: &str) -> u64 {
        self.versions
            .lock()
            .unwrap()
            .get(env)
            .copied()
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_per_environment_and_monotonic() {
        let watch = CaptureWatch::default();
        assert_eq!(watch.version("a"), 0, "从未变化 = 0");
        let first = watch.bump("a");
        assert_eq!(watch.version("a"), first);
        assert_eq!(watch.version("b"), 0, "别的环境不受影响");
        let second = watch.bump("a");
        assert!(second > first, "只增不减");
        watch.bump("b");
        assert_eq!(watch.version("a"), second, "b 的变化不改 a 的版本");
    }

    #[test]
    fn concurrent_bumps_stay_monotonic_per_environment() {
        let watch = std::sync::Arc::new(CaptureWatch::default());
        let mut workers = Vec::new();
        for _ in 0..4 {
            let watch = watch.clone();
            workers.push(std::thread::spawn(move || {
                for _ in 0..250 {
                    watch.bump("a");
                }
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
        assert!(watch.version("a") >= 1, "并发 bump 之后版本仍在");
        // 只验单调与不丢：最后一次 bump 是全局最大
        assert_eq!(watch.version("a"), watch.next.load(Ordering::Acquire));
    }
}

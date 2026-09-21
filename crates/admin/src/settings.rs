//! 设置与运行时概览域（只读）+ 快照流的读取原语。

use envboard_engine::{CoreCapabilities, Error};
use envboard_protocol::EnvView;

use super::AdminService;

impl AdminService {
    pub fn capabilities(&self) -> CoreCapabilities {
        self.manager.capabilities()
    }

    pub fn config(&self) -> envboard_manager::ManagerConfig {
        self.manager.config().clone()
    }

    /// 快照流轮询源：全环境视图 + 账本代次。
    ///
    /// `state_generation` 是 **advisory**（健康变化不经过控制面写入，
    /// 同代次两帧内容仍可能不同）—— 调用方不得据此跳过渲染。
    pub fn snapshot(&self) -> Result<(u64, Vec<EnvView>), Error> {
        Ok((self.manager.state_generation(), self.manager.list()?))
    }

    pub fn events_dropped(&self) -> u64 {
        self.manager.events_dropped()
    }

    /// 快照流的代次游标（advisory，见 [`Self::snapshot`]）。
    pub fn state_generation(&self) -> u64 {
        self.manager.state_generation()
    }
}

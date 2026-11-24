//! 调试域：工作区级抓包单例 + 每环境抓包读侧 + 故障注入探针。

use std::sync::Arc;

use envboard_engine::{
    CaptureDelta as EngineCaptureDelta, CaptureView as EngineCaptureView, Error, Record,
};
use envboard_protocol::{CaptureRows, DebugStartReq, DebugView};

use super::AdminService;

impl AdminService {
    pub fn captures(&self, name: &str, limit: usize) -> Result<Option<EngineCaptureView>, Error> {
        self.manager.captures(name, limit)
    }

    /// 单条记录本体（`Arc` 共享：调用方拿到的是引擎缓冲里那一份，不做深拷贝）。
    pub fn capture_detail(&self, name: &str, request_id: u64) -> Result<Option<Arc<Record>>, Error> {
        self.manager.capture_detail(name, request_id)
    }

    /// 列表用的瘦行（尾部窗口，按 request_id 升序；只带表格与过滤要的字段，不搬正文）。
    pub fn capture_rows(&self, name: &str, limit: usize) -> Result<Option<CaptureRows>, Error> {
        self.manager.capture_rows(name, limit)
    }

    pub fn clear_capture(&self, name: &str) -> Result<bool, Error> {
        self.manager.clear_capture(name)
    }

    /// 抓包数据版本（S3）：同进程读侧据此判断要不要去读窗口，0 = 从未变化。
    pub fn capture_version(&self, name: &str) -> u64 {
        self.manager.capture_version(name)
    }

    /// `POST /api/debug`：请求体是 typed `DebugStartReq {env}`。
    pub fn start_debug(&self, request: &DebugStartReq) -> Result<DebugView, Error> {
        self.manager.start_debug(&request.env)
    }

    pub fn stop_debug(&self) -> Result<bool, Error> {
        self.manager.stop_debug()
    }

    pub fn debug_view(&self) -> Option<DebugView> {
        self.manager.debug_view()
    }

    /// 调试实时流的增量读原语（游标 = 会话内最新 request_id）。
    pub fn debug_delta(&self, after: u64, limit: usize) -> Option<(String, EngineCaptureDelta)> {
        self.manager.debug_delta(after, limit)
    }

    /// test-only 探针（`POST /api/_fault`）。
    pub fn inject_fault(&self, name: &str, reason: &str) -> bool {
        self.manager.inject_fault(name, reason)
    }
}

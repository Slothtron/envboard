//! 调试域：工作区级抓包单例 + 每环境抓包读侧 + 故障注入探针。

use envboard_engine::{
    CaptureDelta as EngineCaptureDelta, CaptureView as EngineCaptureView, Error,
};
use envboard_protocol::{DebugStartReq, DebugView};
use serde_json::Value;

use super::AdminService;

impl AdminService {
    pub fn captures(&self, name: &str, limit: usize) -> Result<Option<EngineCaptureView>, Error> {
        self.manager.captures(name, limit)
    }

    pub fn capture_detail(&self, name: &str, request_id: u64) -> Result<Option<Value>, Error> {
        self.manager.capture_detail(name, request_id)
    }

    pub fn clear_capture(&self, name: &str) -> Result<bool, Error> {
        self.manager.clear_capture(name)
    }

    /// `POST /api/debug`：入口先过 `DebugStartReq` 形状校验。
    pub fn start_debug(&self, body: &Value) -> Result<DebugView, Error> {
        let req = Self::parse_request::<DebugStartReq>("request", body)?;
        self.manager.start_debug(&req.env)
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

//! 活动与对比域（只读）。

use envboard_engine::Error;
use serde_json::Value;

use super::AdminService;

impl AdminService {
    /// 控制面审计事件（`GET /api/history`）。
    pub fn history(&self, name: Option<&str>, limit: usize) -> Result<Vec<Value>, Error> {
        self.manager.history(name, limit)
    }

    /// 跨环境确定性查账（`GET /api/compare`）。
    pub fn compare(&self, host: &str) -> Result<Value, Error> {
        self.manager.compare(host)
    }
}

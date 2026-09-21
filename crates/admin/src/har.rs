//! HAR 导入会话域。

use envboard_engine::Error;
use serde_json::Value;

use super::AdminService;

impl AdminService {
    pub fn har_import(&self, name: &str, body: &Value) -> Result<Value, Error> {
        self.manager.har_import(name, body)
    }

    pub fn har_list(&self) -> Vec<Value> {
        self.manager.har_list()
    }

    pub fn har_get(&self, id: u64, limit: usize) -> Option<Value> {
        self.manager.har_get(id, limit)
    }

    pub fn har_delete(&self, id: u64) -> bool {
        self.manager.har_delete(id)
    }
}

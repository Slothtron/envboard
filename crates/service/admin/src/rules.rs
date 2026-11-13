//! 规则账本域：导入 / 列举 / 读取 / 删除。

use std::path::PathBuf;

use envboard_engine::Error;

use super::AdminService;

impl AdminService {
    /// `POST /api/rules`：包装对象 `{name, text}`（形状校验在调用方 handler 已做，
    /// 这里保留 name 的 domain 校验路径）。
    pub fn import_rules(&self, name: &str, source_text: &str) -> Result<PathBuf, Error> {
        self.manager.import_rules(name, source_text)
    }

    pub fn rules_list(&self) -> Result<Vec<String>, Error> {
        self.manager.rules_list()
    }

    pub fn rules_read(&self, name: &str) -> Result<String, Error> {
        self.manager.rules_read(name)
    }

    pub fn rules_delete(&self, name: &str) -> Result<(), Error> {
        self.manager.rules_delete(name)
    }
}

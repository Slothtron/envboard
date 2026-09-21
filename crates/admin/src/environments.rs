//! 环境域：CRUD + 生命周期 + 每环境读取（logs / trajectory / captures 读侧）。

use envboard_engine::{CoreInfo, Error};
use envboard_protocol::{EnvView, ReconcileReport};
use serde_json::Value;

use envboard_protocol::{EnvCreateReq, EnvPatchReq};

use super::AdminService;

impl AdminService {
    pub fn list(&self) -> Result<Vec<EnvView>, Error> {
        self.manager.list()
    }

    pub fn get(&self, name: &str) -> Result<EnvView, Error> {
        self.manager.get(name)
    }

    /// `POST /api/environments`：入口先过 `EnvCreateReq` 形状校验。
    pub fn create(&self, body: &Value) -> Result<EnvView, Error> {
        Self::parse_request::<EnvCreateReq>("request", body)?;
        self.manager.create(body)
    }

    /// `PATCH /api/environments/:name`：三态（未提及 / 显式 null / 给值）由
    /// `EnvPatchReq` 保住，语义校验在 domain merge。
    pub fn update(&self, name: &str, patch: &Value) -> Result<EnvView, Error> {
        Self::parse_request::<EnvPatchReq>("request", patch)?;
        self.manager.update(name, patch)
    }

    pub fn remove(&self, name: &str) -> Result<(), Error> {
        self.manager.remove(name)
    }

    pub async fn start(&self, name: &str) -> Result<EnvView, Error> {
        self.manager.start(name).await
    }

    pub async fn stop(&self, name: &str) -> Result<EnvView, Error> {
        self.manager.stop(name).await
    }

    pub async fn restart(&self, name: &str) -> Result<EnvView, Error> {
        self.manager.restart(name).await
    }

    pub fn reallocate_port(&self, name: &str) -> Result<EnvView, Error> {
        self.manager.reallocate_port(name)
    }

    pub async fn reconcile(&self) -> Result<ReconcileReport, Error> {
        self.manager.reconcile().await
    }

    pub fn logs_tail(&self, name: &str, lines: usize) -> Result<Vec<String>, Error> {
        self.manager.logs_tail(name, lines)
    }

    /// REST 轨迹窗口：`(cursor, events)`，与轨迹流帧同形。
    pub fn trajectory(&self, name: &str, limit: usize) -> Result<(u64, Vec<Value>), Error> {
        self.manager.trajectory(name, limit)
    }

    /// SSE 轨迹流的增量读原语（游标 = jsonl 字节偏移）。
    pub fn trajectory_since(&self, name: &str, offset: u64) -> Result<(u64, Vec<Value>), Error> {
        self.manager.trajectory_since(name, offset)
    }

    pub fn is_live(&self, name: &str) -> bool {
        self.manager.is_live(name)
    }

    pub fn core_info(&self) -> CoreInfo {
        self.manager.core_info()
    }
}

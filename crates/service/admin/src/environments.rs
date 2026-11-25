//! 环境域：CRUD + 生命周期 + 每环境读取（logs / captures 读侧）。

use envboard_engine::{CoreInfo, Error};
use envboard_protocol::{EnvCreateReq, EnvPatchReq, EnvView, ReconcileReport};

use super::AdminService;

impl AdminService {
    pub fn list(&self) -> Result<Vec<EnvView>, Error> {
        self.manager.list()
    }

    pub fn get(&self, name: &str) -> Result<EnvView, Error> {
        self.manager.get(name)
    }

    /// `POST /api/environments`：请求体是 typed `EnvCreateReq`（`deny_unknown_fields`
    /// 的形状校验在反序列化时就完成，适配器解码失败即 `invalid_config`）。
    pub fn create(&self, request: &EnvCreateReq) -> Result<EnvView, Error> {
        let body = Self::ledger_value(request)?;
        self.manager.create(&body)
    }

    /// `PATCH /api/environments/:name`：三态（未提及 / 显式 null / 给值）由
    /// `EnvPatchReq` 保住，语义校验在 domain merge。
    pub fn update(&self, name: &str, patch: &EnvPatchReq) -> Result<EnvView, Error> {
        let body = Self::ledger_value(patch)?;
        self.manager.update(name, &body)
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

    /// 退出前收摊（T3）：停掉所有在跑实例但保留 `desired`，返回 (停掉几个, 失败说明)。
    pub async fn park_all(&self) -> (usize, Vec<String>) {
        self.manager.park_all().await
    }

    /// 退出前把账本的 WAL 合进主库（T3）：`state.db-wal` 归零。
    pub fn checkpoint_state(&self) -> Result<(), Error> {
        self.manager.checkpoint_state()
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

    pub fn is_live(&self, name: &str) -> bool {
        self.manager.is_live(name)
    }

    pub fn core_info(&self) -> CoreInfo {
        self.manager.core_info()
    }
}

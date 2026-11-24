//! 上游代理账本域。

use envboard_engine::Error;
use envboard_protocol::{ProxyPutReq, ProxyView};

use super::AdminService;

impl AdminService {
    pub fn proxy_list(&self) -> Result<Vec<ProxyView>, Error> {
        self.manager.proxy_list()
    }

    pub fn proxy_get(&self, name: &str) -> Result<ProxyView, Error> {
        self.manager.proxy_get(name)
    }

    /// `POST /api/proxies`（put 语义）：请求体是 typed `ProxyPutReq`。
    pub fn proxy_put(&self, request: &ProxyPutReq) -> Result<ProxyView, Error> {
        let body = Self::ledger_value(request)?;
        self.manager.proxy_put(&body)
    }

    pub fn proxy_delete(&self, name: &str) -> Result<(), Error> {
        self.manager.proxy_delete(name)
    }
}

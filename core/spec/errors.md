# envboard 错误码契约

语言中立契约。**core 判定，适配器只翻译**（AGENTS.md §5.4）。REST 面返回
`{"error": {"code": ..., "message": ..., "field": ...}}`，`field` 为可选的点分路径。

| code | HTTP | 含义 | 触发场景（示例） |
|---|---|---|---|
| `invalid_config` | 400 | 配置非法，**加载即失败** | 环境名不合规；`dns_servers` 写成 `10.0.0.53:53`；`hosts` 的值不是 IP；未知字段 |
| `not_found` | 404 | 目标环境不存在 | `GET /envboard/api/environments/nope` |
| `conflict` | 409 | 状态冲突 | 删除当前激活环境；删除最后一个环境；环境重名 |
| `dns_failure` | 502 | DNS 查询失败（**非** NXDOMAIN） | 上游 SERVFAIL、超时、无可用 DNS 服务器 |
| `store_failure` | 500 | 状态文件读写失败 | 权限不足、JSON 损坏、磁盘满 |
| `upstream_unavailable` | 503 | 宿主能力缺失 | 非 mitmweb 宿主却请求 Dashboard |
| `disabled` | 409 | 功能被显式关闭 | `envboard_enabled=false` |
| `internal_error` | 500 | 未归类的内部错误 | 兜底，不应出现 |

## 归一化规则

1. **NXDOMAIN / NODATA 不是错误**。它们是合法的"查无此记录"，以结果形式返回
   （`rcode: "nxdomain" | "nodata"`），绝不抛异常、绝不写 5xx。
2. **单条查询失败不得拖垮整批**。批量解析里某一条失败 → 该条 `rcode` 标记失败，
   其余照常返回。
3. **`dns_failure` 与 `invalid_config` 必须分开**：前者可重试，后者需要改配置。
   禁止把配置错误伪装成网络错误（反之亦然）。
4. **禁止静默降级**。环境定义里出现无法识别的字段时必须响亮失败；唯一的
   "降级"是 `dns_servers` 为空时的语义 —— 那不是降级，是**显式语义**：
   跟随操作系统 DNS。

## rcode 取值（结果载荷内）

| rcode | 含义 |
|---|---|
| `noerror` | 有答案 |
| `nxdomain` | 域名不存在 |
| `nodata` | 域名存在但没有该类型的记录 |
| `servfail` | 上游解析失败 |
| `timeout` | 超时 |
| `unsupported` | 环境不具备该能力（如缺 `mitmproxy_rs`、无可用 DNS 服务器） |
| `error` | 其它错误 |

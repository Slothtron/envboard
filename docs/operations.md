# 验证与排障

## 验证

任务入口统一是 **mise**（只管本地开发环境；`mise run` 列出全部任务）。判据一律在
Rust 测试里，任务只做编排，遇错即停：

```bash
mise run verify            # 全量：Rust 层 + 契约层 + 桌面层
mise run verify-rust       # fmt / clippy / check / build / test / doc
mise run verify-contract   # 84 个 fixture
mise run verify-desktop    # 桌面三套：smoke / artifact / 命令级契约
mise run verify-frontend   # pnpm install + build（tsc 内含）
mise run verify-live       # 实机层：引擎直驱（只需 openssl 与 curl）
```

`--locked --offline` 全程在位；离线可构建是验收项。

| 层 | 手段 | 关键断言 |
| --- | --- | --- |
| rust | `cargo fmt --check` / `clippy -D warnings` / `check` / `build` / `test --workspace` / `doc` | 编译、lint、单测 + 冒烟（单实例锁、端口分配、reconcile、健康判定、日志轮转、环境编辑热/停机）+ rustdoc 可构建 |
| contract | `cargo test -p envboard-contract-tests` | **84 个 fixture** 的形状与语义；失败用例钉错误码；能力注册表 ↔ fixture 目录一致 |
| desktop | `cargo test --locked --offline`（crates/app/desktop） | smoke（help 面 / 坏参数退出码 / 二次启动激活）+ artifact（引擎标记、产品身份、dist 底线、IPC 接线、导航三集合、CSP hash）+ 命令级契约 **14 条** |
| frontend | `pnpm install --frozen-lockfile` + `pnpm build` | tsc 零错 + Vite 产出 dist |
| live | `live_manager` `--ignored` + CDP 真机走查 | 引擎直驱（真端口 / 真 TLS）；桌面全链路（真窗口 + IPC 流 + 真代理流量） |

发布物内容判据住在 `crates/app/desktop/tests/artifact.rs`：二进制里真的带着进程内
引擎与内嵌前端（引擎线程名标记、产品身份、dist 体积下限、外链资产、IPC 命令接线、
导航 ↔ 视图一一对应、CSP 严格档）。界面侧的验收是**命令级契约 + CDP 真机走查**。

## 健康与错误码

环境实际状态全部来自**内存报告**：

| 状态 | 含义 |
| --- | --- |
| `stopped` | 无登记实例 |
| `starting` | 已下发启动，绑定结果未定态 |
| `running` | 引擎线程存活且 listener 已绑定；配置装配即生效 |
| `port_conflict` | 绑定失败（EADDRINUSE）；**不换端口**，等一次显式动作 |
| `unhealthy` | 线程存活但异常：accept 持续失败，或配置被 apply 拒绝（旧快照仍在服务） |
| `failed` | 引擎线程终止（panic）→ reconcile 按 desired 自动重启 |

判定次序：**标记 → desired → 引擎内存报告**；权威判定在
`crates/service/manager/src/reconcile_health.rs`，视图派生同源。

错误码：`invalid_config`（含热更被拒）/ `not_found` / `conflict` / `port_conflict` /
`port_range_exhausted` / `store_failure` / `internal_error` —— 语义与可重试性以
`envboard-engine::api::error` 模块文档为准。

## 故障排查

`store_failure` 的文案**一定带它动的是哪个路径**（`cannot create the CA directory
<path>: …`）。看到 `cannot … <path>: 拒绝访问。 (os error 5)` 先查**进程权限**，
不要查代码。Windows 上最隐蔽的一种是 **exe 所在目录被打了 Low 完整性标签**：进程
继承 Low 完整性后，`%LOCALAPPDATA%` 这类 Medium 位置读得通、写不通，症状正是启动即
`store_failure`。判据：`icacls <exe>` 看 `Mandatory Label`，或把同一份 exe 拷到用户
主目录再跑；修复 `icacls <目录> /setintegritylevel (OI)(CI)M`。

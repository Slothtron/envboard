# envboard

单一发布物：Windows 桌面应用 `envboard`（Tauri 2 壳 + 进程内代理引擎）。
本文件面向所有在仓库内工作的 AI agent 与人类贡献者，约定结构、契约与验证规则；
新增代码必须遵守，发现偏差先归位再继续。人的导览见 [README](README.md)，
使用与构建见 [docs/](docs/)。

## 仓库形态

- `crates/` 按依赖方向分三层，层内平铺；依赖单向：
  `protocol/events ← engine ← manager ← admin ← host ← desktop`。
- **只有一个交付形态**：`crates/app/desktop`（`[[bin]] envboard`）。不要新增第二个
  宿主二进制；界面在 `frontend/`，数据面走 IPC 直调 `AdminService` 门面，不起监听端口。
- `crates/app/desktop` **自成 workspace**（tauri-build 的静态 CRT 会毒化根工作区
  doctest），根 `Cargo.toml` 显式 `exclude` 它；tauri 系依赖只登记在它自己的清单。
- 全部 crate `publish = false`：发布物是二进制，不是 crate。

## 分层规则

| crate | 拥有 |
| --- | --- |
| `core/protocol` | UI 边界词汇：视图 DTO、请求 DTO、帧形状、游标、错误信封（纯 serde） |
| `core/events` | 事件词汇：控制面审计 / 数据面轨迹 / JSONL 两档存取 |
| `core/engine` | `ProxyEngine` 接缝、错误码、端口；`domain` / `rules` 纯逻辑 + 引擎本体 |
| `core/engine-fake` | 引擎测试替身（真绑端口，测试专用） |
| `service/manager` | 环境 CRUD、端口分配、账本、锁、健康判定、reconcile、热装配 |
| `service/admin` | `AdminService` 门面：入口校验 + 用例编排，传输无关 |
| `service/host` | 唯一组合根：引擎装配、共享 CA 物化、管理器构造（含单实例锁） |
| `app/desktop` | Tauri 壳、IPC 命令、推送流、CSP、WebView2 加固 |
| `tests/contract-tests` | golden fixture 的形状与语义消费 |

1. **引擎装配符号只许出现在 `crates/service/host`。** admin 与 desktop 只认
   `AdminService` 与 `CaControl`，不认识装配细节；换引擎实现不动界面。
2. **命令行解析库（clap）只许出现在 `envboard-desktop`。** 状态目录与端口区间是
   启动方式，不是界面设置；其它 crate 出现解析库即违规。
3. `manager` 的 `impl` 按域拆文件（`lifecycle` / `rules_store` / `proxies` /
   `reconcile_health` / `ledger` / `observability` / `debug_capture` / `projection`
   …），跨域共享私有项留在 `manager.rs` 并收窄可见性；一个文件不为别的域定义方法。
4. `domain` / `rules` 是纯逻辑：不引 tokio / rustls 等运行时库。
5. GUI 框架只用 Tauri 2；不引入第二套界面栈。

## 契约

契约 = **Rust 类型 + crate 文档（rustdoc）+ 可执行 fixture**，没有独立的契约文档层：

- 视图 DTO / 帧 / 游标 / 安全约定 → `envboard-protocol`；
- 错误码 / 可重试性 → `envboard-engine::api::error`；
- 方法语义 / 生效方式 → `envboard-admin`；
- 规则语法 / 归一化 / 渲染 → `envboard-engine::rules`；
- 环境字段不变量 / PATCH → `envboard-engine::domain::environment`；
- 事件信封 / 两档存储 → `envboard-events`；
- 能力清单 / 端口 / 账本 → `envboard-manager`。

规则：

1. 改契约的顺序是 **先判断"契约该不该改" → 改 crate 文档与类型 → 同步 fixture**；
   不跟随实现反过来改契约。
2. fixture 总数 **84** 由 contract 测试钉住；增删能力要同时改 `envboard-manager` 的
   能力清单与 `tests/contract-tests/fixtures/` 目录。
3. 界面错误映射唯一定义在 `frontend/src/errors.ts`；展示派生（健康字面量、需处理
   判定、字节短表示）唯一定义在 `frontend/src/data.ts` —— Rust 侧不维护第二份。
4. `cargo doc --workspace --no-deps` 在验证链里：文档必须能编译。

## 状态目录与持久化

- 状态目录布局的**唯一实现**是 `crates/service/manager/src/state.rs` 的 `layout`
  模块；别的文件一律经常量拼路径，不得自拼字面量。
- 状态目录之外不写任何位置：没有注册表项、没有 `%ProgramFiles%`、没有 `/etc`。
  换位置只有 `$ENVBOARD_STATE_DIR`（整套一起换）或 exe 旁的 `envboard.portable` 标记。
- 账本权威是 `state.db`（SQLite + WAL，单事务原子写）；规则物化文件可再生。
  `ui.json` 只放界面偏好（主题 + 窗口几何），不作为业务状态。
- 凭据只进不出：视图只暴露布尔，日志 / argv / 错误文本不回显；CA 私钥不进任何
  交付给界面的结构。
- 窗口生命周期：关窗销毁窗口与 WebView、进程与引擎线程常驻；托盘重建窗口。
  不要注册前端 `onCloseRequested`（那会让窗口永远走不到销毁）；重建窗口要
  **先隐藏建窗、贴好几何、页面首帧后再显示**。

## 构建与工具链

- Rust **1.98.0**（`rust-toolchain.toml`）、Node **24.16** / pnpm **10.18.2**
  （`mise.toml`）、tauri-cli（`cargo install tauri-cli --version "^2.11"`）。
- `cargo build` 经 `crates/app/desktop/build.rs` 自动跑 `pnpm install + build`；
  前端源码不变则不重跑。逃生门：`ENVBOARD_SKIP_FRONTEND_BUILD=1`（要求 dist 已在位）、
  `ENVBOARD_PNPM=<path>`。
- `Cargo.lock` 与 `frontend/pnpm-lock.yaml` 必须入库；`--locked --offline` 是验收
  口径，新增依赖要有理由。
- 本机直跑 cargo 用 `RUSTUP_TOOLCHAIN=1.98.0`；测试加
  `TMP=target/tmp-test TEMP=target/tmp-test`（Git Bash 的 TMP 会被 Rust 当成 `C:\tmp`）。
- 发布由 `.github/workflows/release.yml` 触发：推 `v*` 标签（或手动 dispatch）→
  `windows-latest` 构建 → 上传 `envboard.exe` 与 NSIS 安装包。标签必须等于
  `Cargo.toml` 的 `[workspace.package] version`。

## 测试与验收

| 层 | 命令 | 关注 |
| --- | --- | --- |
| Rust | `mise run verify-rust` | fmt / clippy `-D warnings` / check / build / test / doc |
| 契约 | `mise run verify-contract` | 84 个 fixture 的形状与语义 |
| 桌面 | `mise run verify-desktop` | smoke / artifact / 命令面契约 |
| 前端 | `mise run verify-frontend` | tsc 零错 + Vite 产出 dist |
| 实机 | `mise run verify-live` | 引擎直驱（需要 openssl 与 curl） |

- 测：输入→状态、纯逻辑（解析 / 格式化 / 边界）、刚修的回归（注释写清当时怎么坏）。
- 不测：标签文案、框架自身行为、墙钟相位（注入时钟，不 sleep 后断言）。
- 网络 / 时钟 / 环境用替身注入（`engine-fake` 是范例）；需要真宿主的标 `#[ignore]`，
  由 `verify-live` 触发。
- 发布物内容判据在 `crates/app/desktop/tests/artifact.rs`（只有那里拿得到
  `CARGO_BIN_EXE_envboard`）。
- 界面验收 = 命令面契约 + CDP 真机走查；UI 颜色只在 `frontend/src/globals.css`
  的令牌层定义，组件不写裸色值。

## 文档

- `README.md` 只做**项目介绍**：是什么、为什么、能力概览、架构一览、安装、文档索引。
  使用教程与操作细节放 `docs/`，不要回填进 README。
- `docs/`：`usage.md`（安装 / 接入 / 工作台 / 配置 / 状态目录）、`building.md`
  （发布与分发）、`architecture.md`（引擎 / 分层 / 契约位置）、`operations.md`
  （验证 / 健康 / 排障）、`limitations.md`（限制与非目标）。
- `CHANGELOG.md` 按**用户可见的变化**写，按版本分节；不写提交级流水账。
- 文档只引用仓库内路径；不要把本机路径、归档目录或仓库外材料写进文档。

## 提交纪律

- 一次变更一个主题；破坏性变更（配置 / 状态目录 / 命令面）写进 CHANGELOG 的升级注意。
- 文档与代码同一提交；改契约时 crate 文档、fixture、前端类型一起动。
- 提交前跑对应的 `mise run verify-*`；CI 只跑构建与发布，不替代本地验证。

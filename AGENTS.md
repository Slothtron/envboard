# envboard

单一发布物：Windows 桌面应用 `envboard`（NSH 0.6.1 原生壳 + 进程内代理引擎）。
本文件面向所有在仓库内工作的 AI agent 与人类贡献者，约定结构、契约与验证规则；
新增代码必须遵守，发现偏差先归位再继续。人的导览见 [README](README.md)，
使用与构建见 [docs/](docs/)。

## 仓库形态

- `crates/` 按依赖方向分四层，层内平铺；依赖单向：
  `protocol/events ← engine ← manager ← admin ← host ← nsh`，
  `ui` 是叶子（`app/nsh ← ui`，ui 不依赖任何 `envboard-*` 业务 crate）。
- **只发布一个交付形态**：`crates/app/nsh`（`[[bin]] envboard-nsh`，CI 收集产物时
  改名 `envboard.exe`）。数据面进程内直调 `AdminService` 门面，不起监听端口、
  不带 webview。Tauri 壳（`crates/app/desktop`）与 `frontend/` 已删除（git 历史可查）。
- `crates/ui` 不在任何 glob 内，根 `Cargo.toml` 的 `members` 显式列出它。
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
| `app/nsh` | NSH 0.6.1 壳（唯一发布物）：`model`（业务语义）/ `pages`（布局）/ `shell`（标题栏与状态条）/ `service`（数据面唯一入口） |
| `ui` | 设计系统：24 个颜色角色、度量、字阶、八原语、`themes/default.json` |
| `tests/contract-tests` | golden fixture 的形状与语义消费 |

1. **引擎装配符号只许出现在 `crates/service/host`。** admin 与 nsh 只认
   `AdminService` 与 `CaControl`，不认识装配细节；换引擎实现不动界面。
2. **本仓没有命令行解析库。** 状态目录与端口区间是启动方式，不是界面设置：
   走 `$ENVBOARD_STATE_DIR` / 便携标记，经 `HostOptions` 入参（见
   `envboard-admin` 门面文档「不在门面上的东西」）。clap 随 Tauri 壳退场。
3. `manager` 的 `impl` 按域拆文件（`lifecycle` / `rules_store` / `proxies` /
   `reconcile_health` / `ledger` / `observability` / `debug_capture` / `projection`
   …），跨域共享私有项留在 `manager.rs` 并收窄可见性；一个文件不为别的域定义方法。
4. `domain` / `rules` 是纯逻辑：不引 tokio / rustls 等运行时库。
5. GUI 框架：**发布物只用 NSH 0.6.1**，只允许出现在 `crates/ui` 与
   `crates/app/nsh`；不许引入第二套界面栈（Tauri / webview / GPUI 均已退场）。
6. `crates/ui/Cargo.toml` 不出现任何 `envboard-*` 业务依赖；`ui` 不读实例、不调门面、
   不认识“环境/规则/端口”。守门测试在 `crates/ui/tests/`（令牌对账、零 hex、依赖面）。
7. 界面颜色只在 `crates/ui/themes/default.json` 定义，代码里经 `tokens` 的具名角色取；
   页面与组件不许写 hex，也不许新增第二份主题文件。
8. 品牌名、窗口标题、`app_id`、图标与标题栏记号只在 `crates/app/nsh/src/brand.rs` 定义；
   shell 与 `main` 取值不写字面量。资产是 `assets/envboard.ico`（窗口图标）、
   `assets/brand-mark-32.rgba`（标题栏记号）与 `assets/app.rc`（`1 ICON "envboard.ico"`），
   都从原图 `crates/app/nsh/assets/icon.png` 由 `tools/envboard-brand/pack-icon.ps1` 重出；
   生成脚本不进发布物，换图标时三个文件一起换。
   **exe 内嵌资源是正式做法**：`build.rs` 用 `embed-resource` 把 `app.rc` 编进 exe，
   NSH 的 `set_icon()` 先 `filter(|_| !has_own_icon())` —— exe 自带 1 号图标资源时
   它直接返回、不再读 `AppOptions.icon` 的路径。所以 `brand::icon_path()` 只是兜底
   （开发宿主 / 没有图标资源的 runner），标题栏 / 任务栏 / Alt+Tab / 资源管理器同源。
9. 数据面只许出现在 `crates/app/nsh/src/service.rs`：`envboard-host` / `envboard-admin` /
   `envboard-protocol` / `envboard-engine` 的依赖只能被它用，`pages` / `shell` 认的是
   `model::Env` 与 `actions::Ctx`。写操作一律经 `actions`（同步门面直接调、异步门面
   交普通线程 `Handle::block_on` 回传），不在引擎线程里 `block_on`。

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
3. 界面错误翻译唯一定义在 `crates/app/nsh/src/actions.rs`（`fail`：行内条
   `code · message`，三层落点只响两层）；展示派生（健康字面量、需处理判定、
   字节短表示）唯一定义在 `crates/app/nsh/src/model.rs` 与 `model/capture.rs`
   —— 其它地方不维护第二份。
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
- 窗口生命周期：关窗默认**隐藏进托盘**（`ui.json` 的 `close_to_tray`，可在设置里
  改成关窗即退）；隐藏态页面循环随可见性门卸载，代理照常服务。托盘「退出」走
  正常关闭路径：**停所有实例 → WAL checkpoint 合库 → 关窗**。托盘菜单只有
  「显示窗口 / 退出」两项，不展示计数。

## 构建与工具链

- Rust **1.98.0**（`rust-toolchain.toml`）。纯 Rust 工具链，无 Node / pnpm /
  tauri-cli（随 Tauri 壳退场）。
- `Cargo.lock` 必须入库；`--locked --offline` 是验收口径，新增依赖要有理由。
- 本机直跑 cargo 用 `RUSTUP_TOOLCHAIN=1.98.0`；测试加
  `TMP=target/tmp-test TEMP=target/tmp-test`（Git Bash 的 TMP 会被 Rust 当成 `C:\tmp`）。
- 发布由 `.github/workflows/release.yml` 触发：推 `v*` 标签（或手动 dispatch）→
  `windows-latest` 构建 `envboard-nsh` release → 上传 `envboard.exe` 与便携 zip。
  标签必须等于 `Cargo.toml` 的 `[workspace.package] version`；版本号带预发布段
  直接失败（NSH 形态暂无预览身份隔离，预览渠道暂停）。

## 测试与验收

| 层 | 命令 | 关注 |
| --- | --- | --- |
| Rust | `mise run verify-rust` | fmt / clippy `-D warnings` / check / build / test / doc |
| 契约 | `mise run verify-contract` | 84 个 fixture 的形状与语义 |
| 界面 golden | `cargo test -p envboard-nsh --features harness --test golden --offline -- --ignored` | 八张基线（`crates/app/nsh/tests/golden/`）逐像素；`UPDATE_GOLDEN=1` 重拍；`NG_GOLDEN=1` 自动走 `model::fixture()`，不碰真状态目录。**要拉前台，别在用户主屏跑**：用 Anode 座席（`seat_display` 2560×1440@125%，`seat_exec` 里跑同一条命令），主屏锁屏时也能验；租约过期会还原显示，续租后须重设 |
| 中文 IME | 座席内 `ime9.ps1 -Exe <exe>` | 点 UIA 定位的输入框（不按版式推算坐标）→ Shift 切中文 → 打 nihao → 空格提交；用 UIA 读输入框值，不靠 OCR |
| 10k 性能（G3） | 座席内 `tools/envboard-p3-check/g3.ps1 -Exe <release exe>` | `NG_STRESS=10000` + `NG_PLAIN=1`，量首窗 / 峰值内存 / 滚动；量测进程自己 `SetProcessDPIAware()`，否则 125% 下窗口报 1294×848；滚动门槛以真机 `scroll_frame_ms.p95 ≤ 18ms` 终裁（已测 17.85），座席用 `scroll_throughput_ms`（处理成本，已测 1.7–2.2ms/格）代判；自查 `fg_ok` / `colors_now` / `scroll_moved_px`（必须 >0） |
| 数据面 | `cargo test -p envboard-nsh --test data_path --offline` | 真组合根 + 真门面：建→起→停→删一条链，错误信封是 `code·message` 不是 panic；状态目录固定在 `%TEMP%\eb-m3-gui` |
| 实机 | `mise run verify-live` | 引擎直驱（需要 openssl 与 curl） |

- 测：输入→状态、纯逻辑（解析 / 格式化 / 边界）、刚修的回归（注释写清当时怎么坏）。
- 长列表一律 `nsh_core::element::virtual_list`（只造视口附近的行），格内容用读
  `at.index()` 的位置闭包；`each` 只给几十条的小集合。全量铺开 10k 行 = 1.1 GB。
- 不测：标签文案、框架自身行为、墙钟相位（注入时钟，不 sleep 后断言）。
- 网络 / 时钟 / 环境用替身注入（`engine-fake` 是范例）；需要真宿主的标 `#[ignore]`，
  由 `verify-live` 触发。
- 发布物内容判据：发布构建按**默认特征**（`harness` 探针不进发布物）；界面验收 =
  golden 八例 + 数据面测试 + 座席真机走查（托盘 / 单实例 / 隐藏态 / 退出）。
  UI 颜色只在 `crates/ui/themes/default.json` 的令牌层定义，页面不写裸色值。

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

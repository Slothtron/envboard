# ADR-003 · web 形态下线：只留桌面形态

| 项 | 值 |
|---|---|
| 状态 | 已采纳（2026-10-29，P1 执行） |
| 相关 | [adr-001](adr-001-gpui-desktop-form.md)（为什么是 GPUI）、[adr-002](adr-002-composition-root-in-envboard-host.md)（组合根为什么与形态无关）、[spec/admin-api.md](../spec/admin-api.md)（门面契约）、[spec/desktop-parity.md](../spec/desktop-parity.md)（下线的前置证据） |
| 落点 | 删除 `frontend/`、`crates/web`、`crates/server`、`spec/ui.md`、`spec/design.md`、`tests/policy-tests/tests/ui_style.rs`、`scripts/systemd/`；`crates/desktop` 的二进制更名 `envboard`；根 `Cargo.toml`、`scripts/verify.py`、`mise.toml`、`tests/policy-tests/tests/{deps,toolchain,artifacts}.rs`、README 与本文件 | <!-- doc-scope-lint: allow -->

## 判断层：要解决什么

桌面形态在 P0.8 之后已经覆盖环境列表、环境详情（概览/配置/规则三页签）、规则集目录、
上游代理目录四屏，逐屏对账见 spec/desktop-parity.md。于是"要不要继续养 web 形态"
变成一个真问题，而不是信仰问题。

web 形态今天的成本：

| 成本 | 具体形态 |
|---|---|
| 两条工具链 | cargo + Node/pnpm/Vite/TS；`frontend/` 7,313 行（含 lockfile），`pnpm-lock.yaml` 单独占大头 |
| 一层传输绑定 | `crates/web` 1,540 行：axum 路由、SSE 三流、token/CSRF/CSP 中间件、`include_dir!` 内嵌产物、二维码 |
| 一套构建顺序 | 先 `pnpm build` 出 `dist/`，再 `cargo build` 内嵌；缺 dist 编译期响亮失败 —— 这条衔接链要人记着 |
| 两份界面逻辑 | 展示派生（健康字面量、"需处理"判定、字节短表示）在 `frontend/src/data.ts` 与 `crates/desktop/src/display.rs` 各一份，靠人保持同步 |
| 一份设计语汇 | `spec/design.md` + `spec/ui.md` + `ui_style` 门禁只服务 SPA；桌面用 GPUI 主题令牌，两套语言互相拽不动 | <!-- doc-scope-lint: allow -->

需求（按权重，合计 100）：

| # | 需求 | 权重 |
|---|---|---|
| R1 | 一个发布物、一条工具链，新人 clone 完 `cargo build` 就能干活 | 30 |
| R2 | 界面只有一套真相，改行为不用同步两处 | 25 |
| R3 | 本机数据直连，不经过 HTTP/token（这是桌面形态立项时的原始理由） | 15 |
| R4 | 保留"从别的机器/脚本管理环境"的能力 | 15 |
| R5 | 门禁与文档只描述一种界面纪律 | 15 |

## 决策层：候选与结论

**结论**：选 **A —— 全删，只留桌面形态**（含 `crates/web` 的 HTTP+SSE 面一起删）。
这是用户在两个岔路口都选了激进项的结果：先是"替换：桌面转正，web 逐步退役"，
后是"全删：只留桌面形态"而不是"保留无头 API"。

| 维度 | 权重 | A 全删（只留桌面） | B 删 SPA、留无头 HTTP 面 | C 双形态并存 |
|---|---|---|---|---|
| R1 一条工具链 | 30 | 30：Node 面彻底出库，`toolchain` 门禁转为回归守卫 | 24：不再有前端构建，但 axum/SSE/token 这套运行时面还在 | 5：两条工具链 + 构建顺序原样保留 |
| R2 一套界面真相 | 25 | 25：展示派生只剩 `display.rs` 一份 | 25：同 A | 8：两份派生逻辑长期并存 |
| R3 进程内直连 | 15 | 15 | 12：直连仍在，但多一条"同一动作有两个入口"的一致性负担 | 12：同 B |
| R4 远程/脚本能力 | 15 | 0：**明确放弃** | 15 | 15 |
| R5 一份界面纪律 | 15 | 15：`spec/ui.md`/`design.md`/`ui_style` 门禁一起退场 | 10：SPA 规范删了，但"给远程用户看的界面"这件事仍需要某种规范 | 5 | <!-- doc-scope-lint: allow -->
| **合计** | 100 | **85** | **86** | **45** |

B 与 A 只差 1 分，差在 R1：留着的 HTTP 面本身不轻（传输绑定 + 鉴权分档 + CSP + SSE
帧契约 + 它自己的集成测试），而**它服务的场景在本机工具这个定位上没人用** ——
`crates/server/tests/live_workbench.rs` 那 29 条实机断言里，走 HTTP 的部分 <!-- doc-scope-lint: allow -->
在桌面形态下由 `AdminService` 直连覆盖，不需要一条网络通路来证明。
所以那 1 分的优势（R4）不足以付它自己的维护成本。

**被否掉的中间项**：「先删 SPA，观察半年再决定 HTTP 面」。否掉的理由是它会留下
最坏的中间状态 —— 一个没有界面的服务端（`crates/web` 还在，但没人能从浏览器点它），
门禁、文档、CI 层都要为它保留"两套形态"的分支，而观察期里没人会来做最终裁决。

## 接受的损失（写清楚，别让人事后才发现）

1. **远程访问**：从另一台机器用浏览器管理环境的能力没有了。曾经的用法（监听非回环 +
   必须带 `--token`）连同它的鉴权分档一起退场。
2. **无头常驻服务**：`scripts/systemd/envboard.service` 随形态出库。GUI 二进制不适合
   当 systemd 用户服务跑（没有可交互的桌面会话时它起不来），所以这条不是"暂时没配"，
   是**能力本身没了**。
3. **脚本化管理面**：以前可以直接 `curl -X POST /api/environments/beta/start`。现在
   管理动作只有界面一条路。需要脚本化的场景（批量启停、CI 里拉起环境）要么等 CLI，
   要么走「复核触发条件」里那条复活路径。
4. **浏览器里的工作台体验**：多标签、可分享 URL、缩放与字体由浏览器管 —— 这些
   桌面形态不复刻（ADR-001 已定：不搬设计语汇，用 GPUI 现成组件）。

## 落地层：改了什么

| 原来 | 现在 |
|---|---|
| `frontend/`（Vite + React 19 + HeroUI v3 + Tailwind v4） | 出库。展示派生早已逐字移植进 `crates/desktop/src/display.rs`（带单测），没有丢逻辑 |
| `crates/web`（axum 路由 + SSE 三流 + 安全中间件 + `include_dir!` 内嵌 + 二维码） | 出库。传输面不再是任何一层的职责 |
| `crates/server`（`[[bin]] envboard` + `smoke`/`artifact`/`live_workbench` 三套测试） | 出库。二进制名 `envboard` 让给桌面 crate |
| `crates/desktop` 的 `[[bin]] envboard-desktop` | 更名 `envboard`：它是唯一发布物 |
| 根 `Cargo.toml` 的 `axum` / `tokio-stream` / `futures-util` / `include_dir` / `qrcode` | 出库（`Cargo.lock` 848 → 837 包）。依赖纪律改写：旧口径"CLI 之外不引大框架"已无 CLI，改成"只有一个可执行形态：桌面 GUI" |
| `deps` 门禁允许表里的 `envboard-web` / `envboard-server` 两行、装配盲区表里的两项 | 删。表里不再给它们留位置 —— 想带回来必须同时改表，改表就是评审点 |
| `ui_style` 门禁 + `spec/ui.md` + `spec/design.md` | 出库。界面纪律改由 spec/desktop-parity.md（行为对账）与 GPUI 主题令牌承担 | <!-- doc-scope-lint: allow -->
| `scripts/verify.py` 的 `frontend` 与 `artifact` 两层 | 删。默认层变为 policy + rust + contract；`live` 层只剩引擎侧的 `live_manager` |
| `crates/server/tests/artifact.rs`（发布物校验） | 拆成两半各归其位：仓库文件面事实进 `tests/policy-tests/tests/artifacts.rs`（必需文件、历史残留为零、`target/` 被忽略、四件 CA 测试资产必须在库），二进制内容事实进 `crates/desktop/tests/artifact.rs`（产物里真的带着进程内引擎的线程名标记） | <!-- doc-scope-lint: allow -->
| `mise.toml` 的 `node` / `pnpm` 工具钉与 `dev`/`build`/`typecheck`/`verify-frontend` 任务 | 删。`package` 不再依赖前端层，`desktop` 任务指向新二进制名 |
| `spec/admin-api.md`（HTTP 端点表 + 鉴权分档） | 改写成 **AdminService 门面契约**：方法、入参出参、错误码、生效方式（热/停机）、幂等与审计。端点路径不再是要约 |
| `spec/protocol.md` 的 SSE 帧契约 | 剪掉只服务于推流的部分；保留"快照是全量的、游标只是建议"这条语义 —— 桌面 1s 轮询器正是照它实现 |
| `toolchain` 门禁 | 保留，语义从"圈禁前端工具链"变成**回归守卫**：仓库里已无合法容身处，任何 Node/TS 面出现即红 |

## 代价与实测

| 项 | 数值 |
|---|---|
| 删除规模 | 40 个文件、12,545 行（`frontend/` 7,313 + `crates/web` 1,540 + `crates/server` 3,026 + 规范/门禁/部署件 666） |
| `Cargo.lock` | 848 → 837 包（−11） |
| 唯一发布物 | `target/release/envboard.exe` = **32,881,152 B**（`cargo build --release --offline --locked`，53.1 s 增量）。旧 web 宿主同名二进制是 7,902,720 B —— 差出去的 ~25 MB 就是 GUI 框架的价钱，ADR-001 已记；下线没有让它变贵，只是让**只剩这一颗** |
| 新人上手 | clone → `cargo build --release` 一步；不再需要 `mise install` 装 Node/pnpm，不再有"先前端后 Rust"的顺序要求 |

## 验证

| 判据 | 结果 |
|---|---|
| `cargo check --workspace --all-targets` | 通过（删完即验，无残留引用） |
| `cargo test -p envboard-policy-tests` | 全过：deps / doc_scope / naming / toolchain / **artifacts**（新） |
| `cargo clippy --workspace --all-targets -- -D warnings` | 干净 |
| `cargo build --release --offline --locked` | 通过（`--locked` 要求 `Cargo.lock` 与 manifest 一致） |
| 桌面二进制内容判据 | `crates/desktop/tests/artifact.rs`：产物里找得到 `envboard-engine-` 线程名标记 |
| 更名后的实机冒烟 | `target/release/envboard.exe --state-dir <种子目录>` 起界面（`PrintWindow` 183 色非黑帧，`shots/p1-single-artifact.png`）；`--help` 显示 `Usage: envboard.exe` 与桌面形态说明，参数表已无 `--listen` / `--token` |
| 全量测试 | `cargo test --workspace --no-fail-fast`：仅 `crates/engine/tests/backend.rs`（2）与 `data_plane.rs`（11）因本机拒绝访问当前用户证书存储失败，与 P0.8 同形（HEAD 亦如此） |
| 行为对账 | 下线前逐屏走查已记录（spec/desktop-parity.md），含"已知不等价"清单 |

## 复核触发条件

出现下面任何一条，就该重新讨论"要不要把 HTTP 面带回来"，而不是直接开写：

1. 有人真的需要**从另一台机器**管理环境（不是"想要"，是有场景）；
2. 需要**脚本化/CI 化**管理动作（批量启停、压测前拉起环境），而界面没有对应能力；
3. 需要**无头常驻**（没有交互桌面会话的机器）。

复活的最小路径是清楚的，这也是本次组合根重构（ADR-002）留下来的资产：
`envboard-host` 不动、`envboard-admin` 门面不动、`spec/admin-api.md` 已是传输无关的
方法契约 —— 只需要新加一个传输 crate 把门面映射成端点，并把它登记回 `deps` 门禁的
允许表与装配盲区表。**不要**复活的是：内嵌 SPA、两条工具链、以及为浏览器写的一套
设计语汇。

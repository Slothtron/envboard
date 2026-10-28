# ADR-002 · 引擎装配收进唯一组合根 `envboard-host`

| 项 | 值 |
|---|---|
| 状态 | 已采纳（2026-10-28） |
| 相关 | [README.md](../README.md)（分层与依赖方向）、[spec/admin-api.md](../spec/admin-api.md)（门面契约）、[spec/capabilities.md](../spec/capabilities.md) |
| 落点 | `crates/host`（新）、`crates/server/src/main.rs`（变薄）、`tests/policy-tests/tests/deps.rs`（第 8 条改判据） <!-- doc-scope-lint: allow —— 当时的落点，宿主已随 web 形态出库（见 adr-003） --> |

## 判断层：为什么要动

装配逻辑原来写在宿主二进制里：`crates/server/src/main.rs` 一处承担了引擎选择、 <!-- doc-scope-lint: allow —— 历史路径，宿主已出库 -->
共享 CA 物化、单实例锁、reconcile 顺序、日志体积照看。这在"只有一个二进制"的时候
没问题，但它有三个必然的走向：

1. **界面形态会增加**（本仓正在做原生桌面形态）。每个形态一个 `main.rs` 就得复制一遍
   装配，而这段是"漏了一项就出问题"的编排：忘了 flock 就有两个进程写同一份 `state.json`；
   忘了日志照看就写满磁盘；忘了先 reconcile 就有僵尸环境显示为停止。
2. **测试也需要一个真宿主**。界面形态的测试要的是"真管理器 + 假引擎"，如果测试自己
   复制一遍构造序列，测试通过就不代表生产能起。
3. **原判据是脆的**。门禁第 8 条原来允许装配符号出现在 `envboard-server` 的
   `src/main.rs` **这一个文件**里 —— "只许一个文件"这种约束，一旦装配被搬进第二个
   二进制就必然失效，而失效方式是**静默的**（新形态照样能跑，只是两份装配开始漂移）。

## 决策层：怎么定

**结论**：新建 `envboard-host`（lib，`publish = false`）作为全仓唯一组合根；
宿主二进制退化为「CLI 翻译 + 运行时归属 + 接上传输面」三段。

候选与取舍：

| 方案 | 判据 | 结论 |
|---|---|---|
| 各形态自己装配一份（现状延伸） | 改动最小 | 否：编排项漏一项就是真故障，且漂移不可见 |
| 把装配塞进 `envboard-admin` | 少一个 crate | 否：门面对依赖方向的红线是"不认识引擎装配"（deps 门禁第 8 条 + `crates/admin/src/lib.rs` 头注），塞进去等于自己拆自己的墙 |
| 抽成 `envboard-host`（采纳） | 界面形态与测试共用同一条构造路径；装配位置从"一个文件"变成"一个 crate"，判据可以更强 | 是 |

三条约束决定了它的形状：

- **管理器类型不出 crate**。`Prepared` 内部持 `Arc<Manager>` 是 `pub(crate)`，
  界面向外界只拿到 `admin: Arc<AdminService>`。所以新界面形态的产物边表里
  **没有** `envboard-manager`，它连类型名都拿不到 —— 依赖方向被 manifest 锁住，
  不靠自觉。
- **不依赖任何传输 crate**。host 不认识 axum / HTTP / SSE，也不认识 web 层的
  `CaAssets`：它返回自己的 `CaBundle { info, pem }`，由宿主把字段搬进 `CaAssets`。
  于是 host 不引入新的向下依赖，P1 拆掉 web 时 host 一行不用改。
- **测试注入口是显式的**。`prepare_with(options, Arc<dyn ProxyEngine>)` 与
  `prepare(options)` 共用同一段构造序列（`build_prepared`），假引擎走的正是生产路径。
  注入假引擎时不物化共享 CA，`CaBundle` 给空值 —— 这是刻意的：CA 物化属于引擎装配，
  属于 `prepare` 那条路。

## 落地层：改了什么、怎么验

搬移是逐条对应的，没有新增判定：

| 原来在 `crates/server/src/main.rs` | 现在在 | <!-- doc-scope-lint: allow —— 历史路径，宿主已出库 -->
|---|---|
| `build_engine()`（引擎 + 共享 CA + CA 重生物化的两条 `eprintln!`） | `crates/host/src/assembly.rs`；警告提升为 `Prepared::warnings`，宿主打印、界面渲染都来得及 |
| `Manager::new(...)` + 单实例锁 + `Conflict` 文案加工 | `crates/host/src/assembly.rs`（`build_prepared` / `ownership_conflict`） |
| reconcile → 初次 `maintain_logs()` → 30s 照看循环 | `crates/host/src/startup.rs`；报告作为返回值交回调用方，宿主 `println!` |
| `state_dir()` 的环境变量回退链 + `build_config()` | `crates/host/src/options.rs`（`HostOptions`）；`--port-range` 的**文本**解析也在这里（`parse_port_range`），CLI 参数本体仍留在各 `main.rs` |
| `WebConfig::parse(...)` | **不搬**：那是 web 形态的参数翻译，不属于共享组合根 |

一处行为差异，是有意的：CA 重生物化的警告原来在装配中途直接打到 stderr，现在先收集、
由宿主在 `prepare` 返回后立即打印。两条文本逐字未改，只是不再可能与管理器构造的
输出交错。

门禁（`tests/policy-tests/tests/deps.rs` 第 8 条）从"禁止 + 一个例外文件"变成三条判据：

1. **正判据**：`envboard-host/src/**` 里必须真的同时出现 `EngineBackend` 与 `SharedCa` ——
   装配被搬回某个二进制或界面时，这条先红；
2. **反判据**：`envboard-server` / `envboard-web` / `envboard-admin` / `envboard-desktop`
   的全部源码出现任一装配符号即红；
3. **登记判据**：凡产物边消费 `envboard-admin` 或 `envboard-manager` 的 crate，
   除组合根本身，必须在装配盲区表里 —— 新界面形态漏登记 = 白送装配权限，所以判红。

`envboard-engine` 与 `envboard-engine-fake` 不在盲区表内：前者定义这些符号，
后者是引擎的测试替身（必须与真后端同一抽象面）。

验证（2026-10-28 实测；**分层报告，不写成"全绿"**）：

| 检查 | 结果 |
|---|---|
| `cargo test -p envboard-host` | 7 条全过（3 单测 + 3 接缝 + 1 文档测试） |
| `cargo test -p envboard-policy-tests` | 全过（deps / doc_scope / naming / toolchain / ui_style） |
| `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings` | 干净 |
| `cargo test -p envboard-server` | `artifact` 1 条 + `smoke` 4 条全过 —— 含"第二个实例拒绝碰状态"（锁行为搬进组合根后依旧）与 token 双通道门禁 |
| `Cargo.lock` | 只新增 `envboard-host` 一个包，无版本漂移 |
| `cargo test -p envboard-server --test live_workbench -- --ignored` | **本机未跑通**：断言 758 处宿主返回 `internal_error`，原因是"打不开当前用户证书存储 `拒绝访问 (os error 5)`"。**已在 HEAD 的干净 worktree 上复现同一行同一消息**（`test result: FAILED ... trust store yielded no usable roots`），判定为本机环境条件而非本次改动 |
| `cargo test --workspace` 里的 `crates/engine/tests/backend.rs` | **本机同样未跑通**（两条，同一 `os error 5`）；已在 HEAD 的干净 worktree 上复现同一失败，判定为本机环境条件而非本次改动 |

**本机验证边界（写给下一个在这台机器上跑的人）**：这台机器上由 agent shell 派生的进程
被拒访问两类资源 —— 默认用户 `TEMP` 目录，以及**当前用户证书存储**。前者用
`TEMP=<repo>/target/tmp` 覆盖即可（否则任何写临时目录的测试都以 `os error 5` 失败）；
后者**没有绕过办法**：引擎起实例时要从系统信任根装 TLS 信任库
（`the system trust store yielded no usable roots`），所以凡是要"真起一个环境"的测试
（`live_workbench` 的 SSE/抓包段、`engine/tests/backend.rs`）在这台机器上必红。
这与界面形态无关，但意味着 **P0.1 的"HTTP 行为零变化"只被 `smoke`/`artifact`/
`contract` 覆盖到，SSE 端到端段要在能访问证书存储的会话里复跑**。

接缝测试钉的是组合根自己的职责：门面装出来真能用（写一次、读得到、快照全量）、
同一状态目录第二个宿主以 `Conflict` 响亮失败并带可操作文案、`startup()` 返回报告
而不是自己打印。`crates/host/tests/host.rs` 用独立端口窗口 + `--no-log-file`，
与既有测试同一手法。

**未迁的一项（记账）**：`maintain_logs()` 返回的轮转提示原来就被丢弃，本次照搬
不改（P0.1 的验收是"行为零变化"）。它该被渲染成界面提示，归 P2 的设置/日志页。

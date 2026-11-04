# ADR-001 · 桌面形态采用 GPUI（`gpui-kit`）

| 项 | 值 |
|---|---|
| 状态 | 已采纳（2026-10-28）；P0.2 实测数字已回填（见「代价」与「本机验证边界」） |
| 相关 | [README.md](../README.md)（界面形态与依赖方向）、[spec/protocol.md](../spec/protocol.md)、[spec/admin-api.md](../spec/admin-api.md)、[adr-002](adr-002-composition-root-in-envboard-host.md) |
| 落点 | `crates/desktop`（新，唯一允许引 `gpui-kit` 的 crate）、根 `Cargo.toml`（依赖纪律 + dev profile） |

## 判断层：要解决什么

envboard 的界面今天是**内嵌 SPA**：Vite + React 19 + HeroUI + Tailwind，编译产物用
`include_dir!` 塞进二进制。它的价值是"一跑就有界面、免装任何东西"，代价是仓库要养
两条工具链（cargo + pnpm），并且界面跑在浏览器里 —— 而这是一个本机常驻工具：用户
一直在同一台机器上开它，代理与证书也都是本机的事。

需求（按权重，权重合计 100）：

| # | 需求 | 权重 |
|---|---|---|
| R1 | 原生窗口，免浏览器与前端构建链，工具链收回一条 | 25 |
| R2 | 本机数据直连：进程内调用管理门面，不经过 HTTP/token | 20 |
| R3 | 等宽字体能用于机器值（环境名、端口、证书摘要、curl 命令） | 15 |
| R4 | 主从布局（列表 + 停靠详情面板），面板可拖宽 | 15 |
| R5 | 组件库现成可用，少造轮子（表格 / 表单 / 下拉 / 弹窗 / 通知 / 主题） | 15 |
| R6 | 能在本机被自动化验证（截图 + 无头 UI 测试） | 10 |

## 决策层：候选与结论

**结论**：桌面形态用 `gpui-kit = "0.8"`（crates.io；内含 gpui-pre 0.3.9 +
gpui-component 0.8.0 + gpui-base 0.8.0 + 默认图标资产），落在新 crate
`crates/desktop`，进程内直连 `envboard-admin`。

| 候选 | R1 | R2 | R3 | R4 | R5 | R6 | 判据 |
|---|---|---|---|---|---|---|---|
| **`gpui-kit` 0.8（采纳）** | ✅ | ✅ | ✅ | ✅（`Sheet.resizable` / `h_resizable`） | ✅（组件目录约 60 件） | ⚠️（截图需 `PW_RENDERFULLCONTENT`；键盘注入本机不可用） | 本地同构先例已在生产跑通：dipswitch 用同一版本交付了 6 个模块 + 53 条坑账 + 可复用件 |
| `windows-reactor` 0.100 | ✅ | ✅ | ❌ | ❌ | ⚠️ | ❌ | **本机实测判死**：`TextBlock`/`TextBox` 无 `font_family`（等宽不可用，见 `skeletons/windows-reactor/README.md` 与本地探针）；`SplitView` 的 Content 按无限宽测量，右列被裁出视口；`ListView.SelectedIndex` 写入即 `E_INVALIDARG` 且被 reactor 当 fatal **直接终止进程**；截图对 WinUI 3 不稳定 |
| 继续只做 web（SPA 常驻） | ❌ | ⚠️（必须过 HTTP 面） | ✅ | ✅ | ✅ | ✅（已有 bsk/playwright 走查） | 不满足 R1/R2：工具链两条、鉴权一套、界面在浏览器标签里 |
| Tauri / WebView 壳 | ⚠️（仍是 Web 界面，只是套壳） | ⚠️ | ✅ | ✅ | ✅ | ⚠️ | 只解决"窗口"，不解决"前端构建链"与"进程内直连"，R1/R2 半成 |
| egui / Slint | ✅ | ✅ | ⚠️（字体族控制弱） | ⚠️（主从/可拖宽要自己拼） | ⚠️（件少，多数要手写） | ⚠️ | 与 R5「少造轮子」相反；本仓无先例，坑要靠新踩 |

三条决定性事实：

1. **R3/R4 是硬门槛**，reactor 在它上面是**缺失**而不是"难看"——它没有字体族属性、
   分栏测量按无限宽走。这一条在 2026-10-26 的选型里已经把 reactor 判出局。
2. **本地有同构先例可照抄**：`projects/dipswitch` 是同一个工作区里的 GPUI 工作台
   （侧栏 + 主从布局 + 表格 + 表单 + 弹窗 + 主题 + 后台任务），版本一致、坑已数清。
   R2 那条"进程内直连"它没有先例（它是纯本机文件应用），但那正是本仓已有的
   `envboard-admin` 门面 + `adr-002` 的组合根在做的事。
3. **协议层早就为这一步留了位置**：视图 DTO 与推送流形状收在 `crates/protocol`
   （纯 serde、零传输依赖），契约文档写明"桌面形态可换任何进程间通道而不动词汇"。
   本决策选的通道是"同一个进程里的函数调用"，是那条话最省的落法。

## 落地层

**依赖纪律怎么破例**：根 `Cargo.toml` 的第 2 条原来是"CLI 之外不引大框架"，
现改为「GUI 形态唯一允许的框架是 `gpui-kit`，且只允许出现在 `crates/desktop`」。
破例不靠自觉：`deps` 门禁第 11 条在 manifest 的产物依赖面上判 —— 别的 crate 引它就红。

**profile**：GPUI 未优化时慢到不可用（dipswitch 实测）。这里只给四个框架包
`opt-level = 3`，**不**用 `[profile.dev.package."*"]` 通配、**不**动 `[profile.dev]`：
本仓日常是跑引擎与管理器的测试，不该为界面框架的可用性付全仓 dev 编译成本。
若界面在 dev 下确实迟钝，再回来加（一次改动，可实测比较）。

**取数**：不写 HTTP 客户端。web 那三条 SSE 流本来就是轮询器（快照每秒全量、
debug 每 500ms 增量、轨迹按字节偏移 tail），桌面用界面自己的定时器驱动
`AdminService::snapshot()`；跨界只有一条通道（tokio → GPUI 主线程）。
游标语义照搬：`state_generation` 是 advisory，界面不得据它跳过渲染。

**不引入的东西**：模块宏（照 dipswitch 的 `dipswitch_modules!` 写法对本仓 3 个目的地
是过度设计，用现成 `Sidebar` 即可）、自嵌 SVG 图标（图标一律取自 `gpui-kit-assets`；
入口注册的是**完整目录** `AllAssets` 而不是组件默认的 106 个 —— 默认集之外的名字
不会报错，只会画成空白 svg，代理屏的「编辑」就这么丢过一次）、
SQLite 偏好库（主题持久化推到设置屏一起做）。

## 代价（P0.2 实测，2026-10-28）

| 项 | 基线 | 桌面化后 | 结论 |
|---|---|---|---|
| `Cargo.lock` 包数 | 157 | **848** | 实测（原按 dipswitch 820 估，略低） |
| gpui 血脉 | — | 每个 `gpui-*` crate 各 1 条 lineage | ✅ 无两套类型系统 |
| 既有依赖版本 | tokio 1.53.1 / ring 0.17.14 / rustls 0.23.45 | **全部未漂移** | 计划里"会被抬到 tokio 1.53.3"的担心**没有发生** |
| `envboard.exe`（web 宿主） | HEAD 同 dist 重构建 7,822,336 B | 本次树 7,902,720 B | +80,384 B（+1.03%）；见下方"体积数字的可信度" |
| `envboard-desktop.exe` | — | 骨架 23,930,880 B → 接上环境列表后 **25,033,216 B** | 比原估的 25–30 MB 落在下沿；随屏数继续长 |
| release 构建（框架包首次进 release） | — | 109 s（单次）；后续增量 47 s（单次） | **单次测量，不作基线**（同配置抖动可达 ±40%）；registry 已缓存、非空 target 的增量口径 |
| 工具链 | cargo + pnpm | 仍两条（P1 之后只剩 cargo） | 达成 R1 的路径上 |
| 远程访问 | HTTP 面支持 | **失去**（P1 起） | 既定代价，不留隐藏开关 |

**体积数字的可信度**：同一份代码两次 release 构建实测差约 40 KB（`lto = "thin"` 的
函数排布不确定），所以上表只能支撑**量级**结论：GUI 框架没有渗进 web 宿主（22.8 MiB 的
框架不可能塞进 7.9 MB 的产物，且 `deps` 门禁第 11 条在 manifest 面上禁止），
组合根抽层带来的增量在 1% 量级。P1 之后 `envboard.exe` 这个产物本身消失，该守卫
转为"唯一产物体积基线"。

**放行线复核**：①`cargo build --release --offline --locked` 在本机可达 ✅；
②gpui 血脉单条 ✅；③web 宿主未因 GUI 变大（量级判定）✅；④数字已写回本表 ✅。

## 本机验证边界（P0.2 实测，写给下一个跑的人）

这台机器上由 agent 会话派生的进程被 OS 拒了三类操作，直接决定了哪些验收能自动做：

| 能力 | 状态 | 影响 |
|---|---|---|
| 渲染截图（`PrintWindow` + `PW_RENDERFULLCONTENT`） | ✅ 可用（110 个不同颜色，非黑帧） | 静态视觉可自动验收 |
| 读 UIA 树 | ✅ 可用：`按钮 深色`、`树项目 环境/规则/代理` 带 `selected` 态 | 结构与选中态可自动断言 |
| **OS 输入注入** | ❌ `SetCursorPos failed: 拒绝访问 (0x80070005)` | **实机点击不可用** |
| 当前用户证书存储 | ❌ 拒绝访问 | 凡"真起一个环境"的测试必红（`live_workbench`、`engine/tests/backend.rs`） |
| 默认用户 `TEMP` | ❌ 拒绝访问 | `cargo test` 需把 `TEMP` 指到可写目录 |

> **2026-10-29 更正**：上表后三项的「拒绝访问」**不是这台机器的固有属性**，而是
> 工作区目录带着 **Low 完整性标签**（`C:\Users\WPS\Workspace\myclaw` 上的
> `Mandatory Label\Low Mandatory Level:(OI)(CI)(NW)`，沙箱会话残留）：从低完整性
> 目录里构建/存放的 exe 启动后进程继承 Low 完整性，凡是写 Medium 位置的操作
> （`%LOCALAPPDATA%`、当前用户证书存储、`SetCursorPos`）一律 `拒绝访问`，
> 而**读是正常的** —— 所以症状看起来像"某个 API 不可用"，其实是进程令牌的问题。
> 判据：`icacls <exe>` 看 `Mandatory Label`，或把同一份 exe 拷到用户主目录下再跑
> 一次（行为翻转即证实）。修法是 `icacls <目录> /setintegritylevel (OI)(CI)M`
> （自动传播到子树）。修好后本机实测：`cargo test --workspace` **全绿**，包括
> 此前必红的 `engine/tests/backend.rs` 与 `data_plane`；桌面形态用默认状态目录
> 正常启动。`SetCursorPos` 属同类，未单独复验。原表保留作为当时的记录；
> 「`TEMP` 指到可写目录」那条措施不再必要（测试里保留它也无害）。

**因此本 ADR 的 R6 只被部分满足**，并且有一条明确的未验证项：标题栏内控件的点击
（坑 #1 的 `stop_propagation` 处理）是从 dipswitch 逐字照搬的成熟写法，但**本会话
无法用真鼠标证明它生效**。

**接替方案本来打算用 `#[gpui_kit::test]` 的框架内事件注入，但它在 gpui-kit 0.8.0
上不可解析**（实测，2026-10-28；2026-10-29 联网复验，错误原文一致）：开 `test-support` 会引入

```text
gpui-kit 0.8.0 → gpui-fast-platform 0.1.10 → gpui-fast-macos 0.1.10 → cocoa =0.26.0
gpui-kit 0.8.0 → gpui-pre-platform 0.3.9  → gpui-pre-macos 0.3.9    → cocoa ^0.26.0 (锁到 0.26.1)
```

两条要求互斥，把 `cocoa` 强钉到 0.26.0 后立刻撞上第二处冲突
（`gpui-pre-web 0.3.9 → unicode-properties ^0.1.3`，锁里是 0.1.4）。继续解下去要
**整体重解析 `Cargo.lock`**，那会把 rustls / tokio 一起抬版本 —— 为了测试便利去动
MITM 数据面的依赖版本不划算。

**这条约束后来找到了绕法（2026-10-29 落地）**：不碰 kit 的 feature 转发，
**直接给底层 crate 开同名 feature**即可，两条 cocoa 约束就都不在依赖图里了：

```toml
[dev-dependencies]
gpui-pre = { version = "0.3.9", features = ["test-support"] }
# 单独加这一条是必须的：gpui-pre 的 test-support 不转发给平台层，
# 而 WindowsWindow::render_to_image 挂在 gpui_windows 自己的 test-support 后面。
gpui-pre-platform = { version = "0.3.9", features = ["test-support"] }
```

（`gpui-base/test-support` 看着更干净，但它会把 `gpui-fast` 拉进依赖图 —— 那是第二套
gpui 类型系统，撞本仓第 11 条依赖门禁，不采用。）

于是无头 UI 集成测试落地为 `crates/desktop/tests/ui_l0.rs`：真的建工作台视图、
真的派发鼠标事件、真的断言切屏。**锁屏下也能跑**（实测 0.04s）。

自动证据现在分四层，交互验收按它们分工：

1. **纯逻辑单测**：过滤/排序/健康着色/`envNeedsAttention` 判定等全部留在可单测的
   纯函数与实体方法里（视图不许自算）；
2. **实体级测试**：`TestAppContext` 不需要 `test-support` 即可用，状态迁移
   （选中、脏态、面板开合）在实体上调真方法断言；
3. **无头 UI 集成测试（L0）**：`crates/desktop/tests/ui_l0.rs`，走真实事件路径
   （`simulate_mouse_down` / `simulate_mouse_up`），断言界面状态迁移；
4. **人工点击 + `PrintWindow` 截图**：凡依赖真实事件派发路径的（标题栏控件、
   拖拽宽度、IME、焦点路由）由人验收，截图留档。

L0 的已知短板：组件库的 `SidebarMenuItem` 没注册 `debug_selector`，坐标得靠探针
硬编码，布局一改就要重探。通用方法与跨项目对比见工作区调研线
`gpui-headless-acceptance-2026-10`（不入本仓）。

**复核触发条件**（补）：gpui-kit 修好 `test-support` 的依赖冲突时，把第 4 层里
能自动化的部分也搬进无头 UI 集成测试。

### 锁屏不是障碍（2026-10-29 实测补充）

上面那张表容易让人以为「验收必须在有交互式桌面的会话里跑」。**实测否定了这一点**：
在**锁屏**（`LockApp.exe` + `LogonUI.exe` 都在跑）状态下，envboard 的桌面验收
**照样能做**，而且能跑到端到端写库。

| 能力 | 锁屏下 | 判据 |
|---|---|---|
| 启动应用、拿到窗口 | ✅ | `Start-Process` 正常，`MainWindowHandle` 有效 |
| `PrintWindow` + `PW_RENDERFULLCONTENT` | ✅ | 176~203 个不同颜色，**是真实画面，不是锁屏壁纸** |
| UIA 读元素树 | ✅ | `TreeWalker` 能列出 `TreeItem` / `Button` / `Edit` |
| `SelectionItemPattern.Select` | ✅ | 真的驱动 GPUI 切屏（截图比对确认） |
| `ValuePattern.SetValue` | ✅ | 真的写入并触发过滤 |
| `InvokePattern.Invoke` | ✅ | 常规按钮可点 |
| `PostMessage` 合成鼠标点击 | ✅ | 可点**不暴露** `InvokePattern` 的弹窗主按钮 |
| 端到端写操作 | ✅ | 导入规则集 → `state.json` 里落库 |
| `CopyFromScreen` | ❌ | **不报错，但抓到的是锁屏壁纸**（实测纯蓝壁纸，600×400 只有一种颜色） |
| `SetCursorPos` / `mouse_event` | ⚠️ | 行为不稳定（实测时灵时不灵），别依赖 |
| 剪贴板 | ⚠️ | 锁屏时 `OpenClipboard` 可能 `ACCESS_DENIED` |

三条容易踩的坑（每条都真实踩过）：

1. **必须 DPI-aware**。本机 DPI=120（1.25×），`GetWindowRect` 返回**虚拟化**坐标而
   UIA `BoundingRectangle` 返回**物理**坐标。混用会让点击偏移 1.25 倍，表现为
   「点了没反应」或「点错控件」——而且界面看起来一切正常，最难自查。
   修法：脚本开头 `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2 = -4)`。
2. **UIA 树用 `TreeWalker` 走，不要用 `FindAll(Descendants)`**。后者在 GPUI 窗口上
   返回 **0** 个子元素，会让人误判「UIA 不可用」而放弃这条路。
3. **别拿屏幕抓取当锁屏判据**。`CopyFromScreen` 在锁屏下有时抛
   "The handle is invalid"、有时成功返回壁纸，同一台机器上前后判据相反。
   正确判据是探**验收真正依赖的能力**：能不能 `PrintWindow` 拿到 ≥10 色画面、
   能不能读出 UIA 子节点。锁没锁与这个判据无关。

可复用脚本落在工作区级目录 `tools/gpui-ui-accept/uia-drive.ps1`（不进本仓）：它把上面
这些姿势固化了，`-Mode dump|click|select|setvalue|shot|lockprobe` 六个模式都已在锁屏下
实测通过；通用方法、机制解释与跨项目交叉验证见工作区同名调研文档
（`gpui-locked-screen-ui-acceptance.md` 与 `gpui-headless-acceptance-2026-10` 调研线）。
本仓读者按上面三条坑自己写脚本即可，不必依赖工作区。

## 复核触发条件

以下任一项发生，本 ADR 需要重开：

* gpui-kit 升到 0.9+（组件 API 有破坏性变动，本仓引用的 `Sheet`/`TableState`/
  `SearchableListDelegate` 形状要重新核对）；
* 出现"必须远程管理环境"的真实需求（那要的是重新引入一个传输面，而不是换 GUI 框架）；
* R6 的自动化缺口变成阻塞（截图 + 无头测试覆盖不住交互，且需要回归到跨平台验证）。

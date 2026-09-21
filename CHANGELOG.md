# Changelog

All notable changes to `slothtron-envboard` are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versioning follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed（v4 架构升级：前端工程化 + Admin API 门面 + manager 按域拆分）

- **前端工程化（破坏性：资产形态）**：新增 `frontend/`（Vite 7 + React 19 +
  HeroUI v3 + Tailwind v4，pnpm 管理），工作台 UI 转正自验证 demo
  （`examples/envboard-workbench-heroui`）并接真实 Admin API；补建「活动」视图
  （`/api/history` 审计事件时间线）与主题切换（浅色/深色/跟随系统，localStorage 持久化）。
  旧原生三件套 `crates/web/assets/` 退场；`web` 改用 `include_dir!` 内嵌
  `frontend/dist`（hashed 资产走 `/assets/*` + immutable 缓存，token 豁免面从
  两个固定路径改为 `/assets/` 前缀）。URL 与响应形状不变，前端零回归。
- **构建顺序纪律：先 `pnpm build` 后 `cargo build`**。`frontend/dist/` 是构建
  产物、不入库（与 `target/` 同类）；`crates/web/build.rs` 缺 dist 时编译期响亮
  失败；`ci/verify.sh` 的 frontend 层（typecheck + build）成为默认 all 的最前置，
  无 pnpm 但 dist 在位时以既有产物通过。
- **工具链纪律重划**：Node/TS/Vite/pnpm 从「全仓禁止」改为「圈禁在 `frontend/` 边界内」；
  可执行面仍禁前端命令（cargo 永不驱动前端工具链），衔接发生在 verify 编排层。
- **新增 `envboard-admin`（Admin 管理 API 门面）**：控制面用例的传输无关门面 ——
  类型化入口校验（`EnvCreateReq` / `EnvPatchReq` / `ProxyPutReq` / `RulesImportReq` /
  `DebugStartReq`，`deny_unknown_fields`）+ 统一错误信封；`web` 依赖从 `manager`
  改为 `admin`（web 降级为纯传输绑定），契约成文为 `spec/admin-api.md`。
  `envboard-protocol` 同步补请求 DTO（patch 的「未提及/显式 null/给值」三态保真）。
- **`manager.rs` 按域拆分**（行为不变，48 个公开方法签名逐字未动）：单一 `Manager`
  类型的 impl 拆入 lifecycle / rules_store / proxies / reconcile_health / ledger /
  observability / debug_capture / projection 八个模块文件；跨域共享私有项留
  manager.rs 改 `pub(crate)`。
- **UI 契约升 v2.0**：机检档从「三件套资产扫描」改为「frontend 源码扫描」
  （UI-1 色值零容忍 / UI-2 禁内联样式 / UI-3 禁手写 dark: / UI-4 禁裸 px 任意值 /
  UI-5 交互形态 / UI-6 锚点对账）；新增 `spec/design.md` v1.0 设计规范
  （12 组设计语言 + bsk 走查检查表，转正自 demo 的 REVIEW_NOTES P1–P14）。
- 文档修正：README 版本号 0.2.0→0.3.0、crate 树补 events/admin/frontend、
  架构图与工具链纪律改写。

### Changed（UI crate 拆分：protocol + web，前后端协议成文）

- **新增 `envboard-protocol`（叶子 crate）**：UI 边界词汇的唯一定义处 —— 视图 DTO
  （`EnvView` / `ProxyView` / `DebugView` / `ReconcileReport`，自 manager 收拢）与
  抓包词汇（`CaptureView` / `CaptureDelta` / `SessionInfo`，自 engine 收拢）迁移至此，
  两侧以原名 re-export 保兼容；新增推送流帧类型（`TrajectoryWindow` /
  `SnapshotFrame` / `DebugSnapshot` / `DebugEvents` / `ErrorFrame`）与
  `Cursor`（Byte / Request / Generation）。纯 serde、零传输依赖 —— 未来桌面形态
  （`envboard-desktop`）与 web 平级共用。
- **新增 `envboard-web`（web 形态 crate）**：`crates/server` 的资产（index.html /
  app.css / app.js，git mv 保历史）、axum 路由/handler/安全中间件、`WebConfig` 与
  CSP、qrcode 端点整体平移；只做传输绑定，整 crate 不认识引擎装配（源文件面门禁
  随拆分扩展到本 crate）。
- **`envboard-server` 瘦身为宿主**：只剩 `[[bin]] envboard` 的组合根（引擎装配 /
  SharedCa / flock）与 serve 编排（reconcile、日志照看循环从 web 层归还宿主）；
  lib 面随拆分取消。
- **推送流帧统一**（行为变更，契约见新增的 spec/protocol.md）：三条 SSE 流的每帧
  都带显式 `cursor`，帧名统一 `snapshot / baseline / events / error`。轨迹流
  `baseline` 帧补齐游标（= 窗口末端字节偏移），REST `GET …/trajectory` 响应体改
  为同形 `{cursor, events}`（修复前端此前拿事件 seq 硬凑字节偏移的错位）；快照流
  帧新增控制面账本代次作 advisory 游标；调试实时流无会话首帧补 `cursor: 0`。
  前端 baseline 直读游标续传、快照帧记录代次。
- **门禁**：`policy-tests` 依赖边表登记 protocol / web 并更新 server 边；
  `ui_style` 门禁资产路径改指 `crates/web/assets/`；artifact 校验同路径。

### Added（上游代理 / 二级代理：一等实体 + 按环境热绑定）

- **代理账本**：`state.json` 增 `proxies[]`（`name/host/port/user/password`），manager 为
  唯一真相。凭据与环境 `proxy_user`/`proxy_password` 同规（同生共死、不含 `:`/空白、
  ≤64/≤128 字符），明文只落 0600 状态文件，API / 视图 / 日志**永不回显**，只暴露
  `has_auth` 布尔；同名保存即整体替换并热应用引用环境。
- **环境第 8 字段 `upstream`**：按名引用代理账本（`null` = 直连），与 `rules` 绑定同构、
  **热字段**（运行中 PATCH 即生效，上游连接每请求新建无残留）；绑定不存在的名字
  `invalid_config`，删除被引用的代理 `conflict` 并点名全部引用方，状态文件手改出的
  悬空引用加载即拒启 —— 三条同证「禁止静默降级」。
- **引擎链式建连**：兑现 `ConnectTarget.chained_proxy` 扩展位（基础层从配置播种、
  插件链保留修订权；`ProxySpec` 增出向 Basic 鉴权位）。TLS 目标：TCP 连代理 →
  `CONNECT`（带 `Proxy-Authorization`）→ 非 2xx 返 502 带代理状态码 → 隧道内按既有
  `TlsPolicy` 做 rustls 握手（**内层 TLS 与直连一致，`insecure_hosts` 语义不变**；
  CONNECT 目标 = 规则改写后的终址）。明文 http 经代理按 absolute-URI 转发（Host 头
  透传）。`config_hash` 覆盖代理 host/port/user 与密码摘要。`auth.rs` 补
  `base64_encode`（RFC 4648 向量互验），不引新依赖。
- **API 面**：`GET/POST /api/proxies`、`GET/DELETE /api/proxies/:name`（清单带
  `references[]` 供删除确认）。
- **工作台第五视图「上游代理」**：代理清单（地址 / 鉴权位 / 引用环境）+ 保存表单 +
  破坏性确认删除；环境配置表单与创建弹窗增「上游代理」下拉（热字段不进停机锁）；
  概览增「上游代理」行（直连显式写出）。
- **契约同步**：环境模型 7→8 字段（capabilities.md「上游代理（chained proxy）」新节 +
  热/停机矩阵补行）；errors.md 补 invalid_config / conflict 触发场景；契约 fixture
  66 → 79（新增 `proxy.validate` 能力与环境/合并 upstream 用例）；README 能力表、
  PATCH 示例、热矩阵与 HTTP 端点表同步。
- **明确不做**：SOCKS5、`https://` 代理（与代理本身的 TLS）、PAC / 透明代理、
  按域名分流、代理链。

### Fixed（明文 http 经带鉴权二级代理不吃 407）

- 二级代理的出向 Basic 凭据此前只在 CONNECT（TLS 目标）那半注入：明文 http 走
  absolute-URI 转发时凭据缺失，要求认证的上游代理对每个明文请求回 407 且被逐跳头
  过滤掉 `Proxy-Authenticate`，客户端只见一个没有挑战头的 407（实测
  yunshu-beta → 127.0.0.1:11088 即此病；https 不受影响，故 live 用例漏网）。
  现在转发请求头同样携带 `Proxy-Authorization: Basic`，与 CONNECT 两半对齐；
  客户端自己的 `proxy-authorization` 维持不透传。契约 `spec/capabilities.md`
  「建连语义」明文条款同步钉死；data_plane 新增带认证明文链路用例（负向验证：
  撤修复即红），live 16c 的二级代理改为要求鉴权、真正覆盖出向凭据。

### Added（UI 设计规范契约化：入库契约 + policy 门禁）

- **新契约 `spec/ui.md`**：把工作台三资产（index.html / app.css / app.js）的
  UI 纪律固化为入库契约，分机检档（UI-1…UI-8，违反即 `ci/verify.sh` 红）与评审档
  （等宽字体 / 异常三通道 / 语义色专用 / 破坏性操作模态二次确认 / `--muted-2`
  表面限制 / WCAG AA 底线等门禁不可达项）。令牌值唯一权威仍是 app.css 第 ① 区。
- **新 policy 门禁 `tests/policy-tests/tests/ui_style.rs`**：机检色值出没域
  （只许在 `:root` / `[data-theme]` 令牌区）、字号 / 圆角 / 阴影 / 间距的令牌化
  （间距例外登记 1px / 2px 发丝档）、纯黑禁令、令牌存在性、禁原生弹窗与内联
  script / 内联事件属性，以及契约锚点 `UI-n` 与门禁规则的双向对账（缺锚点或
  多规则号都判红，契约与门禁必须同提交同步）。三个资产现状已全量合规，门禁
  零豁免上线；README「运维」节与契约互指。

## [0.3.0] - 2026-09-18

### Added（调试实时流：抓包记录自动上屏）

- **`GET /api/debug/stream`（SSE）**：调试页不再靠进页时一次性拉取 ——
  连接即发 `snapshot` 帧（整幅会话视图），此后 500ms 一轮询内存缓冲、有增量发
  `events` 帧 `{records, cursor, captured, dropped}`；会话换代或目标消失重发
  `snapshot`。游标（request_id）由流自己维护：淘汰只移除游标之前的记录，增量
  无缺口，断线重连自愈。前端在「调试」视图维持一条连接（离开即断），记录追加
  就地渲染、原本贴底才自动跟随；「开启 / 停止 / 清空」按钮即时反馈。
  契约见 `spec/events.md`「调试实时流」。

### Fixed（续）

- **`GET .../captures/:request_id` 全缓冲查找**：曾经的实现是「取尾部 1 条再
  比对」—— 只有最新一条查得到，其余一律 404。改走引擎缓冲的按 id 查找。

### Fixed（调试页恒空的两个根因：capture 不热应用 + 轨迹窗口被账本读者整份拒绝）

- **`capture` 真正热应用**：`capture` 是契约声明的热字段（调试页「开启 / 切换」
  驱动的就是它），但更新路径的热更触发集合漏了它 —— 开关只落盘，引擎手里的
  快照永远 `capture=false`，抓包计数恒 0，页面忠实地渲染这个 0。现在
  `capture` 变更与 `insecure_hosts` 同路：一次同步热应用，并计入审计事件的
  变更字段清单。
- **轨迹按窗口契约读**：轨迹文件从写下第一行起就是无头行、`seq` 每个实例会话
  从 1 重启的**窗口化日志**，而两个读者按控制面账本契约（`parse_log`：要求
  版本头 + seq 连续）读它 —— `GET .../trajectory` 恒 `BadHeader`，SSE 基线被
  吞错置空、流在第一次增量处死掉（`SeqGap`）。新增 `parse_window`（无头、
  seq 不校验、撕裂行逐行跳过、未知类型仍 fail-closed），两个读者改用它；
  两档契约成文于 `spec/events.md`「存储格式（两档）」。

### Changed（v4：独立工具形态 + DSH 式事件轨迹）

- **移除插件抽象**：`Plugin` trait、能力注册表（`CAPABILITIES`）、装配期拓扑
  校验、钩子执行器与 `debug-inject` 整体退场。hosts 改写下沉为引擎 connect
  路径的内部直调（`hosts::apply`，语义不变：脏规则 fail-closed 502），请求
  终局日志直投有界总线（行格式不变）。`EngineReport.bypass_counts` 删除。
- **目录重排为独立工具形态**：`core/spec/` → `spec/`；`core/rs/crates/*` →
  `crates/{engine,engine-fake,manager,server}`；契约/门禁测试 →
  `tests/{contract-tests,policy-tests}`。core-api + core + domain + rules 四
  crate 合并为 `crates/engine`（api/rules/domain 变模块边界，deps 门禁改为
  模块面判据）；`envboard-web` → `envboard-server`（bin 仍为 `envboard`）。
- **控制面审计事件**：`<state_dir>/events.jsonl`（append-only，权威仍是
  state.json）；`Manager::commit` 单一发射点（先 save 后 emit，门禁钉死）；
  `GET /api/history` + 工作台「活动」视图；事件写失败丢弃 + 计数
  （`/api/status` 的 `events_dropped`）不阻断控制面。契约见 `spec/events.md`。
- **抓包（capture）**：`capture` 环境字段（默认关、热开关）；请求/响应详情存
  实例内存会话（停止/重启即丢弃，清空走 `POST .../capture/clear`），满即淘最旧
  （`--capture-budget`，默认 256 MiB），body 超 256 KiB 标 omitted（缺失而非截断）；
  `GET .../captures` 详情/清空/导出（HAR 1.2 / JSONL）+ 工作台轨迹页点开详情与导出。
  设计参考 mitmproxy（成对落盘、raw/decoded 分离、缺失而非截断、View.clear）。
- **调试页 + 会话双轨**：抓包独立为「调试」视图；调试会话是**工作区级单例**，
  切换环境 = 原环境抓包停止并清空 + 新环境开启（`POST /api/debug`）；支持导入
  HAR 会话（`POST /api/har/import`，多个并存只读，最多 8 个 / 256 MiB，超限拒收）。
- **数据面请求轨迹**：`trajectories/<env>.jsonl`（request/start→upstream→
  body→response/head→end，`request_id` 贯穿；正文与头部不入轨迹）；独立有界
  总线 + `EngineReport.trajectory_drops`；`GET /api/environments/:name/trajectory`
  + SSE 实时流（baseline + 增量 + cursor 续传）+ 工作台「轨迹」页签。

## [0.2.0] - 2026-09-17

### Fixed + Changed（第三轮走查后：配置页签回显、动作行收敛、设置页结构件补齐 + 按设计规范全量 UI 走查）

- **配置页签 = 常驻编辑器**：选中环境即回显全部当前值（幂等重填，SSE 快照不冲掉
  输入中的改动）；「保存」按钮监听表单改动，与基线一致时禁用；运行中照旧锁停机字段
  并亮「运行中」提示徽章。
- **动作行收敛为三键**（停止/启动 · 重启 · 删除）：右上角「日志」快捷按钮删除
  （日志已是页签）、代理命令复制按钮删除、「更多操作」平铺行退场；
  重分配端口移入 port_conflict 的原因条。
- **设置页结构件补齐**：`.ca-card / .ca-head / .ca-title / .spanel / .notice.ok`
  此前 HTML 用了类名而 CSS 从未定义，证书面板塌成无结构裸块（用户截图所示排版错误）；
  页签样式与详情页 `.tab` 共用；标题栏只留「设置」。
- **按「envboard 设计规范 · v2.0 Light」全量走查落地**：
  破坏性操作（删除环境 / 删除规则 / 重分配端口）迁入独立确认模态，写明后果与
  不可逆性、确认按钮显式第二次点击（规范 7.2.3）；模态对齐规范 6.6（520 宽、
  15px/600 标题、32×32 关闭、字段控件 36px + --line-2 描边、必填星号 --bad、
  遮罩冷灰 .45、窄屏竖排按钮确认在上）；模态控制器补焦点陷阱 / ESC 策略
  （submitting 忽略 ESC、取消与关闭保持可点）/ 遮罩点击策略 / 焦点归还；
  创建弹窗补失焦校验 + 提交全量校验 + submitting 态（「创建中…」+ 字段只读），
  校验文案按规范 6.6.5 落地（原因 + 建议动作，409 重名映射回名称栏）；
  toast 移到顶栏下方右侧、最小宽 240、2.5s 淡出（错误 6s，偏差已记录规范 7.3）；
  图标一律 currentColor（删除五处独立配色）；区块标题加 3px 主色竖条；
  日志搜索命中高亮转 25% 底；禁用按钮 pointer-events 关闭；
  设置版本 chip 与「运行中」徽章的语义色归位（装饰不再占用 --accent）。
- **退役**：就地二次确认（.confirm 展开块）、state.confirm / state.editing、
  copyText / flashCopied、.detail-more 样式、--logbar-h 令牌。
- **规范文档同步**：7.3 四项待对齐全部裁决关闭（事件类型过滤、输入 32/36 分层、
  错误 toast 时长、CmdCard 无复制按钮）；附录 A 与 app.css :root 逐字同步。

### Changed（第二轮走查后：UI v3 版式 + 设置·证书 + 全库测试数据脱敏）

- **工作台版式按「envboard 工作台 UI 设计稿 · Light」重排**：视图导航从顶栏移进
  侧栏（环境 / 规则库 / 跨环境对比 / 设置四项）；日志从常驻底栏收进环境详情的
  第 4 个页签（过滤 chip 与搜索/导出/跟随工具条随迁）；新建环境改为居中弹窗
  （只收名称 / 描述 / 端口 / 规则集，对外绑定与代理鉴权留在编辑表单）；规则库
  行内元信息带落盘文件名（`<名字>.hosts`），侧栏在规则库视图切换为「规则集」
  列表并常显「N 环境引用」徽标；跨环境对比行按设计稿改为「环境 → IP + 命中
  来源 chip / 无规则命中」。
- **新增设置视图**：证书页（根证书状态、下载、扫码二维码、证书信息、常见问题）、
  通用页（运行信息只读回显）、关于页（版本信息）。配套端点 `GET /api/ca`、
  `GET /api/ca.pem`、`GET /api/ca/qrcode.svg`（照常受 Host/token/CSRF 门禁管束，
  401 roll-call 断言已扩到 21 端点）。`/api/status` 增加 `version`（工作台自身
  版本）。证书结构读取按仓内纪律走 `envboard-core` 自有 der 模块（不引 x509 库）。
- **依赖**：新增 `qrcode`（纯 Rust，default-features 关闭、只留 svg 渲染）——
  Cargo.lock 增量仅此一个包，离线可构建不受影响。
- **本版未做**：上传根证书 / 重置根证书。两者需要管理器支持 CA 轮换（引擎在跑
  实例的热切换与全量重启编排），超出本次 UI 升级范围，后续单独立项。
- **测试数据脱敏**：契约 fixtures、单测与界面示例文案里的个人测试域名与内网 IP
  统一替换为 RFC 2606 保留域（`example.com` 家族）与通用私网假值；仓库文本零残留。


v2 把 envboard 从「一个 mitmproxy 进程内的运行时开关」改成**多环境管理器**
（一个环境 = 一个独立代理实例 + 一个独立端口，附 Rust 工作台），落地分支
`feat/rust-multi-env-manager`（从 `v0.1.0` 切出）。以下是**已经落地的契约层改动**——
它们先于实现改动，因为契约是两份实现的裁定依据。

### Changed（走查后第二轮：鉴权换默认 + CLI 退役，两条 BREAKING）

- **鉴权按绑定地址定档**：回环监听（127.0.0.1 / ::1）**默认免鉴权** —— 本机即本机
  用户，token 挡不住同机进程、只添摩擦；`--token <T>` 在任何监听上显式启用，裸给
  `--token`（不带值）自动生成 128 bit 随机值并在启动日志打印可点链接；**非回环监听
  不给 token 直接拒绝启动**（`invalid_config`，字段 `web.token`）。`--without-token`
  退役（它的语义成了默认档，选项失去存在理由）。`Host` 校验与 CSRF 头与档位无关、
  永久在场；多用户共享主机建议回环也上 `--token`（README「工作台鉴权与对外暴露」
  有取舍说明）。token 不再落盘：`runtime/api.json` 随 CLI 一起退场。
- **CLI 子命令面整体退役**：`envboard-cli` crate（`status` / `run` / `env` / `rules` /
  `compare` 子命令、直连模式、瘦客户端 `api_client`）删除，`--core`（含 `fake`）、
  `--once`、`--json` 一并退役 —— `envboard` 二进制唯一行为 = 启动工作台；`[[bin]]`
  并入 `envboard-web`（组合根与产品同 crate，一个 crate = 一个发布物）。管理动作
  统一走工作台 UI 或本地 HTTP API（端点不变，README 有 curl 示例）：判定只在服务端
  做一次，界面与脚本是两个翻译面，这正是砍掉第三张面孔后仍成立的理由。
- **测试面随迁**：smoke 重写，钉住"命令面不得再长出子命令"、鉴权四档、非回环
  拒启（负向守卫）、状态单写者；live 工作台铺设从 `cli()` 改 HTTP（`seed_rule` /
  `seed_env`，与脚本用户同一条路）；第 11 组按新档位重写（回环默认免鉴权 200、
  裸 `--token` 自动生成、显式 token 双通道 200 与端面逐个 401 清点、非回环拒启）；
  `thin_client` 层随功能退场。

- **deps 门禁随形态调整（有理由的放宽，负向验证过）**：crate 登记表移除
  `envboard-cli`；`envboard-web → envboard-core` 边放行，但**只给 `src/main.rs` 组合根用**
  —— 工作台 lib 面（api / config / lib.rs）新增源文件面判据：出现 `envboard_core::`
  引用即红（注入探针验证过）。"换引擎实现不动工作台"这条不变量原样在场，只是裁决
  粒度从 manifest 细化到了文件。

### Fixed（实机浏览器走查揪出的三个缺口 + 一个测试盲区）

- **空列表里「新建环境」是死的**：renderDetail 在没有选中环境时把含表单的详情区
  整体隐藏 —— 第一个环境永远建不出来（v2 遗留，live 层从不走 UI 所以从未暴露）。
  前端引入显式 creating 态：新建时详情区盖过空态、动作行收起、取消/创建后回位；
  renderDetailActions 补回 is-hidden 复位（曾经隐藏过的动作行必须能回来）。
- **硬刷新必 401**：前端捕获 token 后立刻抹掉地址栏 query，而 document 请求本身
  需要 token —— F5 得到 401 JSON 页。改为捕获后保留在 URL（可用性优先于观感，
  凭据本就在启动横幅里，换 token 随时可重启）。
- **运行标记行随子进程模型丢失**：契约承诺"每次启动写一行运行标记"，v3 实现漏了
  ——launch 现在经 log writer 写运行标记行（追加不截断；manager_lifecycle 新增
  断言：启动即有文件、重启成两段）。
- **query token 端到端是测试盲区**：live 的 api() helper 一直用 header 通道，
  "URL 携带 token"这条浏览器唯一路径从未被断言覆盖。补 api_query helper 并把
  11a 扩成 header 与 query 双通道断言（实机验证 query 200 ✓）。
- **新增内嵌资产完整性门禁**（本轮修复过程自己的事故换来的）：app.js 曾被
  "读截断 + 写回"弄丢尾部并随 release 部署出去——JS 没有任何编译期检查，默认层
  全绿而页面卡死。artifact 门禁现在钉住 app.js 最小行数与关键符号（envboardReady/
  renderDetail/refreshAll/addEventListener）、index.html 关键 id；已负向验证
  （截断必红、恢复转绿）。
### Removed（v3 重构 M-P6：收尾退场，v2 面清零）

- **全部 Python 产品代码退场**：`adapters/mitmproxy/`（注入器）、
  `envboard-core-mitmproxy` crate（子进程监督 + 内嵌 + 状态文件读侧）、
  FakeCore/StatusMode（v2 替身）、dual_impl 对拍与 `--render` 跨语言入口文档
  ——一次删净。工具链收敛门禁的适配器例外整套翻正：`.py`/`__pycache__`/适配器
  脚本调用现在是**无条件违规**（PENDING 白名单机制保留，表已空 = 收敛完成）；
  artifact 门禁把"注入器自包含"检查换成 v3 反向断言（adapters/ 不得存在 +
  引擎线程名标记在场），并做了负向验证（塞入 .py 必红）。
- ProxyCore-v2 抽象面删除（InstanceSpec/InstanceHandle/InstanceHealth/StatusReport/
  ProcessIdentity/redact_cmdline/config.json 与软链常量）；core-api 的 proxy 模块
  只剩 v3 共享词汇：Listen、CoreInfo、能力表（六项：listen/dynamic_certs/
  rewrite_upstream/per_domain_insecure/shared_ca/http1_only，全部在线声明）。
- 状态面剪枝：PersistedState 不再携带 records/config_seals；ManagerConfig 删
  status_ttl_secs/reload_interval_secs/annotate 与 confdir config.yaml 防线；
  cli 删 `--reload-interval`；reconcile 规划保留**占用探测**（MarkConflict 判据），
  启动路径以绑定为唯一真相。**新增升级契约测试**：v2 的 state.json（含旧键）
  直接加载成立，再保存时旧键整体消失、环境数据原样搬过来。
- 契约文档收口：capabilities.md「实例日志」按 v3 总线形态重写（有界 1024 行 +
  丢弃计数 + copytruncate + 有界尾读，v2 的 64 KiB 管道实测教训作为约束来历保留）。
- README 全量重写为 v3（架构/能力矩阵/已知限制/从 v2 升级/验证分层）；
  systemd 单元同步 v3（去 `--core-bin`/mitmdump/PDEATHSIG 叙述；单进程语义：
  停服务即停一切、无孤儿代理）。
- 版本 0.1.0 → **0.2.0**（v2 多环境形态与 v3 纯 Rust 引擎随本版一并发布）。

### Changed（v3 重构 M-P5 第三步：live 层迁移——一条不减）

- **live/workbench 28 项全绿**：v2 的 24 条断言全部保留（机制触点按映射换），
  新增 4 条：
  - 12b **凭据 argv 审计**：全系统 /proc cmdline 扫描不得出现明文密码（v2 靠
    脱敏契约兜底，v3 结构性成立）；
  - 14b **热生效时延**：PATCH 返回后的第一次请求即已生效（v2 要等 1s 轮询）；
  - 15 **既有 CA 零感知兼容**：confdir 预放 mitmproxy 形状 CA → 引擎原样加载
    （逐字节不变）且真 curl 仅凭该 CA 校验 MITM 链成功；
  - 端点清单扩到 18 个（含 /api/_fault）且全部无 token 401 的清点面不变。
- 组 9（实例崩溃）机制替换：SIGKILL 真子进程 → **注入 failed**（ProxyEngine 的
  故障注入旋钮，真引擎与 fake 同构实现，经 web 的 /api/_fault 转发）；"僵尸
  回收"判据 → "监听端口真的释放"（原端口可再 bind）。组 8 的"写满 64 KiB 管道"
  字节判据 → "全部成功 + 日志逐条落盘"（总线介质不同，丢弃量归 log_drops 面）。
- **live/manager 3 组重写为引擎直驱**（真 openssl 现场 + 真 curl）：①预放 CA
  加载复用 + 回执如实 + curl --cacert 验链；②注入 failed 可见、端口释放、重拉
  回 running；③insecure 名单外 502 → 热 apply 第一次请求即 200 且未点名域名
  仍 502（放宽不传染）。v2 的 options_echo 断言随"静默忽略选项"介质的消失删除。
- live 宿主从 mitmdump/openssl/curl 三件减到 openssl/curl 两件；verify.sh 的
  live/manager 指向 envboard-core。
- 产品侧顺手修一处可用性缺陷：自产 502 的响应体现在回声状态行
  （"502 Bad Gateway: <原因>"），与 v2 失败页同款可辨识。

### Changed（v3 重构 M-P5 第二步：manager 行为测试按 v3 语义重实现）

- manager_lifecycle.rs 以 v3 形态重写（28 条全绿），暂存件删除；v2 的 29 条
  逐条对账，**机制退场者目的由等价断言接替**：
  - stale/missing 状态文件 → unhealthy/failed **来自引擎内存报告**
    （inject_state 旋钮；"TCP 可连≠在跑"反转为"报告是唯一真相，期望保持
    running 等 reconcile 拉回"）；
  - 配置回显比对 / 收敛窗口 / config_error → apply 被拒三连：旧快照继续服务、
    invalid_config 标记进健康原因、修复后标记自动清除；同步生效留痕
    "hot-applied configuration"；
  - 软链断 = 不覆盖 → 账本缺条目 = rules_missing（legacy 状态场景），
    补导入即在跑实例热应用；
  - 孤儿清理 / PID 复用绝不杀 → 结构上不存在可误杀的进程：reconcile 在
    现实与期望一致时保持安静（keep / 无动作）；上一代重启 → **failed 引擎
    被 reconcile 拉回 running**；
  - reconcile 的 MarkConflict：占用探测**只保留在规划路径**（没有它，
    自动端口环境会在 reconcile 里被静默换端口——契约禁止）；启动路径仍以
    绑定为唯一真相。
- FakeEngine 注入语义补强：非 running/starting 的报告**同时释放监听**
  （与真引擎"线程死→运行时散→socket 关"同构），否则 failed-自愈路径测不出来。
- 测试脚手架换共享 RecordingLogger 的 standalone()：单一写者纪律下，
  "改状态文件再观察"的 legacy 场景必须经新管理器读取（这正是纪律的演习）。

### Changed（v3 重构 M-P5 第一步：契约改写与对拍退役）

- 实例生命周期契约改写为 v3（capabilities.md 的「实例生命周期」与「配置下发与热
  应用」两节、errors.md 健康状态表）：判定 = 标记 → desired → 引擎内存报告；
  **config_mismatch 从契约删除**（装配即生效没有中间态；被拒配置以 invalid_config
  标记 + unhealthy 表达）；僵尸/PID 复用/状态文件 TTL/收敛窗口等进程时代判据
  全部退场；"任务级崩溃隔离 + release 保持 unwind"与"视图与权威判定同源"升格
  为契约硬要求。
- domain 的 plan_reconcile 换 v3 模型：InstanceRecord.live 从"进程身份"改为
  **bool（线程存活）**；Action 去掉 Restart（上一代概念消失）；keep 成为清理遍的
  可见决策；warnings 恒空（字段保留给未来的退避/告警面）。契约 fixture 7 张 → 5 张
  （lifecycle 组按新语义重写：starts/stops/order/port-conflict/no-port-start），
  总数 68 → 66（计数断言与 README 同步）。
- 规则语法 BNF 仍是唯一仲裁（fixtures/rules 组不动）；**dual_impl 逐字节对拍退役**
  ——v3 只剩一份解析实现（envboard-rules），第二份 Python 镜像与它需要的宿主解释器
  探测一起删除；ci/verify.sh 的 adapter/dual 层与 PYTHON 旋钮随之移除（默认层 =
  policy + rust + contract + artifact）。
- 「引擎能力矩阵」改写为 v3 表（内核能力 + hosts-rules/request-log 的落点逐项对应
  代码；已知限制——仅 h1、信任库换源——随本表登记，README 收口在 M-P6）。

### Changed（v3 重构 M-P4 第二步：manager/web/cli 接线到 ProxyEngine）

- Manager 换依赖面：构造参数从 ProxyCore 九件套变成 ProxyEngine 八件套
  （**ProcessTable 参数删除**——没有外部进程就没有孤儿清理）。启动 =
  engine.start + 等待绑定定态（running / port_conflict / failed）；端口冲突
  的"新建自动重试一次、已持久化只标记"契约原样保留。健康判定统一走
  verdict（标记 → 期望 → 引擎内存报告），**视图与权威判定同一实现**；
  EnvView.health 升级为 v3 InstanceState（新增 starting 态；config_mismatch
  随同步装配消亡）。
- 进程监督层在管理面的用法全部消失：config.json 通道、固定名规则软链、
  状态文件物化与 TTL/收敛窗、runtime/agent 目录、/proc 身份、信号梯子。
  规则绑定改为**账本 rendered 直接进 EngineSpec**（账本唯一真相不变，少了
  整条文件物化-轮询链）。热更新改为对运行中实例的同步 apply：insecure_hosts、
  rules 绑定、**规则内容**（import_rules 覆盖即热应用）、description；apply
  失败留 invalid_config 标记（health_reason 可见"旧快照继续服务"），成功清标记。
  ProxyEngine::apply 改同步签名（装配即生效没有异步语义；管理面同步入口
  不再 runtime 套 runtime）。
- CLI：--core 取值 engine|fake（默认 engine），--core-bin/--core-python 随
  mitmproxy 退场删除；共享 CA 在 confdir 就绪（兼容既有 mitmproxy CA），
  重新物化时向 stderr 响亮警告"客户端需重装证书"。
- 产物门禁翻档：删除"二进制内嵌注入器"断言（include_str 链已断），改为
  断言 v3 进程内引擎簿记标记在场；注入器文件自检保留至 adapters 退场（M-P6）。
- **v2 测试暂存（断言不减，搬家不丢弃）**：manager_lifecycle.rs（29 条）与
  core-mitmproxy 的 live_manager.rs（3 条）改名为 *.pending-migration 暂存
  ——它们断言的机制（状态文件新鲜度/config 哈希回显/收敛窗/软链完整性/
  mitmdump 子进程）在 v3 已不存在。M-P5 逐条按 v3 语义重实现（引擎直驱、
  注入状态替代崩溃、epoch/hash 回执替代文件比对），清单即这两份文件本身。

### Added（v3 重构 M-P4 第一步：ProxyEngine 接缝与双实现）

- core-api 新增 v3 引擎接缝（engine 模块）：EngineSpec（监听/名单/凭据/规则文本/
  日志线出口——全部一等字段，无透传通道）、EngineHandle（**无进程身份**）、
  EngineReport（state/config_hash/epoch/last_error/bypass_counts/log_drops，
  内存读同步报告）、InstanceState（starting/running/stopped/unhealthy/
  port_conflict/failed；**config_mismatch 消亡**——同步换快照 + hash 回执让
  "生效不一致"没有存在的形态）。LineWriter 端口进 core-api::ports。
- config_hash 单一真相：EngineSpec::hashable_json 的 sha256（log 与展示字段
  不参与）。引擎内部编译哈希只做幂等，绝不作对外回执（两处哈希曾差点成为
  第二真相，测试把它钉回了同源）。
- EngineBackend（envboard-core）：ProxyEngine 真实现——簿记 map + 有界日志
  总线（logsink：try_send 即返回、满则丢弃计数；v2"213 个请求写满 64 KiB
  管道全体挂死"的教训以新形态继续成立）。start 成功 = 已登记；端口占用是
  **报告里的状态**而非启动错误（管理面"新建冲突自动重试一次"契约依赖此）。
- FakeEngine（envboard-core-fake）：同一接口的替身，真绑定端口，故障注入
  旋钮 inject_state/inject_log_drops（live"实例崩溃"类断言的迁移目标）。
  迁移期与 v2 FakeCore 并存；Manager 接线完成后 FakeCore 与 ProxyCore 一起退场。
- 验收：backend 实机 2 组（trait 全生命周期 + 真实转发 + 总线日志 + 幂等
  stop/NotFound/Conflict），fake 单测 4 条，总线非阻塞单测 1 条。

### Added（v3 重构 M-P3：插件宿主——阶段管道与能力注册表）

- 三层能力模型落地：内核能力（listen/proxy-auth/tls-policy/mitm-ca/protocol）
  保持原生快路径；内置插件 hosts-rules 与 request-log 走与扩展插件完全相同的
  接口（产品功能自举验证）。阶段模型写死：connect → request_head →
  request_body →（上游）→ response_head → response_body → log。
- 能力注册表（envboard-core/src/plugin.rs 的 CAPABILITIES，静态只读）：
  只参与装配期校验与自省，不参与请求路径查找。契约表在
  spec/capabilities.md 的「v3 插件与能力注册表」；三方一致性
  （契约表 ↔ 静态注册表 ↔ 内置实现）由新增 policy 门禁判定
  （envboard-policy-tests/tests/registry.rs，负向样本双向验证过会红）。
- 装配期校验：id 未注册 / 内核冒充插件 / 缺依赖 / 依赖环 → invalid_config
  点名双方；执行序 = 声明序，依赖约束 > 声明序（同层同依赖保持声明序）。
  明确不采纳"静默等待依赖"语义。
- 错误契约两档：connect/改写钩子 Err → fail-closed 502（正文带插件名与
  原因）；显式 bypass（request-log 即此档）→ 跳过 + 强制 WARN + 逐插件
  计数（EngineInstance::bypass_counts）；每钩子超时（connect/head 1s、
  body 5s）按 Err；on_log 在类型上就不可外溢（同步、返回 unit）。
- Flow::Early 短路语义（mock 类插件的接缝）：request_head 可整条替换响应，
  response_head 可重写上游响应；两者都不触碰上游连接。
- max_buffered_body 从编译期常量升级为快照字段（EngineConfig 可选，默认
  8 MiB）；超限 413/502 而非静默流式。
- 测试/故障注入旋钮 debug-inject（Extension 层，注册表在册——注入路径走
  真实装配校验，不是旁路后门；产品链恒为空，对齐 envboard-core-fake 先例）。
- 验收：单测 7 条（拓扑与注册表拒绝面）+ tests/plugins.rs 实机 6 条
  （fail-closed 带名、bypass+WARN+计数、钩子超时、Early 不触上游、
  connect 阶段失败、未注册 id 启动即拒）。

### Added（v3 重构 M-P2：数据面——进程内引擎实例）

- envboard-core::engine：**引擎实例 = 一个 OS 线程 + 一个 current-thread
  runtime + 一个监听端口**。状态机 starting → running / port_conflict /
  failed：绑定即真相（EADDRINUSE → port_conflict，不换端口、不试绑）；
  请求任务级崩溃隔离（tokio 逐任务捕获），引擎线程 panic → failed。
  apply_config 同步换入全量快照（ArcSwap），回执 config_hash 与 epoch；
  listen 是停机字段，热改它按 invalid_config 拒绝。v2 的状态文件 TTL、
  收敛窗口、/proc 身份、试绑在这条路径上整体消失（manager 侧删除在 M-P4）。
- 数据面能力：CONNECT 隧道 + MITM 按 SNI 现签 + HTTP/1.1 缓冲转发；
  absolute-URI（http scheme）正向代理；407 鉴权门（CONNECT 与 absolute-URI
  同一条门，凭据只在内存里定长时间比对）；insecure_hosts 经
  ConnectTarget.tls_policy 落到连接执行器的两个档位（严格 = 系统信任库，
  放宽 = 仅精确命中可用；不存在"全局关校验"的表达路径）。改写语义与 v2
  注入器逐项一致：只改连接目标，不动请求内容、不动 Host 头、不动 SNI 基准。
- ConnectTarget（envboard-core::target）：authority / sni / resolved_addr /
  tls_policy / chained_proxy（恒 None 的扩展位）——基础配置与后续插件管道的
  唯一接缝。
- body 默认缓冲（8 MiB 上限，超限 413/502 而非静默流式）；101 升级走透传
  旁路，upgrade 链的请求头原样带走。上游连接一问一答（不复用）：热改规则/
  名单不会被连接池里的旧策略穿越。
- 验收：core/rs/crates/envboard-core/tests/data_plane.rs 六组 hermetic 断言
  （规则命中 200、未覆盖 502、407 门、insecure 对照 + 热翻转不重启、
  port_conflict、隧道保活多轮）。

### Added（v3 重构 M-P1：纯库引擎骨架 envboard-core）

v3 把数据面重构为**纯 Rust 库 + 进程内引擎实例**（分支
`refactor_envboard_v3_rust_engine_20260917`）。本阶段不改运行行为：manager 仍走
mitmproxy，本条目记录的是独立库面的落地。

- 新 crate `envboard-core`（内部依赖只有 `envboard-core-api`，已登记进 `deps`
  门禁边表）：
  - **共享 CA 的加载与物化**：直接读既有 confdir——`mitmproxy-ca.pem`（PKCS#1 RSA
    私钥 + 证书拼接）与 `mitmproxy-ca-cert.pem`。判据沿用 v2 并逐条实现：私钥与证书
    公钥**逐字节配对**、`-ca.pem` 内嵌证书与 `-ca-cert.pem` 一致、basicConstraints
    CA:TRUE、未过期；任一不满足 → 删除并重新物化，且**以 `CaOutcome::Regenerated
    { reason }` 交回上层**——坏 CA 的处置必须可见，禁止静默带病服务。
  - **新生成的 CA 用 ECDSA P-256**（签发库 rcgen 0.14 的 ring 后端不支持生成 RSA
    密钥，文档明示只有 aws-lc-rs 能生成）；**加载既有 RSA CA 并用它现场签发**已由
    进程内握手测试实证（`core/rs/crates/envboard-core/tests/mitm.rs`，fixture 为
    纯测试假值 CA，密钥无外部用途）。
  - **叶子证书按 SNI 现签现缓存**：容量 512、满则整体清空重签；剩余寿命不足 30 天
    即重签；私钥文件写盘 0600。
  - **rustls 接线**：MITM 服务侧配置（客户端 ALPN 只协商 `http/1.1`）与上游客户侧
    严格校验配置（系统信任库 `rustls-native-certs`）。信任库来源从 certifi 换成
    系统库会影响"哪些域名免名单通过"的集合，最终由 v3 README 的已知限制收口。
  - **自写定形 DER 读取**（TLV、PKCS#1↔PKCS#8 封装互转、证书 SPKI/CA 标志/有效期）：
    PKCS#1 兼容面不随上游库的支持摇摆。
- 新增 workspace 依赖：rustls 0.23（ring provider）/ tokio-rustls / rcgen 0.14 /
  rustls-native-certs / rustls-pemfile / time；`Cargo.lock` 已入库（rcgen 首次引入
  需一次联网预热，已完成）。
- **v1 协议面限制**：ALPN 只声明 HTTP/1.1，强制 h2 的客户端（gRPC 等）会失败——
  这是写明的非目标，不是静默降级。

### Added（按域名放宽上游证书校验）

- `Environment.insecure_hosts`：**完整域名清单**（默认空），命中 ⟺ 归一化后的 SNI 与该
  域名**完全相等** —— 无通配符、无子域继承、无后缀匹配；写入时**拒绝 `*` / `?`**：
  放宽上游证书校验是安全控制，必须逐条点名。**没有"整个环境全关"的开关。**
  - 生效方式是注入器接管 `tls_start_server`：名单内域名用 `VERIFY_NONE` 自建上游 context
    （ALPN / cipher / TLS 版本 / ECDH 曲线与不改写时逐项一致），名单外一律走 mitmproxy
    自己的严格校验。无 SNI 时退回上连地址判定；SNI 存在但没命中**不**退回。
  - **热生效**：清单写进每个环境自己的 `config.json`，注入器按轮询间隔（默认 5s）重读，
    运行中改不需要重启。

### Changed (breaking)（配置通道与规则寻址重做）

- **删除 `Environment.options`（任意 core 选项透传）**：它是一条绕过契约的任意通道，
  让"放宽上游证书校验"这类安全控制可以不经一等字段被打开。空对象 / `null` 作为**墓碑**
  接受并忽略（旧版每个环境都写 `options: {}`），**非空即 `invalid_config`** 并指出替代路径。
  `envboard env add/edit --option` / `--no-options`、`InstanceSpec.options`、
  `validate_options` denylist 全家桶、工作台的选项开关与文本框一并删除。
  （这条能力在被删除前**从未发布**，所以这里是直接移除，而不是"废弃再删"。）
- **`Environment.proxy_auth` 拆成 `proxy_user` + `proxy_password`**：同生共死（只有一边
  指向缺失的那一边报错）、字段级错误路径、不含 `:`（mitmproxy 的 `split(":")` 要求恰好
  一个冒号）。凭据明文只落在 0600 状态文件与实例启动参数；视图 / 日志 / SSE 只给
  `proxy_auth_enabled` 布尔，并且**记录进程身份时 cmdline 里的凭据值先脱敏成 `***`**
  （比对存活进程时两侧都脱敏）。旧形状读时**无损迁移**（整体 `trim` 后按第一个 `:` 切开），
  写出不再输出。
- **脚本 `--set` 通道收口**：注入器不再注册任何 `envboard_*` 选项，改读
  `<自身目录>/config.json`；规则也不再经 `--set envboard_rules=<path>` 下发，而是
  `<自身目录>/envboard.rules` 这条**固定名软链**。启动命令行只剩 `-s` / `confdir` /
  `listen_host` / `listen_port` / `proxyauth` —— **没有用户可控的 `--set`。**
- **规则库进账本**：`state.json` 增加 `rules[]`（含渲染后的完整正文），
  `<rules_dir>/<name>.rules` 变为**可再生物化产物**：缺失或被改坏按账本逐字节重建；
  物化目录里有而账本里没有的文件在启动对账时**回填**（升级不丢规则库）。
- **"规则缺失"不再让启动失败**：链不存在/悬空 ⇒ 该环境**不覆盖任何域名**（视图给出
  `rules_missing` 提示并尽量自愈）。绑定一个**不存在**的规则名则在写入时就拒绝 ——
  否则那会变成一次静默失效。
- **热 / 停机矩阵重排**：热 = `description` / `insecure_hosts` / `rules` 绑定 / 规则内容
  （绑定热生效由"原子换链 + 重写 config.json"实现）；停机 = `name` / `listen` /
  `proxy_user` / `proxy_password`。
- **健康判定增加配置哈希回执**：管理器记录期望配置的 sha256 与写入时刻，实例回执
  `config_hash`；不等但在 `reload_interval + 3s` 的**收敛窗口**内算收敛中，超窗才报
  `config_mismatch`；实例热重载失败写 `config_error` → `unhealthy`。状态文件里没有
  `config_hash` 的实例判定为**上一代**，reconcile 走 `restart`。
- 契约 fixture 重新基线化：删除 11 个 options 用例、重写 6 个凭据用例，再加上新增用例，
  总数 **58 → 68**（`environment/` 30 + `merge/` 9 + `rules/` 8 + `ports/` 6 +
  `lifecycle/` 7 + `insecure/` 8）。

### Changed (breaking)

- `spec/capabilities.md` **重写**：领域模型从 v1 的 8 字段一路收到**当前的 7 个**
  （`name` / `listen` / `rules` / `insecure_hosts` / `description` / `proxy_user` /
  `proxy_password`；中间形态是 5 字段 + `options`，再补第 6 个 `proxy_auth`，
  见上面那条 breaking）。
  删除 `dns_servers`、`hosts`、
  `domain_suffix`、`color`、`labels` 与 mapping / annotate / activate / resolve
  全部语义 —— v2 的切换模型是"一个环境 = 一个独立实例 + 一个独立端口"，
  不再有"当前环境"这个全局状态。
- `spec/errors.md`：删除 `dns_failure`、`disabled` 与 `rcode` 表；
  新增 `port_conflict`、`port_range_exhausted`、`config_mismatch`，并补上
  环境健康状态表（`stopped` / `starting` / `running` / `port_conflict` /
  `config_mismatch` / `unhealthy` / `failed`）。
- 契约 fixture **重新基线化**：v1 的 11 个 `environment/` + 2 个 `merge/` 全部重写
  （它们测的是被删除的字段），新增 `ports/` 与 `lifecycle/` 两组。
  当前总数与分目录见上面最后一条 breaking（**68** 个）。

### Added

- `spec/rules.md`：hosts 规则语法的 **BNF**，作为 Rust 实现与 Python 注入器的
  共同仲裁（v1 只有散文式描述，无法支撑"两侧输出逐字节一致"这条要求）。

### Added

- **Rust 核心（v2 M0 + M1）**：多环境管理器的实现落地，由 7 个 crate 组成，
  依赖方向由 `scripts/rust_dependency_lint.py` 强制（core-api 是根，纯逻辑 crate
  不得依赖运行时）。已实现的部分：
  - `envboard-core-api`：`ProxyCore` trait、`InstanceSpec`、`StatusReport`、统一错误码、
    `ClockPort`/`LoggerPort`（`InstanceSpec.options` 与其 denylist 已删除）。
  - `envboard-domain`：环境模型（校验/归一化/PATCH 合并；字段数随后续 breaking 变动，
    当前 7 个）、端口选择（`port.allocate` 的全部规则）、reconcile 决策
    （孤儿清理与 PID 复用保护）。
  - `envboard-rules`：hosts 解析与**确定性渲染**，与 Python 侧在 63 个用例上逐字节一致。
  - `envboard-core-fake`：只监听端口的测试替身 core —— **管理器测试因此完全不依赖
    mitmproxy**（"没装 mitmproxy 的机器也能跑默认流水线"这条验收条件）。
  - `envboard-manager`：环境 CRUD、随机端口分配与持久化、状态存储（原子写 + 0600）、
    单实例锁（flock）、健康判定（状态文件为主 + 契约回显比对）、reconcile、规则导入。
  - `envboard-cli`：`envboard` 二进制（`env`, `rules`, `run`, `status`）。
  - `envboard-contract-tests`：**消费 `spec/fixtures` 全部 golden case**。
- `scripts/verify_dual_impl.py`：跨语言对拍门禁 —— 63 个用例里 Rust 与 Python 输出必须
  逐字节一致，**已知分歧必须显式声明**（声明了却不再分歧也会失败，防止白名单掩盖新分歧）。
- `scripts/rust_dependency_lint.py`：Rust 侧的依赖方向与纯度门禁（含"纯逻辑 crate 不得
  依赖 tokio/libc"、workspace 根唯一、`Cargo.lock` 与 `rust-toolchain.toml` 在位）。
- `ci/verify.sh` 新增步骤：`rust-dep-lint` / `rust-fmt` / `rust-clippy` / `rust-check` /
  `rust-build` / `rust-test` / `contract-dual`，全部带 `--locked --offline`。

### Added（M2 — mitmproxy core + 注入器）

- `adapters/mitmproxy/envboard_mitmproxy.py`：**单文件、纯标准库**的注入器 ——
  读 `<自身目录>/config.json` 与固定名规则软链、在 `server_connect` 里改写上连目标、
  按域名放宽上游证书校验、回写状态文件。它只认自己目录旁边的固定路径，
  不碰别的状态目录 —— 所以随时可以丢掉。既是 addon（`-s`），也有 `--render` 模式
  供跨语言对拍（该模式**不需要 mitmproxy**）。
- `envboard-core-mitmproxy`：`ProxyCore` 的真实实现 —— 按启动契约拼 `--set` 参数、
  spawn `mitmdump`（带 `PR_SET_PDEATHSIG`）、**预物化并校验共享 CA**、
  物化注入器、等状态文件而非等进程、SIGTERM→SIGKILL 停止、按身份探活。
- `core.python` 的推导规则落地：shebang → uv-tool 布局 → 同目录，**每一步都真跑一次
  `import mitmproxy` 自检**，并要求与 `core.bin --version` 的版本一致。
  裸命令名（默认 `mitmdump`）先经 PATH 解析 —— 否则读不到 shebang，
  实测会报"找不到带 mitmproxy 的解释器"而它其实就在 PATH 上。
- **契约回显自检机制**：注入器核对"管理器下发的每个 `--set`"是否被宿主接受
  （`options_echo`），管理器据此判定 `config_mismatch`。宿主对拼错的 `--set`
  是静默忽略的，只有让注入器去 `ctx.options` 里查才知道。当前下发的键只剩
  `proxyauth`（安全控制，被静默忽略等于裸奔），外加 `config_hash` 这条更硬的回执。
- 实机测试 `live_manager.rs`：真管理器 + 真 mitmdump + 真改写（200 vs 502 判别性对照）
  + 代理凭据下发后 `options_echo` 必须回 "ok"；没有 mitmdump 时**跳过**而不是失败。

### Changed（日志通道与存活判定：从"读管道"改成"文件直写"）

- **实例日志默认改为文件直写**（`<state_dir>/logs/<env>.log`）：子进程的 stdout/stderr
  用 `Stdio::from(File)` 直接接文件（两路 `try_clone()` 共享 offset），**没有管道、
  没有读线程**。原来靠"两个读线程把管道读走"来避免子进程阻塞；实测管道容量 64 KiB、
  默认详细度约 307 字节/请求 → **213 个请求就能写满**，写满后 mitmdump 阻塞在自己的
  事件循环里、所有客户端一起挂住（不读管道的对照实验：第 212 个请求开始失败，读走
  65116 字节后立刻恢复）。文件由内核写盘，我们进程不在链路上，这条路径从设计上消失；
  顺带换来"日志跨重启留存、崩溃现场可追溯"。
  - 每次启动追加一行运行标记分段落；`PYTHONUNBUFFERED=1` 两种形态都保留
    （Python 对非 TTY 的文件同样是块缓冲）。
  - 新增 `--no-log-file`（退回"管道 + 内存环形缓冲"，磁盘零写入）与
    `--max-log-bytes`（默认 8 MiB，`0` = 不轮转，下限 64 KiB）；`--log-dir` 由
    "可选落盘目录"变为"覆盖默认目录"。
- **轮转是 copytruncate**（尾部搬去 `<env>.log.1` 后 `set_len(0)`）：**不能 rename** ——
  子进程还持着那个 inode 的 fd，改名之后它会继续写被移走的文件，新文件永远是空的。
- **读尾部改为有界**：只从文件末尾读 256 KiB 窗口再切行，并可跨 `.1` 往前拼；
  删掉原来整个文件 `read_to_string` 的兜底（日志一大就会把管理器拖住）。
- **判活看 `/proc/<pid>/stat` 的状态位**：僵尸（`Z`/`X`）不再算活着。原来只比
  PID + starttime，而僵尸这两个量都没变 → 判成"活着"。新增每 500 ms 的**回收任务**
  用 `try_wait()` 收尸（tokio 1.53.1 该方法是公开同步的）。一处修好，`health`
  / `reconcile` / 孤儿清理 / `terminate` 全部同时受益。
- **视图层与权威判定同源**：`health_blocking()` 原来"只要句柄表里有这个环境就返回
  `running`"，与带状态文件 TTL 的 `health()` 分叉；实测 `kill -9` 掉实例后工作台
  **60 秒仍报 `health=running, reason=None`**。现在两条路径共用同一套判据
  （进程存活 → 状态文件时效 → 契约回显），并且"由本管理器拉起、又没人叫它停、
  进程却没了"报 `failed`（带退出码或信号）而不是 `stopped`。
- 管道路径（`--no-log-file`）硬化：单行 8 KiB 上限、写盘不再每行 open/close、
  `Mutex` 中毒不再 `unwrap()`（一个读线程 panic 会连带把另一个也弄死，管道就再没人读了）。
- 工作台日志面板跟随 SSE 快照刷新（原来只在点"日志"时取一次，之后永远停在那一次快照上）。

### Added（文本自包含门禁）

- **`scripts/doc_scope_lint.py`**：包内文本不得引用本仓之外的本地文档（设计文档、开发计划、
  流程规范、工作区规范文件）：不给路径/链接/章节号、不点名字；指向本仓的路径必须真实存在；
  含 `§` 的行必须写明是哪份本仓文件（或"本文件"）。纳入 `ci/verify.sh` 默认层
  （`doc-scope-lint` 步骤），判据与豁免见 README 的「文本自包含」一节。
- 按同一条规则清理了包内既有引用：README / CHANGELOG / `spec/**` / 验收文档与
  **代码注释**里对外部文档的章节号引用、风险表编号（如 `R14`）一律改成自包含表述或指向
  本仓内真实存在的文件与标题；顺带修掉两处失效引用（对拍入口指向已删除的脚本、
  清单里"断言已不存在"的路径改为显式标记）。

### Added（部署工件）

- `scripts/systemd/envboard.service`：用户级 systemd unit（常驻管理器 + 工作台），
  附 headless（`run`）变体。设计取舍都在文件内注释：`StateDirectory=` 建状态目录
  （`ReadWritePaths=` 要求路径预先存在，首次安装会以 `226/NAMESPACE` 失败 —— 实测踩到）、
  `ExecStart` 全用绝对路径（systemd 的 PATH 不含 `~/.local/bin`）、`Restart=always` 的
  安全性来自持久化的期望状态、`KillMode=mixed` 避免孤儿实例占端口。
  已在真机跑完一轮启停：实例随服务停止而退出、重启后 reconcile 自动拉起、经代理的请求 200。

### Added（日志通道与存活判定）

- `envboard-manager/src/logs.rs`：日志文件的有界尾部读取（含跨轮转拼接）与 copytruncate
  轮转；日志文件落在管理器的状态目录里，所以**读与轮转都只有这一个实现**，
  core 侧不重复一份。
- `ProxyCore::last_exit`：进程**自己退出**时的退出原因（信号/退出码），供同步的视图层
  在不 `await` core 的前提下给出准确结论。
- 实机断言：`verify_live_v2.py` 新增 check 8（连打 320 个请求——足以越过旧的 64 KiB
  卡死阈值——全部成功，日志文件 97 KB 为证）与 check 9/9b/9c（实例被 SIGKILL 后工作台
  不再报 running、说清是被信号杀死、子进程被回收、日志仍可读、能重新拉起）；
  `live_manager.rs` 新增同款崩溃用例并覆盖 reconcile 重新拉起。证据与实测数字见
  `docs/acceptance/log-channel.md`。

### Fixed（日志通道与存活判定）

- `view_of` 有调用点是在**持有 `state` 锁**时调用 `health_blocking` 的，而
  `std::sync::Mutex` 不可重入 —— 判定路径里再取一次状态锁会让 `env add` 直接挂死。
  现在环境定义由调用方传入，判定路径不碰状态锁。

### Added（M3 — 工作台）

- `envboard-web`：axum 路由 + **内嵌前端**（`index.html` / `app.css` / `app.js`，
  全部 `include_str!`，零前端构建链、零 CDN）。
- REST 面：环境 CRUD/启停/重启/**显式重分配端口**/日志尾部、规则导入/查看/删除、
  **跨环境静态对比**（承接 v1 `all_envs` 的用例意图，且不发任何请求）、SSE 快照。
- 安全（三条都不可省）：默认只监听回环、Host 头校验防 DNS rebinding、
  变更类路由要求自定义头防 CSRF；非回环监听必须给 token。
- 严格 CSP（无 `unsafe-inline`），因此**样式与脚本必须外置**；验收断言三件事：
  JS 真的跑了 + 样式真的生效（读计算样式）+ 控制台无 CSP 报错。

### Fixed

- 端口探测改为**惰性**（找到第一个空闲端口即停）。本机实测单次 `bind` 约 **21ms**
  （WSL2 mirrored 模式，连绑定端口 0 也一样），按原设计"先把整个区间探一遍"会在
  16000–16999 上花约 21 秒；现在常见情形只探 1 次，最坏仍是 `max_attempts` 次。
- 注入器**不能**写 `from __future__ import annotations`：mitmdump 用 `-s` 加载脚本时
  不登记 `sys.modules`，而 `@dataclass` 解析字符串注解会去查它 → 启动即崩。
- mitmproxy 的选项系统**不支持 `float`**（启动即失败），间隔类选项一律用整秒。
- axum 0.7 的路径参数语法是 `:name`（`{name}` 是 0.8 的），写错的表现是**全 404**；
  另给 API 加了 JSON 404 兜底，避免前端只能报"响应不是 JSON"。

### Added（编辑已有环境）

- **工作台的「编辑」**：每行一个入口，把「新建环境」表单切到编辑模式（名字 / 端口 / 规则
  绑定 / 实例选项 / 描述五个字段全可改），提交走 `PATCH /api/environments/:name`。之所以
  复用同一个表单而不是行内编辑：字段集本来一样，两套 DOM 迟早不一致（v1 的教训）。
  实例**运行中**会把名字 / 端口 / 绑定 / 选项四个输入框锁住并说明原因，只放开描述 ——
  服务端仍会拒绝，但界面先把这条路封住，免得用户改完再吃一个红字。
- **规则库的「载入」**：把已导入的规则文件原文取回表单再改。原来只有「导入」，
  改一份规则等于盲覆盖（实测 10.8 KB 的规则文件就是这么被当成"没法编辑"的）。
- **CLI `env edit`**：`--rename` / `--port` / `--rules` / `--no-rules` / `--option` /
  `--no-options` / `--description`，空 patch 响亮失败（而不是"成功但什么都没改"）。
  与工作台**共用同一份** PATCH 构造，避免两处字段映射漂移。`ApiClient` 因此补了 `patch()`。
- **CLI `rules show`**：取回规则文件原文（改之前先看，同样是防盲覆盖）。
- 实机断言：`verify_live_v2.py` 新增 check 10 —— 运行中换绑定返回 409、描述热改 200、
  停止后补绑规则并改名换端口后，**真 mitmdump 按新配置把域名送到上游**（未绑定时是 502）。
  管理器侧新增 4 条用例（描述热改 / 绑定需停机 / 改名搬迁 / 旧标记作废），
  `thin_client.rs` 覆盖 `env edit` 走 HTTP 的往返。证据见
  `docs/acceptance/edit-environment.md`。

### Changed（编辑语义）

- **运行中换规则绑定改为拒绝**（`conflict`）。原来 `update()` 的注释写"改绑定是热更新"，
  与事实不符：实例在启动时通过 `--set envboard_rules=<path>` 固定规则**路径**，运行中换绑定
  它根本看不见，结果是"配置说绑了 A、实例还按 B 干活"。热重载的**是内容**（mtime 轮询
  同一个路径），这条能力不变。描述仍然随时可改。
  这个分叉是被一条单元测试逮到的：断言"热改绑定后仍是 running"，实际得到 `config_mismatch`
  —— 状态文件的 `rules_count` 回显如实报出了不一致。
- 改了 `listen` 或 `rules` 绑定即作废该环境旧的失败标记（`port_conflict` /
  `config_mismatch`）：标记陈述的是"上一个配置失败了"，留着它会让界面拿**新**端口号报旧冲突。
- `update()` 补一条审计日志（`updated <env>: changed name, listen, …`），
  系统日志里能看到"谁在什么时候动了哪个环境的哪几项"。
- **CLI 的本地 API 发现不再跨状态目录兜底**：显式给了 `--state-dir` 时只认该目录下的
  `runtime/api.json`，不再退回默认端口。原来 `--state-dir /tmp/scratch` 会被 8900 上
  一个跟它毫无关系的常驻实例接管 —— 命令看着成功了，改的却是**别人的**状态
  （本机实测撞上：`env add` 打到了正在运行的工作台上）。

### Fixed（编辑已有环境）

- **改名之后日志面板指向旧名字**：面板的目标名字不会跟着改，之后每秒一次的尾部读取都是
  404；而那个 404 会让整次 `refresh()` 失败，把"已保存。"顶成一条红字错误提示
  （真浏览器验收时踩到：用户会以为改动没生效）。现在面板目标跟着环境列表自愈，
  日志读取的失败也只落在日志面板里。
- **名字输入框的 `pattern` 在 `v` 标志下是非法正则**：Chrome 校验 `pattern` 用 `v`
  （不是 `u`），而 `v` 模式里裸的 `-` 是非法字符类成员 —— 于是整个 pattern 被浏览器
  **静默忽略**，只在控制台留一行 `SyntaxError`，原生校验形同不存在。转义成 `\-` 后
  两种模式都合法（`9bad` / `Beta` 现在真的会被前端挡下）。这类故障 curl 断言看不出来，
  只有真浏览器的控制台会说话。
- `doc-scope-lint` 漏掉**无后缀文本文件**：按后缀筛文件时 `.gitignore` 整类不在扫描范围内，
  而它的注释同样可能指向本仓之外的文档（实测确实有一条）。现在把 `.gitignore` / `.dockerignore`
  显式列入扫描集合（文件数 111 → 112）。

### Changed（文档脱敏）

- `docs/acceptance/v0.1.0.md`（v1 实机验收转录）里的家目录用户名与当时读到的**真实系统 DNS 地址**
  换成通用值：家目录统一写成 `/home/u/`（形状不变），DNS 地址改为文档专用网段 `192.0.2.53`
  （RFC 5737）。断言、时间戳、退出码一并保持原样 —— 脱敏只改"能指回某个具体人/某台真实机器"
  的部分。文件开头写明了这次替换，避免读者以为它就是当时的原始输出。
- 同一轮排查确认**真实 hosts 文件从未入库**（`examples/hosts.txt` 一直在忽略之列，
  入库的是脱敏示例 `examples/hosts.sample.txt`，用的是 RFC 5737 地址与 `example.com`）。

### Removed（M4 — 收敛与迁移，breaking）

- **v1 的 Python 实现整块移除**：`src/envboard/`（model / registry / mapping / resolver /
  rules / ports / errors / addon / web）、`addons/envboard.py`、`web/*`、v1 的 81 个单测，
  以及 `pyproject.toml`（本包**不再发布 Python 分发物** —— Python 这一侧只剩被二进制内嵌的
  单文件注入器）。`git tag v0.1.0` 保留 v1 全貌。
- 随之退休的门禁脚本：`dependency_lint.py`（分层检查已由 `rust_dependency_lint.py` 覆盖）、
  `pack_check.py`（改校验二进制产物）、`verify_live.sh`（55 条断言里能脚本化的部分迁到
  `verify_live_v2.py`）。
- 命名门禁**收紧**：没有分发物之后，`slothtron` 这个 token 在整棵树里（路径与代码内容）
  **零命中**，唯一例外是规则脚本自身。

### Notes

- `scripts/artifact_check.py` 取代 `pack_check.py`：校验发布物（二进制 + 内嵌注入器 +
  必需产物 + 零 v1 残留），并断言注入器源码真的被 `include_str!` 嵌进了二进制。
- `ci/verify.sh` 的 `smoke` 换成 v2 形态：二进制能跑 + `--core fake` 建环境 + 注入器
  `--render` 在**没有 mitmproxy**的解释器下也能跑。
- 实机层脚本化（`scripts/verify_live_v2.py`）：两个环境同时可用且结果不同、规则热重载、
  Host/CSRF 防护、CSP 形态、日志落盘、崩溃恢复 —— 补上了 v1 立下的
  "断言必须可重跑"这条规矩在 v2 上的缺口。

### Changed（工作台 UI 升级：明亮主题三视图）

- 前端三资产（`core/rs/crates/envboard-web/assets/` 下的 `index.html` / `app.css` /
  `app.js`）整体重写：从"单表格 + 深色"升级为明亮主题的三视图工作台 —— 环境 /
  规则库 / 跨环境对比三个视图、统计卡、环境详情（概览 / 配置 / 规则三页签）、
  可折叠日志栏（按事件类型过滤 + 搜索 + 跟随开关）。**REST API、SSE 与 CSP 形态
  零改动**，这是纯前端层的重写。
- 设计令牌收进 `app.css` 第 ① 区：表面三级、边框两级、文本四级灰阶（带对 `--panel`
  的实测对比度）、语义色「原色 + bg(10%) + border(30%)」三件套、4px 间距基数、五档
  圆角。组件层不写十六进制色值，主题切换只改令牌区。规范与理由见 `docs/ui-spec.md`。
- 破坏性操作（删除环境）改为**就地二次确认**（后果说明 + 确认/取消），不再用原生
  `confirm()` —— 与"无内联脚本"的 CSP 约束一致，且样式可控。
- 图标改用同文档 SVG `<symbol>` 精灵 + `<use>`：不引字体、不发请求，
  `stroke: currentColor` 使同一份图标随语境变色。
- 可访问性：`prefers-reduced-motion` 降级；`[hidden]` 加 `display:none!important` 修正
  （作者样式会盖过 UA 默认值）；文本四级灰阶全部达到可读对比度。
- 日志栏过滤维度是**事件类型**（请求/响应/连接/运行/异常）而不是日志级别：实测日志行
  不带 INFO/WARN 前缀，按级别过滤会是个假功能。

### Fixed（工作台 UI 升级）

- **引导链隔离**：`renderLogs` 排在 `connectEvents()` / `refreshAll()` 之前且无保护，
  一个写错的容器 id（`log-filters` 改名只落在 CSS/JS 两侧，HTML 未跟上）就能让
  `renderLogFilters` 抛空指针，进而中断整条引导链 —— 数据请求根本不发、页面停在空壳、
  `envboardReady` 永远停在 `"pending"`。现在每个引导步骤过 `step()` 壳：坏掉的步骤记进
  `dataset.envboardError`、ready 明确置 `"no"`，数据加载照常继续。
- `index.html` 注释里写出的脚本标签字面量会命中 `scripts/verify_live_v2.py` 的
  「页面源码里无内联脚本」断言（实机层 4a FAIL）—— 注释改写为不出现该字面量。

### Fixed（工作台 UI 合规修复）

三份走查/审计记录（`docs/acceptance/ui-design-conformance.md`、
`envboard-ui-compliance-audit.md`、`envboard-ui-audit-report.md`）合并后的修复。
第三份的对照基准写的是 v1.0，与仓内现行的 v2.0 规范版本错配，其 6 项量化结论经
实测反证后未采纳（如"0.8px 边框"其实是 `1px ÷ dpr 1.25` 的设备像素换算、
"`.info-grid` 无分隔线"其实已有 1px 缝隙网格）。

**可用性**

- **顶栏在 641–720px 溢出且刷新按钮不可达**：顶栏固有最小宽度 721px 而没有横向
  滚动，`body` 的 `overflow: hidden` 会把溢出部分裁掉且滚不到 —— 实测 720px 溢出
  1px、681px 起刷新按钮完全移出视口。新增 720 断点（隐藏版本徽章、视图切换只留
  图标、图标按钮放大到 36px），并让两枚状态徽章可收缩省略，使顶栏宽度不再随
  SSE 文案变化（断线时连接徽章会变长约 80px）。实测 641–900px 全区间零溢出。
- **日志时间戳对比度 3.91:1 低于 WCAG AA**：`.log-ts` / `.log-peer` 原用 `--muted-2`，
  而日志栏底色是 `--bg-2`，`--muted-2` 只对白系表面达标。改用 `--muted`（实测
  4.66:1）。侧栏的空态文案与日志的 `k-conn` / `k-raw` 标记同病同治。
- **重分配端口没有二次确认**：原先点一下就直接 `POST /reallocate`，而端口一变客户端
  此前的 `export https_proxy=…` 立即失效。改为就地二次确认并写明该后果。

**规范承诺但未落地的部分**

- 规则库补「N 条 · M 个 IP」（优先读规则文件头的 `# entries: N ip: M`，缺失时按
  数据行现算），列表不再需要逐个「载入」去数。
- 日志栏补「导出」与「清空」：导出当前视图（尊重过滤与搜索）为 `.log`；清空只清
  视图缓冲并同时暂停跟随（磁盘日志文件不动）—— 不暂停的话下一个快照就把它拉回来。
- 表单补**字段级错误文本**：`markInvalid` 在输入框下方插一行原因（原先只有描边变色，
  原因只走会自己消失的 toast）。
- 状态徽章改为**中文标签**，原文移入 `data-health`（`config_mismatch` 这种原始值写在
  徽章上等于没写）；概览的「状态 / 期望」改为「实际状态 / 期望状态」，不等时标红。
- 复制成功后按钮原地反馈「已复制」（不整块重渲染，否则会被每秒的快照冲掉）。
- 日志栏无日志时给空态文案，区分"本来没有"与"过滤后为空"。

**设计系统收敛**

- 字号收进 `--text-xs…3xl` 九档并转换组件层 48 处声明，消灭 10 / 10.5px；
  新增 `--input-h` / `--btn-h` / `--btn-h-sm`。
- 组件尺寸对齐规范：徽章 20→24、统计卡图标 34→38、统计值 20→22、输入框 30→32、
  日志类型标记 46→44、命令文本 12→12.5、信息值 13.5→14。
- 消灭规范外的第四种按钮高度：`.btn.icon-btn.sm` 26×26 → 30×30。
- 间距孤立值收进令牌：输入框 `padding: 6px`（规范点名禁止值）、徽章 `gap: 5px`、
  信息格 `gap: 3px`、环境项 `gap: 2px`、视图切换 `padding: 3px`。
- Tab 条底色 `--bg-3` → `--bg-2`；空态图标 48px 色块 → 40px @50%；
  新增计数 chip 激活态；切换类控件补 `:active`；禁用按钮不再响应 hover。
- 统计卡的「环境总数」不再默认高亮：它代表"不筛选"，不是被选中的筛选项。
- 侧栏计数改为跟随当前可见条数（原先筛到空列表时右侧仍挂着总数）。
- 侧栏页脚改为展示**访问地址 + 暴露面 + 数据新鲜度**（原来显示状态目录）。

**规范与文档对齐**

- `docs/ui-spec.md`：圆角纪律改按实际层级（卡片 10px，不是 lg；原措辞与
  `docs/design/ui-spec-full.md` 冲突）、补 `--muted-2` 只在白系表面的纪律、
  补字号尺度与尺寸三档、响应式断点补 720 并说明它为何是硬需求、删掉未实现的
  "抽屉 / 日志栏加高"描述。
- `docs/design/ui-spec-full.md`：删除 Dropdown 与 ActivityList 两个组件并写明取舍
  （CSP 禁内联 style，浮层代价高）、日志过滤维度改为事件类型、Toast 位置改为右下、
  日志行时间戳说明"允许为空"、`--purple` 补上"日志分段标记"这第二个固定用途、
  基础集去掉没有界面的「设置」与「文件」、头部去掉不可解析的路径引用。
- `docs/design/tokens-light.css` 按 `app.css` 第 ① 区重新生成（新增 `--radius-xs`
  等 4 项缺失令牌）；深色块只在令牌文件保留一份，`app.css` 不再重复。
- `app.css` 注释补上 1px 缝隙网格与 Tab `-1px` 负边距的意图说明（它们看着像间距违规）。

### Added（安全增强 — URL token / 代理鉴权 / 对外服务开关）

- **工作台 URL token 鉴权（默认启用）**：token 鉴权**开箱即开** —— 不给 `--token`
  时自动生成 128 bit 随机 token（`/dev/urandom`；异常环境退化到时间+PID 混合），
  启动日志打印 `dashboard: http://<host>:<port>/?token=<值>`
  （通配监听地址用 `127.0.0.1` 展示），点击直达。显式 `--without-token` 才能关闭，
  且**非回环监听拒绝关闭**（无鉴权对外不允许）；与 `--token` 同给是配置冲突。
  生成的 token 写入 `<state_dir>/runtime/api.json`（0600），CLI 瘦客户端自动发现并
  带上，不再依赖用户手抄。`guard()` 在 header（`x-envboard-token`）优先之外接受
  URL `?token=`（浏览器直接打开工作台、SSE 的 EventSource 都带不了自定义头），
  比较改为常量时间；前端捕获后立即 `history.replaceState` 抹掉地址栏中的 token，
  之后全部请求走 header。`/app.css` / `/app.js` 内嵌静态资产豁免 token（浏览器子资源请求带不了凭据），API 与页面本体不豁免。
- **环境字段 `proxy_auth`**（契约 4 字段 → 5）：`user:password` 形态（恰好一个冒号、
  两段非空、无空白/控制字符、总长 ≤128），`null` = 不启用。启用后实例以
  `--set proxyauth=<user:password>` 启动并纳入 `envboard_expect` 回显自检；
  `proxyauth` 加入两级 options denylist（凭据唯一来源是环境字段）；运行中修改返回
  `conflict`（实例启动时读取）。`InstanceSpec` 增加 `proxy_auth`；
  `EnvView` 增加 `proxy_auth_enabled` 布尔，视图（含代理命令）**不回显凭据**。
  状态存储本就原子写 + 0600，凭据入库沿用该通道。fixtures：`environment/` 新增
  7 个（2 valid + 5 invalid），总数 40 → **47**（`valid-listen-all-interfaces`
  钉住 `listen.host = 0.0.0.0` 的合法性）。
- **工作台「对外服务」开关与访问鉴权表单字段**：环境表单新增
  「对外服务（绑定 0.0.0.0）」复选框与「访问鉴权」（password 输入）字段；
  `formPayload` 修复"编辑只发 `{port}` 导致 host 被整体替换语义重置回默认"的隐患
  （host 或端口任一变化时两个字段都发全）；勾选 0.0.0.0 而未启用鉴权时表单给
  红色警示；概览新增「访问鉴权」格、非回环监听地址加警示色；运行中锁定
  新增的两个输入框。创建时勾选对外服务则端口必填（自动分配只支持默认监听地址）。
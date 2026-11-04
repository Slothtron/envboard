# envboard UI/UX 走查整改实施计划（2026-11）

本文件属于 `projects/envboard`。它把 2026-11-04 的桌面工作台静态走查结论转成
**可执行、可分批验证**的整改计划；走查本身是静态读码 + `shots/` 现有截图，
没有实机跑构建产物，因此每条改动都要配一条自动断言或一次视觉复核才算收口。

计划只动 `crates/desktop/src/views/` 与 `crates/desktop/tests/ui_l0.rs`
（外加 `CHANGELOG.md`），边界见 §5。

---

## 1. 问题台账与批次归属

| 编号 | 级别 | 一句话 | 位置（相对 `crates/desktop/src/`） | 批次 |
|---|---|---|---|---|
| F1 | P1 | 表头排序 ≤1s 被快照覆盖回账本序，箭头还说谎 | `views/environments.rs:195-200, 388-411` | 1 |
| F2 | P1 | 换环境后「规则」页签显示上个环境的正文或卡在 Idle | `views/detail.rs:118-125, 140-181, 441` | 1 |
| F3 | P1 | 删除选中环境后详情面板成关不掉的空壳 | `views/environments.rs:973-976, 1104-1137`；`views/detail.rs:198-209` | 1 |
| F4 | P2 | 端口提示写死 16000–16999，与 u16 校验/侧栏动态区间矛盾 | `views/detail.rs:854`；`app.rs:236-240` | 2 |
| F5 | P2 | 三个模态表单校验失败是「点了没反应」 | `views/catalog.rs:576-585, 1177-1194`；`views/ca.rs:518-525` | 2 |
| F6 | P2 | 配置表单错误条在底部、改值不清除、只能鼠标保存 | `views/detail.rs:557-585, 960-964, 1004-1006` | 2 |
| F7 | P2 | 调试切换目标静默清记录；会话失效后记录清不掉 | `views/debug.rs:900-946, 966-968, 1048-1060, 1395-1400` | 2 |
| F8 | P2 | 写操作成功零反馈（复制/CA 轮换却有 toast） | `views/environments.rs:916-919` 及各写回调 | 3 |
| F9 | P2 | 候选取数失败吞成空列表，把「读失败」伪装成「没有规则集」 | `views/detail.rs:613-645, 778-829` | 3 |
| F10 | P2 | `0.0.0.0` / 改端口这类暴露性变更无确认，与删除的纪律不一致 | `views/detail.rs:915-926, 851-854, 340-348` | 3 |
| F11 | P2 | CA 私钥驻留时长与屏上「关掉本页即清空」不符 | `views/ca.rs:388, 610`；`app.rs:120-127` | 3 |
| F12 | P3 | 截断 tooltip 只给「说明」列，其余列裁掉读不回 | `views/environments.rs:427-479`；`views/catalog.rs:204-209, 839-855` | 4 |
| F13 | P3 | 调试空态没有就地动作，与其余三屏不一致 | `views/debug.rs:470-502`；`views/mod.rs:65-101` | 4 |
| F14 | P3 | 同行在途反馈不一致：只有启停转圈 | `views/environments.rs:564-565, 591, 612` | 4 |
| F15 | P3 | 每秒无条件 `cx.notify()`，快照没变也全量重算 | `views/environments.rs:795-808` | 1（搭 F1） |
| F16 | P3 | 抓包详情正文/请求头不能整段复制、不能搜索 | `views/debug.rs:674-711, 731-736` | 4 |
| F17 | P3 | 打开详情后面板挤掉主表宽度，长文本列整段被裁 | `views/environments.rs:1104-1129`；`shots/walkthrough-env-detail.png` | 4 |
| F18 | P3 | `MAX_ROWS` 裁剪后 `selected_row()`（可见下标）错位、选中行静默换人 | `views/debug.rs:293-314, 386-388, 1475-1484` | 4 |

**已剔除的误报**：走查报告里「详情 Sheet 无 Esc 关闭绑定」不成立 ——
`views/detail.rs` 模块头注释明确 `Sheet` 组件自带「焦点陷阱 + Esc 关闭」，
不要再为它加绑定。F16 只保留「正文/头不可复制、不可搜索、base64 只给一句说明」。

---

## 2. 前置与基线（动手前必须先处理）

### 2.1 工具链与验证入口

- Rust **1.98.0**，由 `rust-toolchain.toml` 钉死；`mise.toml` 只管 python，
  不存在「mise 激活 rust」这一步。
- 快回路：`cargo test -p envboard-desktop`（含 `tests/ui_l0.rs` 的 L0 无窗口断言）。
- 门禁：`python scripts/verify.py all` = policy + rust + contract，其中 rust 层依次跑
  `cargo fmt --all --check` / `clippy --workspace --all-targets --offline -- -D warnings` /
  `check --workspace --all-targets` / `build --workspace` / `test --workspace`。
- L0 断言写进 `crates/desktop/tests/ui_l0.rs`，沿用现有 `install_globals()`
  （`FakeEngine` + `port_range 17100–17199`）与 `StateDir` 临时目录守卫；
  不要新建测试专用视图。
- 视觉复核：`cargo run -p envboard-desktop --example screenshot`，
  产物对照 `shots/`。锁屏时剪贴板与 `CopyFromScreen` 验收会失效（已知坑），
  重出截图须在未锁屏时做。

### 2.2 ⚠️ 基线未固化（阻塞项，需用户裁决）

`git status --short` 当前有 **64 项**：53 个已跟踪文件改动（+9825/−4669，
涵盖 v4 前端工程化、Admin API、MITM 白名单、CA 自有身份等）与 11 个未跟踪文件；
分支 `master`，最近提交 `4c4c257`。也就是说**本计划的每一行都叠在这批未提交改动之上**。

因此动手前必须先定基线处理方式，二选一：

- **A（推荐）**：把现有改动固化成一个 commit，再开 `fix/ui-ux-review-2026-11`
  分支开工；每批一个 commit。好处是任一批出问题可单独回退，且不牵连 v4 那批改动。
- **B**：不提交、不切分支，全程只用 `Edit` 逐处改，另存一份
  `git diff > <临时路径>` 作为人工快照。好处是不动 git 历史，代价是没有回退粒度。

**约束**：未获明确授权前，不执行 `git add` / `commit` / `checkout` / `reset` / `branch`，
也不做批量回退。全程逐文件 `Edit`。

---

## 3. 分批实施

顺序按「数据可信度 → 静默失败 → 反馈一致性 → 打磨」。每批独立可验证，
不把两批的改动混在一次验证里。

### 批次 1（P1 清零 + F15，最高优先）

**1.1 F1 排序持久化与 `changed` 判定** — `views/environments.rs`

- `EnvTable` 加排序状态字段。动手前先确认 `ColumnSort`（来自 `gpui_kit::component::table`）
  是否 `Clone + Copy + PartialEq`；若不可用，退化成 `sort_col: Option<usize>` + `sort_asc: bool`。
- `perform_sort`（:388-411）把 `(col_ix, 方向)` 存进字段，抽一个 `apply_saved_sort()`。
- `apply`（:195-211）在 `self.rows = rows` **之后**调 `apply_saved_sort()` 再
  `rebuild_visible()`。
- **同一处根因**：`let changed = self.rows != rows` 是 Vec 顺序比较，排过序后
  与账本序永远不等，导致每帧 `refresh`。改成「先把新帧按保存的键排好，再比较」，
  或改用内容无关顺序的判据（集合/键值比较）。
- **F15 搭车**：`attach` 的轮询回执（:795-808）把 `apply` 的 `changed` 透传出来，
  仅在变化时 `touch_detail` + `cx.notify()`；`tier_counts()`/`rebuild_visible()`
  不再每秒无条件跑。`app.rs:139-145` 已经对根视图这么做，环境屏自己要对齐。
- 断言：`environments.rs` 单测补「`apply` 后顺序保持」；`ui_l0.rs` 补
  「点表头 → 推进一帧快照 → 行序不变」。

**1.2 F2 规则页签串数据** — `views/detail.rs`

- `rename`（:118-125）：`self.rules_task = None`（`hop` 返回的 `Task` drop 即取消在途读盘）、
  `self.rules = Idle`；若当前 `tab == Rules`，立即重新 `load_rules`。
  需要 `window` → 改签名，`views/environments.rs:1112` 的调用点同步。
- `load_rules` 回执（:166-181）带 env 名校验：把 `name` clone 进回调，
  `if this.name != name { return; }` 再写 `this.rules`。
- 断言：覆盖「A→B 后 `rules` 不是 A 的正文」与「Idle 时能懒加载」。

**1.3 F3 删除后关闭面板** — `views/environments.rs`

- `remove` 回调（:962-980）里，若面板正指向该 name，`window.close_sheet(cx)`
  并 `self.detail = None`；回调已持有 `window`。
- 核对 `close_sheet` 之后复用逻辑（:1110-1120）不会因实体销毁出问题；
  全仓目前只有 `views/debug.rs:1013` 调过 `close_sheet`，这是第二处。
- 断言：`ui_l0.rs` 断言删除后 `detail-gone` 锚点不再被画出。

**批次验收**：`cargo test -p envboard-desktop` 全绿 + 3 条新 L0 断言；
`python scripts/verify.py rust` 全绿。

---

### 批次 2（静默失败清零）

**2.1 F4 端口提示动态化** — `views/detail.rs:854`
从 `cx.global::<Host>().port_range()` 取（`render_body` 有 `cx`），
文案改为「自动分配区间 {a}–{b}；显式端口 1–65535 均可。停机生效。」，
与 `app.rs:236-240` 同源；不要复制 `16000–16999` 字面量。

**2.2 F5 弹窗内错误行共享化** — `views/mod.rs` + 三处调用点
`views/environments.rs:993-1057` 的 `Rc<RefCell<Option<SharedString>>>` + 弹窗内
`create-error` 行已经验证过（注释写明「写到表格上方错误条会被模态挡住」）。
把它抽成共享 helper（问题状态 + 渲染函数 + `window.refresh()` 的设置函数），
三处 `on_ok` 照抄这套：

| 调用点 | 拒绝原因文案 |
|---|---|
| `catalog.rs:576-585` 导入规则集 | 名字形状 / 正文为空分别说明 |
| `catalog.rs:1177-1194` 新建编辑代理 | 名字·地址·端口缺失 / 端口非 u16 / 凭据半填 |
| `ca.rs:518-525` 按域名签发 | 域名为空 |

注意「凭据半填」目前 `build_proxy_body`（catalog.rs:1289-1294）是**静默丢掉两栏**
再交给服务端裁决，弹窗侧要补一句「用户名与密码必须同时填写」，
否则用户以为只填了用户名也提交成功了。

**2.3 F6 配置表单** — `views/detail.rs`
- 输入订阅（:557-585）里清 `self.message`（改值即撤旧错误）。
- `error_banner`（:960-964）移到表单顶部，别留在最底部隔着整棵表单。
- 键盘：表单根 `on_key_down` 绑 **Ctrl+Enter = save**；Esc 先不做
  （Textarea 换行、状态栏「回根焦点」、搜索框 `clean_on_escape` 三层语义叠加，
  单独评估后再定），本批只保证「不必切鼠标也能提交」。
- 断言：`dirty`/`message` 的状态迁移可观察（现有 L0 框架能断言）。

**2.4 F7 调试会话边界** — `views/debug.rs`
- 切换目标：`start`（:900-921）前若 `table.delegate().total() > 0`，
  提示「切换会清空当前 N 条记录」；最小实现是 `adopt` 成功后给一条说明性 toast，
  确认弹窗为可选加强。
- 会话失效后（:1048-1060）保留的记录要能清掉：`clear`（:966-968）去掉 `target` 前置，
  `debug-clear` 按钮（:1395-1400）改 `disabled(!has_records)`；
  同时给「停止」一个可见出口，别让 `session_id = None` 把控制区藏掉。
- 断言：`RecordsTable` 单测补「无 target 也能 clear」。

**批次验收**：`python scripts/verify.py all` 全绿；重出 `shots/walkthrough-*`
中受影响的屏（配置页 / 调试空态 / 三个弹窗）。

---

### 批次 3（反馈与信任一致性）

**3.1 F8 写操作成功反馈** — `views/errors.rs` + `views/environments.rs`
复用 `Action::headline`（`errors.rs:57-89`）那套动词表加一个
`notice_success(action, subject)` helper，避免各调用点自己拼中文漂移。
在 `dispatch` 回调与各写回调（`create` / `remove` / `start` / `stop` / `restart`）
成功分支 `push_notification(Notification::success(...).id1::<Self>(...))`；
失败仍走 `report()` 错误条。需要给 `environments.rs` 加
`notification::Notification` import。

**3.2 F9 候选项失败显式化** — `views/detail.rs:613-645`
`load_options` 保留 `Result`，`ConfigForm` 加 `options_error: Option<Notice>` +
「重试」按钮；加载中给 `picker` 一个禁用/loading 态；`picker`（:778-804）
把当前绑定值补进候选列表（否则菜单里缺当前项，读失败看着像绑定丢了）。

**3.3 F10 危险配置确认** — `views/detail.rs`
`save()`（:722-770）里比对 seed：`public_listen` 由关→开、或端口变化，
保存前弹一次确认，文案点明影响面（局域网可直连 / 代理地址变化需重新复制
`proxy_command`）。需要在 `ConfigForm` 里留 seed 快照（目前只在 `new` 拿到 `env`）。

**3.4 F11 CA 私钥驻留** — `views/ca.rs`
**推荐改文案 + 加动作，不改跨屏通信**：把 :388 的「关掉本页即清空」改成
「应用退出前保留在内存中，便于再次复制」，并给「按域名签发」结果区加一个
「清除签发结果」按钮（清 `self.issued`）。理由：`app.rs:120-127` 明确各屏
常驻、切屏不重建，要做到「关页即清」得让 `Workbench` 通知 `CaScreen`，
为一句文案引入跨屏状态流不划算。
另把 `issue()`（:529-560）「证书 + 私钥一起进剪贴板」与屏上两个「复制」按钮
的关系在文案里说清，避免用户以为只复制了证书。

**批次验收**：`verify.py all` + CA 屏视觉复核（宽/窄两种窗口，两列/单列各一次）。

---

### 批次 4（P3 打磨，按投入产出择项）

| 编号 | 改法 | 成本 |
|---|---|---|
| F14 | 重启/更多按钮统一 `.loading(pending)` | 低 |
| F13 | 调试空态改用 `empty_state_with_action`，放「选择环境」 | 低 |
| F12 | 所有可变长 `.truncate()` 单元格统一挂 `text_tooltip`，空值不挂 | 低 |
| F15 补充 | `tier_counts()` 缓存（若批次 1 只做了 notify 收敛） | 中 |
| F16 | 详情加「复制请求行 / 复制头 / 复制正文」+ 正文内查找；base64 给可复制的原始串 | 中 |
| F18 | 选中态改按 `request_id` 持有；补「自动跟随最新 / 暂停」开关 | 中 |
| F17 | 面板打开时给表格最小宽度 + 横向滚动，或临时隐藏次要列 | 中，需视觉定夺 |

F17 会改变 `overlay(false)` 的既有取舍（有意让主列表照常可点），
动手前先确认这不是回归；不做整屏重设计。

**批次验收**：全量 `verify.py all`、重出受影响截图、
`CHANGELOG.md` 的 `[Unreleased]` 按现有「Added / Changed + 理由」写法补条目
（现有段落本身就是范本）。

---

## 4. 验证矩阵

| 时机 | 命令 |
|---|---|
| 每改完一处 | `cargo test -p envboard-desktop` |
| 每批收口 | `python scripts/verify.py rust`（fmt / clippy -D warnings / check / build / test / policy） |
| 批次 2、3、4 收口 | `python scripts/verify.py all`（+ contract fixture） |
| 涉及版面 | `cargo run -p envboard-desktop --example screenshot` 重出 `shots/` |
| 最终 | `verify.py all` + 截图对照 + CHANGELOG 更新 |

新增断言一律落 `crates/desktop/tests/ui_l0.rs`（L0 无窗口）或视图模块内 `#[cfg(test)]`
（纯逻辑），不引入新的测试 target、不开 `gpui-kit/test-support`
（理由见 `ui_l0.rs` 文件头：它会牵出 `cocoa 0.26.0` 与已锁 0.26.1 冲突）。

---

## 5. 边界（明确不做）

- **只动界面**：不改 `crates/host` / `admin` / `engine` / `manager` / `protocol` / `events`。
  判定只在服务端做一次，界面只是它的翻译面；本轮没有 wire 变更，
  因此 `spec/` 契约与 `spec/fixtures/` 一律不动。
- **不改依赖**：不加 crate、不改 `Cargo.toml`；`gpui-kit` 唯一版本守卫与
  「产物依赖不许出现命令行解析库」两条门禁照旧。
- **不碰 `external/`**（第三方只读克隆）。
- **不处理 README 里 ⏳ 的 P2 剩余**（请求轨迹页 / HAR 导入会话 / 活动审计 /
  跨环境对比）：那是功能缺口，不是走查缺陷，另立计划。
- 不改根工作区 `AGENTS.md` / `README.md`（根级文件不索引项目文档）。

---

## 6. 风险与已知坑

| 风险 | 判据 / 绕法 |
|---|---|
| 64 项未提交改动 | §2.2；全程逐文件 `Edit`，禁止 `checkout` / `reset` / 批量回退 |
| `ColumnSort` 形态未知（定义在 gpui-kit 依赖里，源码中只有 import） | 批次 1.1 动手前先查依赖源码；必要时退化成两个基础类型字段 |
| `Task` drop 取消语义（F2 依赖它） | `host.rs:121` 的 `hop` 返回 gpui `Task`，drop 即取消；仍需在改动后验证旧回执不再落地 |
| `close_sheet` 与详情实体复用的配对（F3） | 全仓目前仅 `debug.rs:1013` 一处调用，第二处要实测 |
| Esc 键位叠加（F6） | 状态栏=回根焦点、搜索框=清空、Sheet=组件自带关闭；故本批只做 Ctrl+Enter，Esc 另议 |
| 锁屏下剪贴板/截图失效 | 视觉复核与复制类验收须在未锁屏时做 |
| 排序修复可能触到 `tier_counts()` 的行序假设 | 该判据只数行数、与顺序无关；仍补一条聚合单测兜底 |

---

## 7. 里程碑与待确认

| 里程碑 | 完成标志 |
|---|---|
| M1 | 批次 1 完成：3 条 P1 清零 + 3 条新 L0 断言 + F15 收敛 |
| M2 | 批次 2 完成：静默失败与文案误导清零 |
| M3 | 批次 3 完成：成功反馈、候选失败、危险确认、CA 文案一致 |
| M4 | 批次 4 择项完成 + `verify.py all` 全绿 + 截图重出 + CHANGELOG 更新 |

**动手前待用户裁决的两项**：

1. **基线处理走 A 还是 B**（§2.2，涉及 git 提交/分支，不代为决定）。
2. **F11 取「改文案 + 清除按钮」还是「真做到关页即清」**（推荐前者，理由见 3.4）。

其余各条按本计划默认方案执行。

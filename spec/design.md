# UI 设计规范 · 工作台设计语言

| 项 | 值 |
|---|---|
| 适用范围 | `frontend/`（工作台全部视图与组件）；未来桌面形态同规范 |
| 蓝本 | `examples/envboard-workbench-heroui/`（本规范由其 README + REVIEW_NOTES P1–P14 提炼转正） |
| 上位契约 | 机检纪律与令牌权威见 [ui.md](ui.md)；接口词汇见 [protocol.md](protocol.md)。冲突时以 ui.md 为准 |
| 执行 | 机检条目由 policy 门禁（`tests/policy-tests/tests/ui_style.rs`）判定；其余为评审档 + bsk 走查检查表（附录 A） |
| 版本 | v1.0（2026-09-21） |

本文件回答「工作台长什么样、为什么长成这样」。每条规则给出**可走查断言**，
bsk 验收轮逐条对照（附录 A）；机器可判的条目已登记进 ui.md 机检档。

## 1 Token 纪律（D-1）

- 颜色 / 圆角 / 组件外观 **100% 走 HeroUI v3 语义类 + Tailwind v4**：`bg-background`
  `text-foreground`、`bg-surface / bg-surface-secondary / border-border`、语义色
  `success / warning / danger / accent / muted` 及其 soft / -foreground / 透明度形态。
  **零硬编码色值**——`frontend/src/` 内不允许出现 hex / `rgb()` / `hsl()` 字面量（机检）。
- 暗色模式只靠 `<html class="dark" data-theme="dark">` 整体切换；
  **组件层禁止手写 `dark:` 覆盖**（机检）。
- 自定义样式唯一落点是 `src/globals.css` 的 `@layer components`，BEM 命名（`wb-*`、
  `btn-action--*`）；尺寸用 `clamp()` 表达，动效 200ms ease-out。JSX 不内联写死
  px / hex（机检禁 `style={{`）。
- 间距只用 Tailwind 4px 体系高频档：`gap-1/2/3/4`、卡 `p-3`、面板 `p-4`、页 `p-5`、
  工具条 `px-5 py-3`。

## 2 布局骨架（D-2）

- 三段式工作台骨架（**不与列表管理型骨架混用**，P6）：

  ```
  flex h-screen flex-col
  ├─ TopBar        border-b bg-surface px-5 py-3
  └─ flex min-h-0 flex-1
     ├─ SideBar    w-56 flex-none（纯导航 + 计数，不放业务列表）
     └─ main       min-h-0 min-w-0 flex-1 overflow-hidden
  ```

- 每视图工具条统一形态：`flex flex-wrap items-center gap-2 border-b border-border
  bg-surface px-5 py-3`，主操作在 `ml-auto`。
- 嵌套滚动统一 `min-h-0 flex-1 overflow-y-auto`，全工程同一写法。
- 窄视口策略是**横向滚动兜底，不是断点重排**：`Table.Content` 加 `min-w-240`
  （960px）转横滚，关键单元格 `whitespace-nowrap`，搜索框 `w-40 xl:w-56`，
  窄抽屉隐藏次要回显（`hidden 2xl:inline`）。
- 层级表达靠描边 + 表面色阶（`bg-surface` vs `bg-surface-secondary`）+ `ScrollShadow`；
  阴影不作装饰（对齐 ui.md UI-4 旧精神）。

## 3 字体与排版（D-3）

- **sans 给人读的话，`font-mono` 给机器值**：环境名、端口/监听地址、状态码、
  URL/路径、大小/耗时、计数、快捷键、代理命令。
- 字号刻度：`text-sm` 辅助/muted 说明；`text-base` 单元格正文与字段值、h2/h3；
  `text-lg` 品牌名/抽屉路径；`text-2xl font-bold` 抽屉主标题。
- 分区小标题：`text-sm font-semibold uppercase tracking-wide text-muted`
  （`SectionTitle`，分组先于平铺，P5）。数字列右对齐用 `text-end`。

## 4 按钮分级（D-4，P14）

按**后果是否可逆**分级，不按位置排布；除主操作外一律 `size="sm"`（紧凑工作台密度）：

| 档 | 形态 | 用途 | 配额 |
|---|---|---|---|
| primary | 实心 | 新建/导入/对比/开启抓包等创建类主操作 | **每视图工具条有且仅有一个**，在 `ml-auto` |
| secondary | 实心次级 | 次动作、激活的筛选态、保存 | 不限 |
| ghost | 无框 | 图标按钮、放弃、关闭、排序切换 | 不限 |
| outline + `.btn-action--warn` | 警示描边 | 可逆中断（停止/重启类） | — |
| outline + `.btn-action--danger` / 弹层内实心 `danger` | 危险 | 不可逆（删除）；页面内描边、AlertDialog 内转实心 | — |

图标按钮：`isIconOnly` + 中文 `aria-label` + 外层 `<Tooltip delay={200}>`。
行内操作**常驻可见，不依赖 hover**（P7，触屏/键盘可达）。

## 5 状态呈现分层（D-5）

- **Chip 变体分层是刻意的**：状态与计数用 `variant="soft"`（`color` + `size="sm"` +
  `Chip.Label`）；分类徽标（HTTP 方法）用 `variant="tertiary"` 描边——与状态码的
  soft 填充拉开层级，避免同色误读。
- 健康态用状态点：`size-2 rounded-full bg-success/warning/danger/muted`
  （顶栏指标、Chip 内、侧栏流状态三处同款）。
- 状态码着色：`≥500 danger / ≥400 warning / ≥300 accent / 其余 success`，mono 加粗。
- 同一信息不在一处重复堆叠（P13）；关联信息随行可见（P12：规则/代理列表行内给
  「被引用」环境 chips，删除前可见影响面）。

## 6 反馈契约（D-6）

- **无 Toast**（不在组件冻结清单）。瞬时反馈统一右下角固定
  `role="status" aria-live="polite"` 的 `FeedbackAlert`：**3.2s setTimeout 自动消失**、
  可手动关闭。status 只有三档：`success / warning / danger`；
  「命中提示」类（如规则改写命中）用 **warning 不用 success**——提示不是成功。
- 区域四态全部走 `shared.tsx` 统一组件，任何数据区域都必须覆盖：
  - loading：区域级 `TableSkeleton`，按钮级 `isPending`；
  - empty：`EmptyState{icon,title,hint,action}`——**必须给出路动作**（如「清除筛选」）；
  - error：`ErrorState`（Alert + 重试按钮，重试中 `isPending`）；
  - disabled：`isDisabled` + Tooltip 解释原因（如上游被引用不可删）。
- 复制类操作：按钮文案翻转「复制→已复制」+ 图标替换，1.6s 复原；**剪贴板失败也要
  反馈**，禁止静默失败。
- 脏状态：`InlineNotice status="warning"`「有未保存的改动」+ 放弃按钮。

## 7 抽屉与面板（D-7）

- 详情用**非模态停靠 `<aside>` 面板**，不用官方 `Drawer`/`Modal`：react-aria Modal
  打开会给背景 `inert` + 遮罩，与「面板开着仍能单击列表切换内容」直接冲突（实测）。
  刻意偏离官方组件必须在代码注释写明理由与实测证据。
- 约定：BEM 类控宽（抓包 `clamp(420px,36vw,640px)`、环境 `--env` 加宽
  `clamp(520px,48vw,860px)`）；200ms ease-out 滑入（`wb-drawer-in`）；
  打开后单击另一行仅切换内容不关闭；**被选中项删除时自动收起**；
  `aria-hidden` 表达隐藏态。
- Esc 关闭挂 **window 捕获阶段**（react-aria 组件在冒泡阶段 `stopPropagation`），
  且当页存在 `[role="dialog"], [role="alertdialog"]` 时让位给模态，不误关抽屉。
- 抓包详情面板上下分栏（请求信息 / 响应信息两 `<section aria-label>`，各带 Tabs），
  详情由记录**派生**（`buildCaptureDetail` 模式），点不同行确实看到不同内容。

## 8 导航与筛选（D-8）

- 侧栏纯导航：分组「资源 / 观测 / 系统」+ 计数徽标 + `aria-current` 标记当前视图；
  无路由库，根组件 `useState<ViewKey>` 条件渲染。
- **筛选单一入口**（P3）：工具条常驻计数 chips + 搜索框，不允许三套筛选并存；
  顶栏指标与列表 chips **共享同一 `metric` 状态**——点顶栏即筛列表，反馈就地（P2：
  首屏给操作对象，不给装饰性指标）。
- chips 激活态：`aria-pressed` + `variant="secondary"`（不占用主操作配额）；
  排序按钮循环式（name→port→status）。
- 搜索匹配多字段（name/port/rules）小写包含；空结果 EmptyState 带「清除筛选」。
- 详情页签带计数徽标（P4），异常计数转危险色；页签嵌套上限**两层**。
- 渐进披露（P8）：次要/高级字段收进 `Disclosure` 折叠，右侧标注条目数与性质
  （「3 项 · 安全相关」）。

## 9 表单与确认（D-9）

- 字段三件套：`TextField > Label + Input + 帮助文本 span`；`isInvalid` 驱动校验态，
  帮助文本随校验切换 `text-danger` / `text-muted`；实时正则（如
  `/^[a-z][a-z0-9_-]*$/`）、成对字段约束（鉴权用户名/密码同填同禁字符）、
  校验失败禁用保存。
- 字段标注**「停机生效 / 热生效」**语义（与引擎热装配契约对齐）。
- 表单宽度 `max-w-3xl`；选择类用 `Select`（复合 slot 形态），多选用 `Checkbox` 组。
- 不可逆动作一律 `ConfirmDialog`（AlertDialog）：正文附「此操作不可撤销。」；
  保持官方默认 `isKeyboardDismissDisabled=true`（**ESC 不关，须显式选择**）；
  确认按钮 `onPress` 里先动作后 `close()`。

## 10 数据层与状态（D-10）

- 组件全受控：根组件（`Workbench`）持有全局状态，以 props + `on*` 回调下发视图；
  展示层禁止持有业务数据。
- API 客户端按 Admin API 契约 1:1 手写（`src/api/`，契约文件见 protocol.md 及其后续演进），类型镜像 protocol DTO；
  后端已回显的派生值（如 `proxy_command`）不在前端重算。
- 快照驱动视图用 `generation` 短路重渲；跨视图复用的原子 UI 集中 `shared.tsx`；
  图标统一 `icons.tsx`（内联 SVG，`currentColor`、`strokeWidth 1.7`、viewBox 24）。

## 11 可达性（D-11）

- 中文 `aria-label` 普遍；筛选组 `role="group"` + `aria-label`；激活态 `aria-pressed`；
  抽屉/面板 `<section aria-label>`。
- 操作不依赖 hover（D-4）；键盘路径完整：Esc 契约（D-7）、Tab 顺序跟随视觉顺序、
  快捷键用 `Kbd` 标注。
- 对比度底线沿用 ui.md 评审档：正文对表面 ≥ 4.5:1（WCAG AA）。

## 12 流程规范（D-12）

- 每个非显然决策记录三列：**现在的问题 → 重构后 → 规范依据**（REVIEW_NOTES 表
  模式）；明确拒绝项也要留档（REVIEW_REJECTED：多层嵌套页签、触控级控件尺寸）。
- 刻意偏离官方组件必须写明理由与实测证据（代码注释级）。
- 交付前运行时验证：`tsc --noEmit` strict 0 错、交互断言、**零 `console.error` /
  零 CSP 报错**、多视口（1560/1280/1024）走查截图；UI 每轮变更后跑 bsk 验收
  （检查表见附录 A）。

---

## 附录 A · bsk 走查检查表

验收轮对 `frontend`（dev 5199 / 内嵌 8900）逐条执行；每条 = 一个可观察断言。
截图不入库，归档到本机验收材料目录：文件名含轮次与视图名，暗色模式加 `-dark` 后缀。

| # | 检查 | 方法 |
|---|---|---|
| CK-01 | 首屏三段式骨架正确，侧栏 7 视图带计数与 `aria-current` | observe + 截图 |
| CK-02 | 每视图工具条有且仅有一个 primary 按钮且在右侧 | observe 计数 |
| CK-03 | 顶栏指标点击 → 列表出现激活 chip（secondary + aria-pressed）且结果被筛 | click + observe |
| CK-04 | 搜索无结果 → EmptyState 含「清除筛选」，点击恢复全量 | fill + observe + click |
| CK-05 | 环境行「查看详情」→ 右抽屉滑出；单击另一行内容切换、抽屉不关 | click ×2 + observe |
| CK-06 | 抽屉开着按 Esc 收起；AlertDialog 开着按 Esc **不关抽屉也不关模态** | press Escape |
| CK-07 | 删除环境 → ConfirmDialog 含「不可撤销」文案；确认后行消失、打开中的抽屉自动收起 | click + observe |
| CK-08 | 新建表单填非法名 → isInvalid 红字帮助文本、保存禁用；合法后恢复 | fill + observe |
| CK-09 | 保存成功 → 右下 `role="status"` Alert 出现且 3.2s 后消失 | observe 两次（间隔 >3.2s） |
| CK-10 | 复制代理命令 → 按钮文案变「已复制」 | click + observe |
| CK-11 | 抓包行双击 → 停靠面板打开、上下分栏各带 Tabs；方法徽标 tertiary、状态码着色 | 双击 + observe |
| CK-12 | 暗色切换后整体反色正常，无组件残留亮色块 | evaluate 切 class + 截图 |
| CK-13 | 窄视口（1024）表格转横滚，无列内竖排挤压 | emulate + 截图 |
| CK-14 | 全程 `bsk console` 零 error、零 CSP 违规；SSE 断线时侧栏指示转 warning 态 | console 检查 |
| CK-15 | 行内操作按钮常驻可见（不 hover 也在 DOM 且可点） | observe |
| CK-16 | 图标按钮均有中文 aria-label + Tooltip 内容 | observe/snapshot |
| CK-17 | `data-*` 锚点在位（view 切换、环境行、健康态），live 断言可定位 | get-html 抽查 |

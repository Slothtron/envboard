# 桌面形态与 web 形态的行为对账（desktop parity）

本文件是 **web 形态下线（P1）的前置证据**：逐屏记录"web 有什么、桌面有什么、差在哪、
差的那部分怎么处置"。判定口径是**行为等价**，不是像素一致 —— 桌面用 GPUI 现成组件，
web 用 HeroUI，视觉语汇本就不同（决策见 docs/adr-001-gpui-desktop-form.md）。

范围只到 P0 已落地的四屏。P2 的屏（抓包/调试、活动、设置、跨环境对比）在最后一节
单独列出，它们与 P1 的删除清单一起决定"下线时哪些能力会暂时没有界面"。

## 怎么验的（以及为什么不是全自动）

本机 agent 会话被 OS 拒了鼠标注入（`SetCursorPos 拒绝访问 0x80070005`），框架的无头
UI 测试又依赖 `gpui-kit/test-support` —— 该 feature 在 0.8.0 上不可解析。所以证据是
三件拼起来的，缺一件都不算过：

1. **纯逻辑单测**：展示层派生（`crates/desktop/src/display.rs`，逐字移植自
   `frontend/src/data.ts`）、请求体组装、名字形状规则；
2. **实体级测试**：视图构造与"侧栏每一屏都真的落地"这类不变量；
3. **`PrintWindow(PW_RENDERFULLCONTENT)` 截图**：真实渲染出的画面，人眼对账。
   颜色数与均值亮度做黑帧/空白自检（一次成功抓取 90+ 色）。

弹窗与二屏列表的截图靠一个**临时探针**（`ENVBOARD_PROBE_SCREEN` /
`ENVBOARD_PROBE_DIALOG` 环境变量决定首屏与自动开哪个弹窗）拍到 —— 探针**不入库**，
最终提交的二进制只跑默认路径。走查正是靠它抓到两个"不点就看不见"的缺陷（见下）。

## 逐屏对账

### 环境列表（`frontend/src/views/EnvironmentsView.tsx` ↔ `crates/desktop/src/views/environments.rs`）

| 能力 | web | 桌面 | 判定 |
|---|---|---|---|
| 全量快照刷新 | `/api/events` 1s 推 | tokio 1s 轮询 + `mpsc(1)` 丢帧 | 等价（那三条流本来就是轮询器） |
| 搜索 | SearchField，覆盖名称/说明/规则/上游 | `InputState` + `set_query`，同一覆盖面 | 等价 |
| 状态过滤 | 全部/运行中/已停止/需处理 chips（带计数） | 同四枚 `Button.ghost().selected()`，计数与表同源 | 等价 |
| 排序 | 表头点击 | `Column::sortable()`，组件自带箭头 | 等价 |
| 启停/重启 | 行内图标钮 | 同；在途行转圈并禁用 | 等价 |
| 删除 | 二次确认弹窗 | `open_alert_dialog().confirm()` + Danger 变体 | 等价 |
| 新建 | 表单弹窗 | 同（名字形状先拦，裁决仍在服务端） | 等价 |
| 行选中开详情 | 抽屉 | `TableEvent::SelectRow` → `Sheet` 面板 | 等价（面板形态不同，见下） |
| 空态/加载态 | EmptyState / TableSkeleton | `DataTable` 自带 | 等价 |

### 环境详情（`EnvironmentDetail.tsx` ↔ `crates/desktop/src/views/detail.rs`）

三页签（概览 / 配置 / 规则）用 `TabBar`，面板本体是 `Sheet`（可拖宽、非遮罩、Esc 关闭）。

| 能力 | web | 桌面 | 判定 |
|---|---|---|---|
| 概览字段 | 键值行 | `DescriptionList::horizontal()` | 等价 |
| 凭据回显 | 只给布尔（代理鉴权已配置/未配置） | 同 —— 服务端从不回显，桌面也无从自算 | 等价（硬纪律） |
| 配置字段 | 说明/端口/规则集/上游/放宽校验/对外/抓包/访问鉴权 | 同集合，逐项标注"热生效"还是"停机生效" | 等价 |
| 脏状态 | 逐字段比较 | "任一输入动过"的粗粒度 | **弱于 web**：够用（放弃=按当前环境重建实体），但字段级 diff 没做 |
| 规则集/上游选择 | Select | `Button::dropdown_menu` + `PopupMenuItem::checked()` | 等价（组件库的 Select 面向集合搜索，这里给的是"可清空的单选"，菜单更贴） |
| 规则正文 | 只读文本区 | 页签内按行渲染（等宽），未绑定/缺失/失败三态分开说 | 等价 |
| 代理命令 | 服务端回显 `proxy_command` | 同，桌面不重算 | 等价（硬纪律） |

### 规则集目录（`CatalogViews.tsx::RulesView` ↔ `crates/desktop/src/views/catalog.rs`）

| 能力 | web | 桌面 | 判定 |
|---|---|---|---|
| 列表 | 行式卡片 | `DataTable` 四列：规则集 / 生效条数 / 被哪些环境引用 / 操作 | 等价，且多给"生效条数"（同源于环境快照，不另算） |
| 反向引用 | 环境名 chips | 同数据，逗号串 | 等价（呈现密度不同） |
| 搜索 | 有 | 有（`set_query`，纯函数过滤） | 等价 |
| 查看原文 | 链接到 `/api/rules/<name>`（浏览器开） | 只读弹窗（`AdminService::rules_read`） | 等价 —— 桌面没有浏览器，链接必须换形态 |
| 导入 | 名字 + 正文（rows=6） | 同；正文 `Textarea.h(140px)` | 等价 |
| 删除 | 二次确认，绑定它的环境标记「规则缺失」 | 同，且确认文案把**正在引用的环境名**列出来 | 等价偏强 |
| 计数 | 标题旁 chip | 表上方一行「N 个规则集」，过滤后给「x / y · 无匹配项」 | 等价 |

### 上游代理目录（`CatalogViews.tsx::ProxiesView` ↔ 同文件 `ProxiesCatalog`）

| 能力 | web | 桌面 | 判定 |
|---|---|---|---|
| 列表 | 名称 + `host:port · 鉴权` 两行 | 五列：代理 / 地址 / 鉴权 / 被引用 / 操作 | 等价 |
| 新建/编辑 | 同一表单弹窗，编辑预填 host/port，凭据留空 | 同；`has_auth` 时按钮文案改为「保存（将清除鉴权）」 | 等价偏强 —— 留空的后果写在按钮上，不靠用户读注释 |
| 删除 | 被引用时禁用并说明原因 | 同（`.disabled(referenced)` + tooltip） | 等价 |
| 凭据 | 只进不出；提交时缺项就不发字段 | 同（`build_proxy_body`：`port` 发数字，`user`/`password` 成对或不出现） | 等价 |
| 密码输入 | `<Input type="password">` | `InputState::masked(true)` | 等价（打码同时把值挡在剪贴板选择之外） |

## 已知不等价（下线前必须知道的事）

1. **反馈通道不同**：web 每个写操作有 toast（`onFeedback`），桌面把失败写进表上方的
   错误条、成功靠下一次取数自己画出来。行为不缺，**即时感**弱一档。
2. **列宽不会拉伸填满**：`DataTable` 的 `Column` 只有 `width`/`min_width`，没有
   stretch 开关，所以列宽按默认窗口宽度预算，窗口拉得比默认宽时右侧会留白。
3. **远程访问消失**：web 形态的 HTTP+SSE 面（含 token 门禁、二维码配对用的 CA 下载）
   在桌面形态没有对应物。这是方案里明确接受的损失，不是遗漏。
4. **P2 屏还没有界面**：抓包/调试、活动流、设置（CA 与二维码）、跨环境对比。
   它们在 web 形态都有界面，所以**在 P1 删除清单执行前**必须逐条决定：
   要么先补到桌面，要么承认一段无界面窗口期。

## 走查抓到的缺陷（都已修，记在这里防复发）

* **普通 `Dialog` 不画按钮** —— `gpui-component` 的 `Dialog` 只在 `AlertDialog` 里
  给默认页脚；普通 `Dialog` 的 `button_props`/`on_ok` 只装回调，页脚必须自己用
  `DialogFooter` + `DialogClose` + `DialogAction` 组。缺页脚的弹窗只有右上角一个 ✕，
  **用户根本无法提交**。P0.4 的「新建环境」弹窗带着这个缺陷入库了两轮，
  本轮（P0.8）四张弹窗截图才把它抓出来。判据已写进 `crates/desktop/src/lib.rs`。
* **`Context::on_next_frame` 里再 `update` 同一个实体会 panic**
  （`cannot update X while it is already being updated`）—— 闭包第一个参数就是
  `&mut 本实体`，要动自己就用那个接收者。
* **`proxy_put` 请求体形状**：`port` 必须是 JSON 数字，`user`/`password` 空串会被
  服务端判成非法凭据（要么成对给、要么都不给）。第一版把两者都发成空字符串，
  等于"新建一个不配鉴权的代理"都会失败。

## 证据

截图（本机 release，`PrintWindow`，1493×959 @125% DPI，非黑帧色数 122–269）：
`target/tmp/p03-demo/shots/` 下 `p08-environments-final.png`（最终入库二进制）、
`p08-rules.png`、`p08-proxies.png`、`p08-create.png`、`p08-import.png`、
`p08-text.png`、`p08-put.png`、`p08-edit.png`（后四张与两张目录屏来自含探针的
同源码构建）。种子数据：三个环境（`beta` 绑 `corp` + `up-lab`）、规则集 `corp`
2 条、上游代理 `up-lab`。

门禁：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、
`cargo test --workspace --no-fail-fast`（仅 `crates/engine/tests/backend.rs` 2 项与
`data_plane.rs` 11 项因本机拒绝访问当前用户证书存储而失败，HEAD 同样失败）、
`python scripts/verify.py policy`、`cargo build --release --offline --locked`。

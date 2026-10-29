# 桌面形态与 web 形态的行为对账（desktop parity）

本文件是 **web 形态下线（P1）的前置证据**：逐屏记录"web 有什么、桌面有什么、差在哪、
差的那部分怎么处置"。判定口径是**行为等价**，不是像素一致 —— 桌面用 GPUI 现成组件，
web 用 HeroUI，视觉语汇本就不同（决策见 docs/adr-001-gpui-desktop-form.md）。

范围只到已落地的六屏（环境列表、环境详情、规则集目录、上游代理目录、调试、设置）。
P2 还欠的屏（活动审计、跨环境对比、HAR 导入会话）在最后一节单独列出。

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

### 调试（`DebugView.tsx` ↔ `crates/desktop/src/views/debug.rs`）

| 能力 | web | 桌面 | 判定 |
|---|---|---|---|
| 目标选择 | 调试视图里选环境 | 侧栏「调试」→「选择环境」：运行中的可选，未运行的列出但禁用（写明"未运行"） | 等价 |
| 会话单例 | 切换即停旧 + 清旧 + 开新 | 同（门面保证，界面不假装能同时看两个） | 等价 |
| 实时列表 | `/api/debug/stream` SSE 500ms | 500ms `debug_delta` 增量，按 `request_id` 续游标（清空时归零） | 等价 |
| 记录列 | 时间 / 方法 / 主机 / 路径 / 状态 / 大小 | 同六列；**时间标 UTC** | 等价偏弱（web 用本机时区，桌面没引时区库） |
| 详情 | 请求 / 响应两栏（起始行 + 头 + 正文） | `Sheet` + 请求/响应两页签 | 等价（形态不同） |
| 二进制正文 | 展示 base64 | 只标"二进制 + 大小"，暂不还原 | **弱于 web** |
| 导出 HAR / JSONL | 抓包会话导出两条端点 | 未做（随 HAR 导入会话一起属于后续） | **缺** |
| 清空 / 停止 | 有 | 有（停止 = 停记录 + 清记录，与门面一致） | 等价 |

### 设置（`SettingsView.tsx` ↔ `crates/desktop/src/views/settings.rs`）

| 能力 | web | 桌面 | 判定 |
|---|---|---|---|
| CA 摘要 | `GET /api/ca` | 读组合根交付的 `CaBundle.info`，`DescriptionList` 展示（类型/主体/组织/版本/序列号/生效/到期/签名算法/公钥/SAN/指纹） | 等价 |
| 证书取用 | 下载 pem + 二维码（手机扫码安装） | 「复制路径」「复制 PEM」两个剪贴板动作 | **弱**：没有二维码（`qrcode` 随 web 出库），手机装证书要自己把文件带过去 |
| 主题 | 全局一个明暗开关 | **明亮 / 暗色 / 跟随系统** 三档下拉框（`Select`），写入 `<state_dir>/ui.json`，系统切明暗时"跟随系统"实时生效 | 等价偏强（多一档、能持久化） |
| 关于 | 版本 + 能力矩阵 | 版本 / 引擎 / 状态目录 / 端口区间 / 界面偏好路径 / 装配警告 | 等价 |

## 已知不等价（下线前必须知道的事）
1. **反馈通道不同**：web 每个写操作有 toast（`onFeedback`），桌面把失败写进表上方的
   错误条、成功靠下一次取数自己画出来。行为不缺，**即时感**弱一档。
2. **列宽不会拉伸填满**：`DataTable` 的 `Column` 只有 `width`/`min_width`，没有
   stretch 开关，所以列宽按默认窗口宽度预算，窗口拉得比默认宽时右侧会留白。
3. **远程访问与无头常驻消失**：web 形态的 HTTP+SSE 面（含 token 门禁、二维码配对用的
   CA 下载）在桌面形态没有对应物，`scripts/systemd/envboard.service` 那种"没有交互
   会话也在跑"的用法一并退场。这是方案里明确接受的损失，不是遗漏
   （docs/adr-003-retire-web-frontend.md「接受的损失」）。
4. **还有三块没界面**：活动审计、跨环境对比、HAR 导入会话 —— 引擎与管理器侧能力
   都在，界面还没补（P2 剩余）。调试屏的"导出 HAR / JSONL"也一并欠着。
5. **调试屏的本机验证边界**：这台机器上引擎起不了实例 —— 共享 CA 物化要读系统信任库，
   而当前用户证书存储被拒（`拒绝访问 (os error 5)`，与 `crates/engine/tests/` 里
   那 13 条失败同源，`reconcile: cannot start beta` 就是它）。所以调试屏的列表与详情
   是用**合成记录**（一次性探针，提交前已删）验证渲染的，真实抓包链路未在本机跑通。
   设置屏不受影响：CA 摘要是真实数据（演示状态目录里那张 mitmproxy 形态的共享 CA）。

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
同源码构建）；P2 新增 `p2-debug.png`（记录表）、`p2-debug-sheet.png`（请求详情
Sheet + 页签 + 正文）、`p2-env-light.png`（标题栏已无主题开关）与设置屏三张：
`p2-theme-select.png`（主题下拉框收起态）、`p2-theme-select-open.png`（展开、勾选
当前档）、`p2-theme-select-dark.png`（选「暗色」后即时生效并写入 `ui.json`）。
后两张是靠「向窗口 PostMessage 点击」真正操作出来的，不是摆拍。改控件之前的
单选框旧图（`p2-settings-*.png`）已作废。
种子数据：三个环境（`beta` 绑 `corp` + `up-lab`）、规则集 `corp` 2 条、
上游代理 `up-lab`、共享 CA 一张 mitmproxy 形态的根证书。

门禁：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、
`cargo test --workspace --no-fail-fast`（仅 `crates/engine/tests/backend.rs` 2 项与
`data_plane.rs` 11 项因本机拒绝访问当前用户证书存储而失败，HEAD 同样失败）、
`python scripts/verify.py policy`、`cargo build --release --offline --locked`。

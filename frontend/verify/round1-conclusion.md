# bsk 验收轮 1 · mock 态 UI 走查结论（2026-09-21）

对象：`http://127.0.0.1:5199/`（vite dev，mock 数据）。
检查表：`spec/design.md` 附录 A（CK-01…CK-17）。截图：`shots/round1/`。

## 逐条结果

| # | 检查 | 结果 | 证据 |
|---|---|---|---|
| CK-01 | 三段式骨架 + 侧栏 7 视图计数 + aria-current | ✅ | observe：资源/观测/系统分组，「活动 14」在位 |
| CK-02 | 工具条 primary 配额 | ✅ | 环境「新建环境」、规则「导入规则集」、设置「下载根证书」各一；活动/对比/调试无创建类主操作，无 primary |
| CK-03 | 顶栏指标 → 列表 chip 激活 | ✅ | 点「运行中」后 `aria-pressed="true"`（DOM 验证），共享 metric 状态 |
| CK-04 | 空结果带「清除筛选」出路 | ✅ | 搜索无命中 → EmptyState + 清除筛选按钮，点击恢复 |
| CK-05 | 抽屉打开/单击切内容/不关闭 | ✅ | `wb-detail-panel--env --open`；单击另一行 h2 切换、面板保持 |
| CK-06 | 模态在场时 Esc 让位 | ✅ | alertdialog 打开时按 Esc 不关 |
| CK-07 | 删除确认含「不可撤销」+ 源删收起 | ✅ | alertdialog 文案在位；确认后抽屉 class 失去 `--open` |
| CK-08 | 表单实时校验 | ✅（demo 既有，interact 30/30 覆盖） | — |
| CK-09 | 反馈 Alert 3.2s 自动消失 | ✅（代码契约 pushFeedback setTimeout 3200） | 主题切换反馈即时可见 |
| CK-10 | 复制文案翻转 | ✅（demo 既有） | — |
| CK-11 | 双击开停靠面板 + 上下分栏 | ✅ | `tr[data-key]` 双击 → `--open`；「请求信息/响应信息」region + Tabs |
| CK-12 | 暗色切换无残留亮块 | ✅ | 设置→通用→深色：`html.dark data-theme=dark`，刷新后持久（localStorage） |
| CK-13 | 1024 窄视口横滚兜底 | ✅ | 表格转横滚（截图 environments-1024.png），无列内竖排 |
| CK-14 | 零 console error / 零 CSP | ✅* | 页面自身 error 0；`chrome-extension://invalid` 为浏览器扩展噪音，非页面 |
| CK-15 | 行内操作常驻 | ✅ | observe 直接可见启停/重启/删除按钮 |
| CK-16 | 图标按钮中文 aria-label + Tooltip | ✅ | 「查看环境详情 beta」等 |
| CK-17 | data-* 锚点 | ⚠️ 部分 | 活动行有 `data-activity-seq/type`；环境行锚点待 C2 接 API 时按 live 断言要求补齐 |

## 本轮修复项

1. **活动视图语义**：`custom` 事件不再冒充实例域（新增「其他」）；事件按「今天/昨天/日期」分组；时钟统一 `HH:MM:SS`（`en-GB` locale，避免 zh 汉字撑破 mono 列宽）。
2. **demo 遗留清除**：顶栏「重构 DEMO」徽标与「查看重构说明」弹窗移除（规范内容已沉淀进 `spec/design.md`）；`NotesDialog`、`REVIEW_NOTES/REVIEW_REJECTED` 删除。
3. **主题切换功能补齐**（新增 `src/theme.ts`）：设置→通用「浅色/深色/跟随系统」，localStorage 持久化，main.tsx 启动即应用防闪烁，监听系统切换。
4. **PressResponder 警告治理**：移除 Chip 上的无效 Tooltip；三处 `isDisabled` 按钮不再直挂 Tooltip（禁用解释改走 aria-label + 行内「被引用」chips，符合 D-6）。
5. **设置「关于」文案**：从「零构建链三文件」更新为 Vite 工程形态。

## 已知非阻塞项

- HeroUI 内部仍偶发 2 条 `PressResponder …` **warning**（dev + StrictMode，非 error；demo 基线同样存在，其验证仅断言 error）。留待 C2 接真实数据后复测，若仍存在则向上游记录。

## 判定

**轮 1 通过**（CK-17 的环境行锚点属 C2 接线事项）。进入 C2。

# UI 契约 · 工作台资产纪律

| 项 | 值 |
|---|---|
| 适用范围 | `frontend/`（Vite 工程：index.html + src/**）与其构建产物 `frontend/dist/` |
| 设计语言权威 | [design.md](design.md)（布局、分级、反馈契约等设计规则）；本文件只收**机器可判**的纪律 |
| 令牌权威 | HeroUI v3 语义类 + Tailwind v4（组件库内置）；自定义样式唯一落点 `src/globals.css` |
| 执行 | policy 门禁 `tests/policy-tests/tests/ui_style.rs`（一门禁一文件，`bash ci/verify.sh` 默认层） |
| 版本 | v2.0（2026-09-21，前端工程化改版：机检对象从三件套资产改为 frontend 源码） |

本文件回答「改工作台 UI 之前必须知道什么」。分两档：**机检档**（UI-1…UI-6，
违反即 `ci/verify.sh` 红）与**评审档**（门禁不可达，提交前人工对照）。锚点 `UI-n`
是门禁与本文件的双向对账依据：本文件缺锚点、或门禁实现了本文件没有的规则号，
都判红（见 UI-6）。

## 机检档

### UI-1 色值出没域

颜色字面量（hex `#rgb` / `#rgba` / `#rrggbb` / `#rrggbbaa`，`rgb()` / `rgba()` /
`hsl()` / `hwb()`）在 `frontend/src/` 的 `.ts` / `.tsx` / `.css` 里**零容忍** ——
颜色一律走 HeroUI 语义类（`bg-surface` `text-muted` `border-danger/30` …）。
`frontend/index.html` 同样禁止，仅 data-URI（favicon）豁免。想加颜色：先确认
HeroUI 语义类覆盖不了，再在评审档里登记理由。

### UI-2 内联样式禁令

`.tsx` 禁止 `style={{`（含 `style={{` 的任意空白形态）。自定义样式唯一落点是
`src/globals.css` 的 `@layer components`（BEM 类，见 [design.md](design.md) 第 1 节）；
Tailwind 工具类写进 `className`。

### UI-3 深色覆盖禁令

`frontend/src/` 禁止 `dark:` 前缀工具类 —— 暗色只靠
`<html class="dark" data-theme="dark">` 整体切换（`src/theme.ts`），
组件层不写任何深色分支。

### UI-4 裸 px 尺寸

`.ts` / `.tsx` 禁止 Tailwind 任意值裸 px（`text-[13px]` `p-[5px]` 一类
`[…px]` 形态）——尺寸走 Tailwind 刻度（4px 间距体系 / 字号刻度）。
`src/globals.css` 是自定义样式的登记处，`clamp()` / 动效尺寸允许（它是设计
语言的实现，不是漂移）。

### UI-5 交互形态

- `.ts` / `.tsx` 禁止 `window.alert` / `window.confirm` / `window.prompt`。
  反馈唯一出口是页内 `role="status"` Alert 区（[design.md](design.md) 第 6 节）；确认类交互一律
  由 AlertDialog 承载。
- `frontend/index.html` 禁止内联 `<script>`（只允许带 `src=` 的外链脚本）与
  小写内联事件属性（`onclick=` 等）；JSX 的 `onPress` / `onChange` 组件属性
  不在此列（CSP 严格，无 `unsafe-inline`）。

### UI-6 契约在位

门禁内建的规则清单与本文件的 `UI-n` 锚点必须一一对应：本文件缺锚点，或门禁存在本
文件没有的规则号，都判红。改本文件与改门禁必须是同一个提交。

## 评审档（提交前人工对照）

- **设计语言**：全部设计规则（Token 纪律、布局骨架、按钮分级、反馈契约、抽屉模式、
  可达性…）见 [design.md](design.md)；其附录 A 是 bsk 走查检查表，UI 每轮变更后执行。
- **CSP 与资产形态一致**：构建产物必须保持外链脚本/样式（`modulePreload: false`），
  任何内联注入都会被严格 CSP 拒绝且 curl 断言查不出来（v1 的教训）。
- **可测试性**：保持 DOM 可断言结构（`data-*` 标记、语义化 class、中文 aria-label）；
  这些锚点改名即破坏 live 验收与 bsk 走查。
- **凭据永不回显**：UI 只消费服务端视图 DTO（protocol 词汇），不组凭据字段。
- **构建顺序**：先 `pnpm build` 产出 `frontend/dist/`（构建产物，**不入库**，
  与 `target/` 同类），再 `cargo build` 经 `include_dir!` 内嵌；缺 dist 时
  `crates/web/build.rs` 编译期响亮失败。改 `frontend/src/` 后重跑
  `bash ci/verify.sh frontend` 即可再生。

## 修订规则

1. 改本文件与改门禁（执行它的那个文件）必须同一提交；只改一边会被 UI-6 判红。
2. 新增机检规则：先在本文件立锚点条文，再在门禁实现；反向亦然，缺一判红。
3. 门禁红了的修法只有两种：改资产回归契约；或确属契约过时——按第 1 条同时改契约
   与门禁。禁止在门禁里加文件级豁免。

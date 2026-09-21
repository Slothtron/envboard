# bsk 验收轮 3 · 单二进制内嵌形态结论（2026-09-21）

对象：**release 二进制**（`include_dir!` 内嵌 frontend/dist）实跑
`127.0.0.1:8902`（独立临时 state，种 dev 环境 + beta 规则），零 Node、零 CDN。
截图：`shots/round3/`。

## 走查结果

| 检查 | 结果 | 证据 |
|---|---|---|
| 内嵌资产 serve | ✅ | `/` 200（no-store）、`/assets/index-*.js/.css` 200 + immutable + 正确 MIME |
| 首屏真实渲染 | ✅ | dev 环境行（运行中、16089、beta 1 条）、core 0.3.0、事件流已连接 |
| SSE 快照流 | ✅ | 真实数据驱动，顶栏/列表计数一致 |
| 抽屉 + 真实 proxy_command | ✅ | `wb-detail-panel--env --open`，命令为服务端回显的 16089 |
| 删除确认 | ✅ | alertdialog +「此操作不可撤销。」 |
| console 页面 error | ✅（修复后） | 见下 |

## 本轮抓到并修复的真 bug：严格 CSP 与 react-aria 运行时注入冲突

- **现象**：内嵌形态下 console 报 1 条 CSP 违规 —— react-aria pressable 首挂载
  注入固定内容 `<style>`（`[data-react-aria-pressable]{touch-action:…}`，88 B），
  被 `style-src 'self'` 拦。**dev 模式（vite 无 CSP）暴露不了此问题**——轮 3 的增量价值。
- **走过的弯路**（留档）：空 `<style id>` 占位 + 内容并入外链 css —— 空 style 标签
  本身即报违规（空串 hash），且 Chrome UA 的 `touch-action: manipulation` 本就压过
  author 层规则（react-aria 原注入亦无效），功能无损但报告不干净。
- **终态修法**：`CONTENT_SECURITY_POLICY` 的 style-src 加该固定内容的
  `sha256-38Rh…` hash（`<style>` 元素 hash 豁免不需要 unsafe-inline/unsafe-hashes），
  注释钉死「react-aria 升级改内容 → live 层零 CSP 报错断言会红」。

## 判定

**轮 3 通过**。三轮 bsk 验收 + 全层 verify（默认 policy/rust/contract/artifact +
frontend drift + live 34+3）全绿，升级完成。

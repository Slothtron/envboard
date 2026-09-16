# 编辑已有环境 · 实机验收证据

> 记录时间：2026-09-15 / 16。被测二进制：`target/debug/envboard`（`--core fake` 与
> `--core mitmproxy 12.2.3` 两种），浏览器：用户真实 Edge 153（`bsk` Agent Window）。
> 起因是一个真实使用反馈：**建环境时没绑规则，事后改不出来**。
>
> **2026-09-16 更新**：界面升级为明亮主题后，本页「打开工作台」一行的计算样式断言已
> 随令牌更新（`--bg` 从深色 `#0f1115` 改为明亮 `#f4f6fa`），并在真实实例上用无头 Edge
> 153（CDP）复验：`envboardReady === "yes"`、控制台零 CSP 报错与零异常。下方逐步编辑
> 流程的原始记录保留；其服务端对应行为由 `scripts/verify_live_v2.py` 的第 10 组断言
> 在每次实机层运行时重验。

---

## 0. 起点：写通道本来就存在，只是没有入口

先实测服务端（`PATCH /api/environments/:name`，此前无任何调用方）：

```
$ curl -X PATCH -H 'content-type: application/json' -H 'x-envboard-request: 1' \
    -d '{"description":"灰度环境 v2","rules":"beta"}' \
    http://127.0.0.1:8911/api/environments/beta
{"description":"灰度环境 v2","desired":"stopped","health":"stopped",
 "listen":{"host":"127.0.0.1","port":16827},"name":"beta","rules":"beta","rules_count":1}
```

改名与换端口（停止状态）同样正常；运行中改名按契约返回 `conflict` 并说明原因。
结论：**不是接口坏，是工作台没有入口**（操作列只有 启动/停止/重启/日志/删除），
CLI 也只有 `add`。用户那份 `rules/beta.rules` 已经导入（10844 字节），
而环境的 `rules` 字段是 `null` —— 规则文件建好了，却绑不上去。

## 1. 工作台：真浏览器的一次完整编辑（`--core fake`）

状态复刻用户现场：`beta`（16850，无规则绑定）+ 已导入的规则文件 `beta`（2 条）。

| 步骤 | 断言 | 结果 |
|---|---|---|
| 打开工作台 | `documentElement.dataset.envboardReady === "yes"`（JS 真的跑了）；`getComputedStyle(body).backgroundColor === rgb(244, 246, 250)`（样式真的生效，明亮令牌 `--bg`）；控制台无 CSP 报错 | PASS |
| 点行内「编辑」 | 面板切到「编辑环境」、按钮变「保存」、出现「取消编辑」、顶部出现 `beta` 徽章；四个字段被填成当前值（名字 `beta`、端口 `16850`、描述 `测试环境`、规则为空） | PASS |
| 选规则 `beta` + 改描述 → 保存 | 表格规则列从 `—` 变成 `beta (2)`；提示「已保存。」；表单回到新建模式 | PASS |
| 启动实例 → 再点「编辑」 | 名字/端口/规则三个输入框 `disabled === true`，描述可编辑；提示写明"实例在运行，只能改描述" | PASS |
| 运行中只改描述 → 保存 | 提示「已保存。」，状态仍是 `running`（**热改不重启**） | PASS |
| 表单开着的时候把环境停掉 | 三个输入框自动解锁（表单跟随 SSE 快照自愈），提示改回"改完点保存" | PASS |
| 改名 `beta → gamma` + 端口 `16855` → 保存 | 表格变 `gamma / 16855`，**规则绑定跟着保留**（`beta (2)`，验证契约里"解绑要显式 `null`"没被误触发）；代理命令行同步换端口；提示「已保存。」且**没有**错误 toast | PASS |
| 规则库「载入」 | 取回规范化原文（226 字符，带生成器注释头）并填进表单，提示写明字符数 | PASS |
| 追加一行 → 「导入」 | 表格规则列变 `beta (3)`；磁盘文件 `# entries: 3`；表单内容保留（便于连续改） | PASS |

截图（真实渲染，未入库）：编辑态的锁定形态、最终列表形态两份。

**关于"怎么点的"**：`bsk click` 与 `bsk press` 在这个环境里**到达不了** Agent Window
（本轮再次确认：`click ok` 但页面无反应、无 toast、状态不变），而 `bsk fill` / `bsk select`
正常。因此表单填写用 `bsk fill/select`（真实 DOM 输入路径），提交与行内按钮用
"在真实元素上派发 `click`"（`element.click()`）—— 它走的就是页面自己的
`addEventListener("click")` 处理器，与用户点击同一条代码路径，只是少了操作系统输入注入。
**这一条不影响前端三条断言**（JS 执行 / 样式生效 / 无 CSP 报错），它们全部通过。

## 2. 真 mitmdump 的端到端（`verify_live_v2.py` check 10）

```
PASS  10 编辑：运行中换绑定被拒、停止后可补绑规则并改名换端口、新配置真的生效
      before='<html>...502 Bad Gateway...</html>' bind_running=409 desc_running=200
      edit=200 after='alpha-upstream' old_name=404 port=16514
```

判别性来自"改前是 502、改后是上游名字"：这条 URL（`gamma.test`）在绑定之前**不通**，
补绑规则并改名换端口、重新启动之后**真的通到了上游**。若只是列表好看而实例没按新配置跑，
这里会是 502 而不是 `alpha-upstream`。

live 层 17/17 全绿（含既有的规则内容热重载 check 2：换绑定要停机，但**内容**是热重载的，
这是两件事）。

## 3. 编辑路径上发现并修掉的问题

1. **运行中换绑定会被静默接受**（原 `update()` 的注释声称绑定是热更新）。
   被一条单元测试逮到：断言热改绑定后仍 `running`，实际得到 `config_mismatch` ——
   注入器在启动时用 `--set envboard_rules=<path>` 固定了路径，换绑定它看不见。
   现在运行中拒绝（`conflict`），描述仍可热改。
2. **旧失败标记不作废**：`port_conflict` 记的是"这个端口被占"，用户把端口换到空位上以后
   标记还在，界面会拿**新**端口号报冲突（在说谎）。现在改 `listen`/`rules` 即作废旧标记。
3. **改名之后日志面板指向旧名字**：面板目标不跟着改 → 之后每秒一次的尾部读取都是 404 →
   那个 404 让整次 `refresh()` 失败 → **"已保存。"被顶成一条红字**（浏览器验收踩到）。
   现在面板目标跟着列表自愈，日志读取失败只落在日志面板里。
4. **名字输入框的 `pattern` 被浏览器静默忽略**：Chrome 用 `v` 标志校验 `pattern`，
   `[a-z][a-z0-9_-]*` 在 `v` 下非法（裸 `-`），于是整条 pattern 不生效、控制台只留一行
   `SyntaxError`。转义 `\-` 后验证：`9bad` → 无效、`beta2` → 有效（`checkValidity()`）。
5. **CLI 的本地 API 发现会跨状态目录**：显式 `--state-dir` 时仍兜底到默认端口，
   本机实测 `--state-dir /tmp/eb-edit` 的 `env add` 打到了 8900 上正在运行的工作台。
   现在显式状态目录只认自己的 `api.json`。

## 4. 本轮未能验证的

- **规则文件内容热重载的真实时序**由 live 层 check 2 覆盖（导入后不重启即生效）；
  本轮浏览器验收用的是 `--core fake`，它不读规则，所以"界面里改规则 → 实例行为变化"
  这一段在实机层验证，不在浏览器层。
- 改名**不搬迁**日志文件（新名字从新文件开始写），已写进 README 的已知限制与
  `core/spec/capabilities.md` 的 `instance.logs`，但"用户是否更希望日志跟着改名走"
  是个产品取舍，未做。

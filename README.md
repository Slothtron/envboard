# envboard

mitmproxy 多环境 DNS 工作台：**按环境指定 DNS 服务器**，把同一个域名解析成
**各环境各自真实的 IP**，在自定义 Dashboard 里**管理 / 切换 / 编辑环境**（beta / gray / prod …）。

> 范围：**只做正向解析（host → ip）**，不做 ip → host 反查。

> 自研插件包。上游 `opensource/mitmproxy/` 仅作参考，**从未修改**。

## 命名模型

**`slothtron` 是发包标识，只出现在 manifest 里，不出现在任何目录名或文件名。**

| 层 | 值 | 出现 `slothtron`？ |
|---|---|---|
| PyPI 分发名 | `slothtron-envboard` | ✅ **仅此一处**（`pyproject.toml` 的 `name`） |
| 导入包名 / 目录 | `envboard`（`src/envboard/`） | ❌ |
| 项目目录 | `envboard/` | ❌ |
| addon 名 | `envboard` | ❌ |
| 选项前缀 | `envboard_*` | ❌ |
| 命令前缀 | `envboard.*` | ❌ |
| Dashboard 路由 | `/envboard` | ❌ |
| npm 私有根（不发布） | `envboard` | ❌ |

分发名与导入名不同是标准做法（`beautifulsoup4` / `bs4`、`pillow` / `PIL`）：
**发布身份属于 manifest，代码身份属于代码。** 安装时用的是分发名，导入时用的是
导入名，两者互不影响：

```bash
pip install slothtron-envboard     # 分发名
python -c "import envboard"        # 导入名
```

## 目录结构

本包是**纯 Python 包**，用 **src 布局**：导入包放在 `src/` 下，与仓库根其余部分分开。

```
envboard/
├── core/spec/            # ★ 语言中立契约：能力清单、错误码、13 个 golden fixture
├── src/
│   └── envboard/         # ★ 导入包（wheel 的 packages 就声明 "src/envboard"）
│       ├── core/         #   纯逻辑层，仅标准库：model/registry/mapping/resolver/ports/errors
│       ├── infra/        #   端口的具体实现：DNS（mitmproxy_rs）、状态文件、时钟
│       ├── adapter/      #   宿主接线：mitmproxy addon + mitmweb tornado 路由
│       └── web/          #   Dashboard 资产（零构建、零依赖）
│           ├── index.html
│           └── app.js    #   ← 必须是外部同源文件，不能内联（见下）
├── addons/envboard.py    # mitmproxy `-s` 加载入口（把仓库 src/ 加入 sys.path）
├── tests/                # 单元测试
├── scripts/              # 各项门禁脚本（compile / dependency / naming / contract / pack）
├── ci/verify.sh          # 单一验证入口，也被 `npm run verify` 调用
├── docs/acceptance/      # 非设计类：实机验收证据
├── examples/             # hosts 输入示例（真实的 hosts.txt 已 gitignore）
└── pyproject.toml  package.json  README.md  CHANGELOG.md  LICENSE  .gitignore
```

三条结构性约束，都由门禁脚本强制（不靠人工检查）：

- **Dashboard 的 JS 不许内联**：mitmweb 对 Web 界面下发的 CSP 是
  `default-src 'self'; connect-src 'self' ws:; img-src 'self' data:; style-src 'self' 'unsafe-inline'`
  —— 没给 `script-src` 单独开口，于是回落到 `'self'`，**内联 `<script>` 会被浏览器直接拒绝执行**。
  故障现象极具欺骗性：HTML 照常 200、页面照常渲染，只是一行 JS 都不跑（`active: —`、
  环境列表空白）。`curl` 查不出来，只有真浏览器会暴露。所以 JS 放外部同源文件
  `web/app.js`，由 `AssetHandler` 提供；内联 `<style>` 不受影响（CSP 显式放行了
  `style-src 'unsafe-inline'`）。这条**由 `pack_check.py` + `verify_live.sh` 双重把关**。
- **依赖单向**：`core` 只依赖标准库；`infra` 可用第三方运行时库（`mitmproxy_rs`）；
  `adapter` 才允许碰宿主（`mitmproxy` / `tornado`）。反向 import 一律失败
  （`scripts/dependency_lint.py`）。`infra` 这一层的存在，正是为了让 `core` 保持纯标准库。
- **包位置**：导入包必须声明为 `src/<name>`，且只能有一个（`scripts/naming_lint.py`）。
- **`src/envboard/` 里的 `envboard` 不是冗余，是导入包名本体，删不得。**
  各层用的是跨层相对 import（`from ..core.errors import X`），这要求 `core` / `infra` /
  `adapter` 有**共同父包**——那个父包只能是 `envboard`。若把各层直接摊在 `src/` 下，
  它们在源码树里会变成互不相干的顶层包，`..core` 立刻 `ImportError: attempted relative
  import beyond top-level package`（已实测）。收益看起来是少一层，代价是源码树不可导入：
  `-s addons/envboard.py`、`ci/verify.sh`、单测、`pip install -e .` 全部失效，代码只能从
  装好的 wheel 跑。

## 能力清单

| 能力 | 说明 |
|---|---|
| 多环境注册表 | 环境 = 名称 + DNS 服务器列表 + 域名后缀 + 静态 hosts 覆盖 + 颜色/说明；CRUD 与激活切换，落盘到 `<confdir>/envboard.json`（0600） |
| 动态读取 DNS 服务器 | 三级来源：① 环境自身的 `dns_servers` ② 运行时经 Dashboard 编辑 ③ `mitmproxy_rs.dns.get_system_dns_servers()` 读操作系统配置 |
| 正向解析 (host→ip) | 复用 `mitmproxy_rs.dns.DnsResolver`（Rust / hickory），**每环境一个解析器** |
| 多环境对比解析 | 一次请求把同一批域名分别向**每套环境**的 DNS 各查一次，直接看差异 |
| 被动采集 | `--mode dns` 时从 `dns_response` 钩子直接读取客户端真实解析结果 |
| 映射索引 | 按环境分片（`env + host` → `{ip}`），带 TTL、来源标记与优先级 |
| flow 注解 | 命中映射的请求自动打标：`flow.comment`（mitmweb 可见）和/或 `flow.metadata` |
| Dashboard | 挂载在 mitmweb 上（默认 `/envboard`），继承其鉴权与 XSRF，无独立端口 |
| 规则文件 | 把杂乱的 hosts 风格文件（`ip host…` 或 `host… ip`，多 host 共用 ip，非法行忽略）规范化成确定性规则文件；**按环境绑定**，切环境即切规则文件 |
| CLI/控制台命令 | 16 条 `envboard.*` 命令，与 Dashboard 能力对等 |

## 支持矩阵

| 宿主 | Dashboard | 命令 | hook | 状态 |
|---|---|---|---|---|
| `mitmweb` | ✅ 挂载在 `/envboard` | ✅ | ✅ | 主路径 |
| `mitmdump` | ❌（无 tornado 应用） | ✅ | ✅ | 支持；`--mode dns` 时被动采集生效 |
| `mitmproxy`（console） | ❌ | ✅ | ✅ | 支持 |

> Dashboard 是**加速器不是前提**：没有它，`envboard.*` 命令仍能完成全部操作。

## 安装

本插件是 mitmproxy addon，**不需要单独安装**即可用 `-s` 加载：

```bash
git clone <this-repo> && cd envboard

# 方式一：直接用 -s（自动把仓库 src/ 加入 sys.path）
mitmweb  -s addons/envboard.py
mitmdump -s addons/envboard.py --mode dns

# 方式二：安装后按模块名加载
pip install -e .
mitmweb -s "$(python -c 'import envboard,os;print(os.path.dirname(envboard.__file__))')/adapter/addon.py"
```

打开 mitmweb 控制台打印的地址，追加 `/envboard/`：

```
Web server listening at http://127.0.0.1:8081/?token=...
Dashboard:                          http://127.0.0.1:8081/envboard/
```

## 配置项与默认值

| 选项 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `envboard_enabled` | bool | `true` | 总开关 |
| `envboard_config` | str | `""` | 状态文件路径；空 = `<confdir>/envboard.json` |
| `envboard_web_prefix` | str | `/envboard` | Dashboard 挂载前缀（仅 mitmweb） |
| `envboard_annotate_flow` | bool | `true` | 是否给命中的 flow 打环境标签 |
| `envboard_annotate_field` | str | `comment` | `comment` \| `metadata` \| `both` |
| `envboard_passive_capture` | bool | `true` | 是否从 DNS 流量被动采集映射 |
| `envboard_dns_timeout` | int | `5` | DNS 查询超时（秒） |
| `envboard_cache_ttl` | int | `300` | 映射默认 TTL（秒） |
| `envboard_max_mappings` | int | `50000` | 映射条目上限，超出淘汰最旧 |
| `envboard_watch_hosts` | str[] | `[]` | 随激活环境自动解析的域名清单 |
| `envboard_refresh_interval` | int | `0` | 自动刷新间隔（秒），`0` = 关闭 |
| `envboard_rules_dir` | str | `""` | 规则文件目录；空 = `<confdir>/rules` |

所有选项都是 mitmproxy 原生类型（`bool` / `int` / `str` / `Sequence[str]`）。
**注意**：mitmproxy 的选项解析器不支持 `float`，因此所有数值选项都是 `int`。

## 快速验证

```bash
# 全量门禁：语法 + 依赖方向 + 单测 + 契约 golden + 类型 + 打包白名单 + 冒烟
bash ci/verify.sh          # 或 npm run verify

# 单独跑
bash ci/verify.sh unit
bash ci/verify.sh contract
```

端到端（需要 mitmproxy 与网络）：

```bash
mitmweb -s addons/envboard.py --set web_open_browser=false &
curl -s 'http://127.0.0.1:8081/envboard/api/environments?token='"$(cat ~/.mitmproxy/envboard.token 2>/dev/null)"''
```

## 规则文件：从 hosts 文件到可切换的静态规则

一份手写的 hosts 风格文件 → 规范化规则文件 → **按环境绑定**生效。

```bash
# 命令行导入（非法行自动忽略，会回报 accepted/skipped/conflicts）
mitmproxy 里执行： envboard.rules.import /path/to/hosts.txt [规则名]
# 绑定到环境（切环境即切规则文件；留空解绑）
                   envboard.rules.bind prod hosts
                   envboard.rules.list / envboard.rules.show hosts / envboard.rules.remove hosts
```

Dashboard 的「规则文件」面板可以做同样的事：粘贴文本或填服务器上的文件路径导入，
下拉框绑定到当前环境，导入报告会把**被忽略的行**和**冲突**逐条列出来。

### 输入有多宽容

| 写法 | 处理 |
|---|---|
| `10.0.0.1 api.example.com auth.example.com` | 多个 host 共用同一个 ip |
| `beta.example.com 10.0.0.2` | 反序写法，与上面等价 |
| 空行 / `#` 注释（整行或行尾）/ BOM / CRLF / 行首缩进 | 忽略 |
| 一行里没有合法 IP，或只有一个 token | 整行忽略，记 `no-ip-literal` / `too-few-tokens` |
| 某个 host 不合法 | **只丢那一项**，同一行其余 host 保留 |
| 同一 host 映射到不同 ip | **后出现者胜**，并记进 `conflicts` |

非法内容**永远不会让整次导入失败** —— hosts 文件是人手写的，一个笔误不该让其余几百条
一起报废；但每一处忽略都会带行号和原因报出来，不静默吞掉。

### 输出是确定的

同一份输入 + 同一 source 渲染出的字节完全相同（ip 数值序、host 字典序、每个 ip 一行），
因此重新导入同一份文件不产生 diff，规则文件本身可以进版本管理。

```bash
$ envboard.rules.import examples/hosts.sample.txt demo
imported rules 'demo' from examples/hosts.sample.txt
  accepted=7 host(s) over 5 ip | skipped=5 | conflicts=1
  written to ~/.mitmproxy/rules/demo.rules
```

### 生效优先级

```
环境 inline hosts   >   绑定到该环境的规则文件   >   DNS 解析
      └─────────── 两者都是 static 源：优先于 DNS，且永不过期 ───────────┘
```

环境里的条目是"这个环境特有的例外"，比共用的规则文件更具体，所以它赢。
解析结果里的 `static_from` 会标明每条到底来自 `environment` 还是 `rules:<名字>`。

**被环境绑定的规则文件禁止删除**（返回 `conflict`），否则那个环境会静默失去覆盖 ——
必须先 `envboard.rules.bind <env> ""` 解绑。

## 环境切换到底切什么

MVP 只实现 **L1 观测**：切换环境改变的是「如何解读流量」（用哪套 DNS 解析、
flow 归属哪个环境），**不改写任何流量**。请求重定向（L2）需要预热缓存配合
`server_connect` 这个 blocking hook，风险更高，本版明确不做。

## 文档

- 契约（裁定依据）：`core/spec/capabilities.md`、`core/spec/errors.md`、`core/spec/fixtures/`
- 实机验收证据：`docs/acceptance/`
- 变更记录：`CHANGELOG.md`

## 许可

MIT。`opensource/mitmproxy/` 为第三方项目，遵循其自身许可，未做任何修改。

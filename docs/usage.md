# 使用

面向使用者的操作说明：能力清单、安装、接入、工作台各屏、环境编辑与状态目录。
项目概览见 [README](../README.md)；构建与发布见 [building.md](building.md)；
内部结构见 [architecture.md](architecture.md)。

## 能力一览

| 方向 | 能力 |
| --- | --- |
| **环境与端口** | 环境 CRUD / 启停 / 显式重分配端口；端口从 `16000-16999` 随机试绑，新建冲突自动重试一次；**绑定即真相**（`EADDRINUSE` → `port_conflict`，不偷偷换端口） |
| **热应用** | 写操作返回后的第一个请求就是新配置：`ArcSwap` 换快照，无轮询、无收敛窗口；运行中的请求按进入时的旧快照跑完 |
| **规则（hosts）** | 账本 `state.db` 是唯一真相，物化文件可再生；导入同名覆盖即对绑定且在跑的环境逐个热应用；脏规则 fail-closed 502 |
| **TLS 策略** | `insecure_hosts` 精确域名放宽上游校验（无通配符、无子域继承）；`mitm_hosts` 白名单只解密点名的域名，其余盲隧道 |
| **上游代理** | 按名管理的代理账本 + 环境按名绑定（热字段）；TLS 目标走 CONNECT 隧道，明文 http 按 absolute-URI 转发；删除被引用的代理会被拒绝并点名 |
| **代理鉴权** | `proxy_user` / `proxy_password` 同生共死 → 407 门；凭据只在内存与 0600 账本，不进 argv / 视图 / 日志 |
| **共享 CA** | 所有实例一张 `envboard` CA（`shared/confdir/`）；MITM 按 SNI 现签；轮换下一次 CONNECT 即生效；不复用同目录的 `mitmproxy-ca*.pem` |
| **账本与自愈** | SQLite + WAL 单事务原子写；引擎线程 panic → `failed` → 按期望状态自动重启（任务级崩溃隔离，release 保持 unwind） |
| **抓包与 HAR** | 环境子页的单例抓包会话、500ms 增量推送、方法/状态/主机筛选、请求响应详情、导出 HAR；HAR 导入只经 IPC 命令暴露，界面不提供入口 |
| **桌面工作台** | 两视图（环境 / 证书）、列表搜索排序过滤、双击行开悬浮详情（概览/配置/规则）、顶栏指标聚合 |
| **后台运行 / 托盘** | 关窗 = 销毁窗口与 WebView、**进程与引擎线程常驻**（端口继续服务）；托盘左键/菜单重建窗口；窗口位置/大小/最大化跨关窗与重启记忆；boot 自检屏只在进程首窗播放 |
| **证书页** | 摘要/指纹/路径、单行已安装状态、**安装 / 卸载直接作用于当前用户 ROOT 存储**（免管理员、可逆；不提供下载与系统向导） |
| **设置** | 主题三档（浅色/深色/系统）+ 运行时信息（版本/端口区间/状态目录/事件丢弃） |

**不提供的能力**（写明的取舍，不是待办）：

| 能力 | 状态 |
| --- | --- |
| 跨环境对比 / 活动审计页 | ❌ 未提供 |
| 远程访问 / 脚本化管理面（HTTP API） | ❌ 管理面只经本机 IPC |
| HTTP/2 客户端（gRPC 等） | ❌ 数据面只实现 HTTP/1.1 |
| PAC / 透明代理 / SOCKS | ❌ 未提供 |
| 开机自启 | ❌ 未提供 |


## 安装

从 [Releases](https://github.com/Slothtron/envboard/releases) 下载 Windows 构建：

- `envboard.exe`：免安装，双击即用（需要 WebView2，Windows 10/11 通常已带）。
- `*_x64-setup.exe`：NSIS 安装包（perMachine，装到 `C:\Program Files\envboard`，需要一次 UAC）。

未签名：首次运行会撞 SmartScreen，点「更多信息 → 仍要运行」。

### 从源码构建

前置：

- Windows 10/11 + **WebView2** 运行时（系统通常已带 Evergreen 运行时）。
- **Rust 1.98.0**（`rust-toolchain.toml` 钉死）、**Node 24.16 / pnpm 10.18.2**
  （`mise.toml` 钉死；`mise install` 一把拿齐）、**tauri-cli**
  （`cargo install tauri-cli --version "^2.11"`）。
- 构建衔接由 `crates/app/desktop/build.rs` 持有：`cargo build` 时自动跑
  `pnpm install + build`，前端不变则不重跑 —— 没有需要人记着的构建顺序。

```bash
# 开发：Tauri 壳 + vite HMR（devUrl 127.0.0.1:5199 只是开发服务器，数据面走 IPC）
mise run dev

# 发布构建（在 crates/app/desktop 下）
cargo tauri build              # target/release/envboard.exe + NSIS 安装包
cargo tauri build --no-bundle  # 只出 exe
```

离线口径：`Cargo.lock` 入库，cargo registry 与 pnpm store 已暖；fresh clone 的首次
构建两端都要网。纯离线复构建可用 `ENVBOARD_SKIP_FRONTEND_BUILD=1` 消费既有 `dist/`。

前端单独类型检查 / 构建（日常不需要，build.rs 会驱动）：

```bash
mise run typecheck    # tsc --noEmit
mise run build        # pnpm -C frontend build
```

### 三步入网

1. **起工作台**：双击 `envboard.exe`（无控制台黑窗、无监听端口）。状态目录用平台
   默认值，冲突时弹框响亮失败；换目录见[配置](#配置)。
2. **建环境**：环境页「规则集」页签导入规则 → 回「实例」新建环境 → 在详情抽屉的
   「配置」页签绑定规则（绑定是热字段，不用重启环境）。
3. **客户端接入**：每个环境详情里都有可复制的那一行。

```bash
export https_proxy=http://127.0.0.1:16301 http_proxy=http://127.0.0.1:16301
curl https://api.example.com/
```

### 便携形态

只发 exe 就是合法分发形态：免安装、不写注册表。想让"数据跟着 exe 走"，在 exe 同目录
放一个**非空**的 `envboard.portable` 文本文件：数据落在 exe 旁边的 `envboard-data/`，
整个目录可整体搬走，删目录 = 卸载；一个副本只能起一个实例。

Releases 里的 `envboard-portable.zip`（预览渠道是 `envboard-preview-portable.zip`）就是
这个形态：解压即用，exe 与标记已经放好。

便携副本同时也是一种**独立形态**（`portable`）：WebView2 用户数据目录、窗口/托盘标题
与默认端口段都和安装版分开，所以「装好的正式版 + 一个便携副本」可以同时开着互不影响。
把多份 exe 各放一个目录、各带一个标记，就是多套互不干扰的环境（本地开发常用这个）。

## 工作台

- **两视图**：环境、证书。环境列表列精简为 环境·状态·监听·规则·操作；顶栏聚合
  全部 / 运行中 / 已停止 / 需处理。
- **悬浮详情**：双击整行打开抽屉（概览 / 配置 / 规则三页签），absolute 悬浮层，
  不挤占列表宽度。
- **抓包 = 环境子页**：实例行 icon 进入，导航仍高亮环境，返回保留搜索/排序/页签。
- **证书页**：安装 / 卸载都直接写**当前用户** ROOT 存储（免管理员、可逆）；
  计算机存储里的旧证书只提示、不代删。
- **启动自检屏（boot）**：只在**进程首次打开**时播放；托盘重建的窗口从首帧起直接
  进工作台。窗口与页面首帧的关系是"先隐藏建窗 → 贴回几何 → 等首帧再显示"，
  不会先闪一块白窗再跳位。

### 改一个已经建好的环境

建的时候没绑规则、端口想换一个、描述写错了 —— 不必删了重建。选中环境即开详情面板，
「配置」页签逐项标注**热生效**还是**需停机**；未改动的字段不会被提交。

| 改动 | 运行中 | 生效方式 |
| --- | --- | --- |
| `description` | ✅ | 纯展示字段 |
| `insecure_hosts` / `mitm_hosts` | ✅ | 一次同步 apply，返回后第一个请求就是新配置 |
| `rules` 绑定 | ✅ | 账本 rendered 重编译进快照，不重启实例 |
| 规则文件的内容 | ✅ | import 同名覆盖即对绑定且在跑的环境逐个热应用 |
| `upstream` 绑定 | ✅ | 代理账本定义重编译进快照；改代理实体对引用环境同样热应用 |
| `name` / `listen` | ❌ `conflict` | 它们是环境的对外身份 |
| `proxy_user` / `proxy_password` | ❌ `conflict` | 鉴权门在实例启动时装配 |

绑定一个不存在的规则名在**写入时就被拒绝**（`environment.rules`）；改停机字段返回
`conflict` 并列出哪些字段是热的；热应用被拒时环境标 `failed`/`unhealthy` 并给原因，
"改了但没生效"看得见。

## 配置

启动失败弹原生错误框（windows 子系统没有控制台），stderr 同时留痕
`envboard: <错误码>: <消息>`（带字段路径时附 `(field: …)`）；不可重试错误退出码 2，
其余 1。非法配置**加载即失败**并给出字段路径。`--no-dialog` 供测试 / 无头场景。

### 启动参数

| 旗标 | 默认 | 说明 |
| --- | --- | --- |
| `--state-dir <dir>` | 回退链（见下） | 状态目录 |
| `--port-range <min-max>` | `16000-16999`（便携 / 预览 / 开发形态 `17000-17999`） | 新建环境的端口分配区间 |
| `--capture-budget <bytes>` | 256 MiB | 每环境抓包缓冲预算（0 = 引擎默认） |
| `--log-dir <dir>` | `<state_dir>/logs` | 实例日志目录 |
| `--no-log-file` | 关 | 实例日志只留内存环形缓冲（不影响 `startup.log`） |
| `--max-log-bytes <n>` | 8 MiB | 单环境日志轮转上限（0 = 不轮转） |
| `--no-dialog` | 关 | 启动失败/装配警告不弹框（测试 / 无头） |

桌面形态没有管理端口：管理动作只经 IPC，由 Tauri capabilities 圈禁（只对主窗开放）。

### 状态目录回退链

`--state-dir` → `$ENVBOARD_STATE_DIR` → 便携标记 → 平台默认目录。读到的值会被
**规范化**：`~` 展开、相对路径补绝对、空串等同未设置。环境变量**压过**便携标记。

| 平台 | 默认 | 为什么是这儿 |
| --- | --- | --- |
| Windows | `%LOCALAPPDATA%\envboard` | `LocalAppData` 定义即 nonroaming；状态里含端口分配与单实例锁，漫游只会让两台机器打架 |
| macOS | `~/Library/Application Support/envboard` | macOS 只有这一档（没有 `state` / `logs` 目录类型） |
| Linux | `$XDG_STATE_HOME/envboard` → `~/.local/state/envboard` | XDG 的 state 语义与 nonroaming 是同一条轴 |

不放主目录根（`~/.envboard`）：Windows 明文禁止，macOS/Linux 也各有约定。读环境变量
守两条纪律：**空串等同未设置**；**相对路径的 `$XDG_STATE_HOME` 视为无效并忽略**。

### 正式版 / 便携版 / 预览版 / 开发版

几个形态共用一套机器级资源很危险：状态目录里是账本、单实例锁与共享 CA，激活事件由
state-dir 哈希而来，WebView2 用户数据目录由应用 identifier 决定——都按「正式版」定的话，
换个形态启动就会撞锁、把正在用的正式版拉到前台、或者直接改动正式版的账本。

所以每个进程先解析一个**形态标签**（ADR-014），优先级从高到低：

1. `$ENVBOARD_PROFILE` —— 显式覆盖（`alice` → `envboard-alice`；`release` / `none` = 正式版身份）；
2. **版本号预发布段** —— `0.2.0-preview.1` → `preview`（预览渠道的判据，跟着二进制走）；
3. **便携标记** —— exe 同目录有 `envboard.portable` → `portable`；
4. `debug` 构建 → `dev`；以上都没有 = 正式版。

| | 正式版 | 便携版 | 预览版 | 开发版（debug） |
| --- | --- | --- | --- | --- |
| 触发 | 版本号无预发布段 | `envboard.portable` 标记 | 版本号 `-preview.N` | debug 构建 |
| 状态目录（平台默认） | `%LOCALAPPDATA%\envboard` | `<exe 目录>\envboard-data` | `%LOCALAPPDATA%\envboard-preview` | `%LOCALAPPDATA%\envboard-dev` |
| WebView2 用户数据目录 | `…\com.envboard.desktop` | `…\com.envboard.desktop-portable` | `…\com.envboard.desktop-preview` | `…\com.envboard.desktop-dev` |
| 默认端口段 | `16000-16999` | `17000-17999` | `17000-17999` | `17000-17999` |
| 窗口标题 / 托盘 | 原文 | 后缀「（portable）」 | 后缀「（preview）」 | 后缀「（dev）」 |

单实例范围随状态目录走：同目录第二个实例只把已有窗口拉回前台（ADR-012），不同形态
互不激活，可以同时运行。显式 `--state-dir` / `$ENVBOARD_STATE_DIR` 仍压过一切——临时
指向别的目录不需要动标签。

注意**系统信任库是共享的**：每个形态会物化自己的 CA，点「安装证书」装进的是同一份
当前用户 ROOT 存储（多张证书可以并存，卸载按字节匹配，不会误删另一张）。

### 凭据与对外服务

- **凭据只进不出**：任何视图只有布尔（`has_auth` / `proxy_auth_enabled`），表单不回显。
  注意"留空"不是"保持不变"：`proxy_put` 是整条覆盖，编辑已启用的代理时保存按钮直接
  写成「保存（将清除鉴权）」。
- **代理访问鉴权**：`proxy_user` / `proxy_password` 同生共死，每段非空、不含 `:`/
  空白/控制字符；407 门在引擎内存里定长时间比对。
- **对外服务**：环境监听默认 `127.0.0.1`；「对外服务」开关换成 `0.0.0.0`，勾选而未填
  代理凭据时表单给警示。
- **CA 私钥边界**：私钥永远不出组合根（`envboard-host`），界面只拿 `CaControl` 操作面；
  证书页上出现的只有 `envboard-ca-cert.pem` 与摘要。
- **安全控制只有一个入口**：`insecure_hosts` / `mitm_hosts` / 代理凭据都只在工作台
  「配置」表单里改 —— 做成"随便填个键值对"的文本框，会让"配置说开了、引擎其实没收到"
  这种分叉无从排除。
- **工作台不对外**：桌面形态没有 HTTP 面，管理动作只经本机 IPC。

### 控制面

门面方法契约见 `envboard-admin` crate 文档；IPC 命令映射住在
`crates/app/desktop/src/ops.rs`（纯函数核，命令级契约直接打它）+ `commands.rs`。

```ts
import { invoke } from "@tauri-apps/api/core";
await invoke("status");                                   // 运行时概览
await invoke("env_list");                                 // 环境列表
await invoke("env_create", { body: { name: "dev" } });    // 新建环境
```

判定只在服务端做一次，界面只是它的一个翻译面。

## 状态目录

```
<state_dir>/
├── state.db              环境账本 + 规则账本 + 期望状态（SQLite + WAL，单事务原子写）
├── ui.json               界面偏好：主题 + 窗口几何（物理像素）
├── lock                  单实例锁（flock）
├── rules/<name>.rules    规则物化文件（可再生；给人看，运行时输入是账本 rendered）
├── logs/<env>.log        实例日志（copytruncate 轮转，保留一份 .1）
├── logs/startup.log      壳启动诊断（会话头 + 装配阶段 + panic 现场；1 MiB 轮转留 .1）
├── events.jsonl          控制面审计事件（8 MiB 轮转）
├── trajectories/<env>.jsonl  请求轨迹（append-only 事件流）
└── shared/confdir/       共享 CA（envboard-ca.pem 私钥+证书 / envboard-ca-cert.pem）
```

这张树是**规范**，实现只有一处：`crates/service/manager/src/state.rs` 的 `layout`
模块。两份路径实现必然漂移，而漂移的症状是"文件在、路径对，就是读不出来"。

一处**位置**上的讲究：请求轨迹固定在 `<state_dir>/trajectories/`（它是数据面事件流，
有自己的字节偏移游标与生命周期，不是实例日志），所以与 `logs/` 平级而不是挂在它下面。

**状态目录之外，envboard 不写任何位置**：没有注册表项、没有 `%ProgramFiles%`、没有
`/etc`。要换位置有两条路：`$ENVBOARD_STATE_DIR`（整套一起换），或便携标记。

**`ui.json` 为什么不由 localStorage 管**：WebView2 在窗口销毁时不保证把最近的
localStorage 写入刷盘（实测写入 3s 后关窗、重开读回 null），所以主题与窗口几何由壳
原子写入 `ui.json`；窗口几何跨屏按物理像素还原。

### 启动诊断

发布构建是 windows 子系统、没有控制台，早期失败必须自己留痕：

- `logs/startup.log` 每个会话一段：会话头（版本 / pid / 形态）+ 装配阶段
  （`stage: boot` → `stage: tray+window` → `ready`）+ reconcile 动作。启动失败时
  错误框里带这份日志的路径。超 1 MiB 轮转一代（`startup.log.1`）。
- 任何线程的 panic 先落盘（线程 / file:line / payload / backtrace）再走默认行为。
- 装配期 panic 与 Tauri build 失败弹**原生错误框**（不依赖事件循环），随后按退出码
  退出：boot 失败按可重试性 1/2，装配 panic 3。
- `startup.log` 是壳级诊断，**不受 `--no-log-file` 影响**（那条旗标只管实例日志）；
  `--log-dir` 可以改它的落点。诊断自身写入失败只降级到 stderr，不挡启动。

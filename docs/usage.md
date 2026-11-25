# 使用

面向使用者的操作说明：能力清单、安装、接入、工作台各屏与状态目录。
项目概览见 [README](../README.md)；构建与发布见 [building.md](building.md)；
内部结构见 [architecture.md](architecture.md)。

## 能力一览

| 方向 | 能力 |
| --- | --- |
| **环境与端口** | 环境新建 / 启停 / 删除 / 显式重分配端口；端口从 `16000-16999` 随机试绑，新建冲突自动重试一次；**绑定即真相**（`EADDRINUSE` → `port_conflict`，不偷偷换端口） |
| **热应用** | 写操作返回后的第一个请求就是新配置：`ArcSwap` 换快照，无轮询、无收敛窗口；运行中的请求按进入时的旧快照跑完 |
| **规则（hosts）** | 账本 `state.db` 是唯一真相，物化文件可再生；导入同名覆盖即对绑定且在跑的环境逐个热应用；脏规则 fail-closed 502。**NSH 壳暂未提供导入/编辑入口**（见 [limitations.md](limitations.md)） |
| **TLS 策略** | `insecure_hosts` 精确域名放宽上游校验（无通配符、无子域继承）；`mitm_hosts` 白名单只解密点名的域名，其余盲隧道 |
| **上游代理** | 按名管理的代理账本 + 环境按名绑定（热字段）；TLS 目标走 CONNECT 隧道，明文 http 按 absolute-URI 转发；删除被引用的代理会被拒绝并点名。**NSH 壳暂未提供管理入口** |
| **代理鉴权** | `proxy_user` / `proxy_password` 同生共死 → 407 门；凭据只在内存与 0600 账本，不进 argv / 视图 / 日志 |
| **共享 CA** | 所有实例一张 `envboard` CA（`shared/confdir/`）；MITM 按 SNI 现签；轮换下一次 CONNECT 即生效；不复用同目录的 `mitmproxy-ca*.pem` |
| **账本与自愈** | SQLite + WAL 单事务原子写；引擎线程 panic → `failed` → 按期望状态自动重启（任务级崩溃隔离，release 保持 unwind） |
| **抓包与 HAR** | 环境子页的单例抓包会话：版本订阅 + 尾部窗口（变了才读）、状态/类型筛选、请求响应详情面板、导出 HAR 到下载目录 |
| **桌面工作台** | 环境 / 证书两页；环境页实例/规则两个子页签、状态过滤 chips、搜索排序；点行开详情抽屉；抓包子页 |
| **后台运行 / 托盘** | 关窗默认**隐藏进托盘**（设置里可改成关窗即退），进程与引擎线程常驻、端口继续服务；托盘「显示窗口 / 退出」两项；隐藏态页面循环卸载、近零 CPU，唤出即新鲜；窗口几何与主题跨会话记忆 |
| **证书页** | 摘要/指纹/路径、已安装状态、**安装 / 卸载直接作用于当前用户 ROOT 存储**（免管理员、可逆；不提供下载与系统向导） |
| **设置** | 主题三档（浅色/深色/系统）+ 行为（常驻托盘 / 关窗即退）+ 关于（版本核信息） |

**不提供的能力**（写明的取舍，不是待办）：

| 能力 | 状态 |
| --- | --- |
| 跨环境对比 / 活动审计页 | ❌ 未提供 |
| 远程访问 / 脚本化管理面（HTTP API） | ❌ 管理面只在本进程内 |
| HTTP/2 客户端（gRPC 等） | ❌ 数据面只实现 HTTP/1.1 |
| PAC / 透明代理 / SOCKS | ❌ 未提供 |
| 开机自启 | ❌ 未提供 |

## 安装

从 [Releases](https://github.com/Slothtron/envboard/releases) 下载 Windows 构建：

- `envboard.exe`：免安装，双击即用。NSH 原生壳，**不需要 WebView2**。
- `envboard-portable.zip`：便携包（exe + `envboard.portable` 标记），解压即用。

未签名：首次运行会撞 SmartScreen，点「更多信息 → 仍要运行」。

**从旧 Tauri 版升级**：状态目录是同一份（`%LOCALAPPDATA%\envboard`），账本、规则、
CA、主题偏好原样保留，升级即迁移。两版**不要同时运行**——单实例锁会拦下后启动的
那个并给出提示。

### 从源码构建

前置：Windows 10/11 + **Rust 1.98.0**（`rust-toolchain.toml` 钉死）。纯 Rust
工具链，无 Node / pnpm / tauri-cli。

```bash
cargo build -p envboard-nsh --release   # target/release/envboard-nsh.exe（发布名 envboard.exe）
mise run dev                            # 开发运行（真宿主；RUST_LOG=info 看装配日志）
```

离线口径：`Cargo.lock` 入库，cargo registry 已暖；fresh clone 的首次构建要网。

### 三步入网

1. **起工作台**：双击 `envboard.exe`（无控制台黑窗、无监听端口）。状态目录用平台
   默认值；换目录见[配置](#配置)。
2. **建环境**：环境页「新建环境」，端口自动分配；已有规则集的环境在详情抽屉里
   能看到绑定与条数（规则导入入口暂未接进 NSH 壳，见 [limitations.md](limitations.md)）。
3. **客户端接入**：每个环境详情里都有可复制的那一行。

```bash
export https_proxy=http://127.0.0.1:16301 http_proxy=http://127.0.0.1:16301
curl https://api.example.com/
```

### 便携形态

只发 exe 就是合法分发形态：免安装、不写注册表。想让"数据跟着 exe 走"，在 exe 同目录
放一个**非空**的 `envboard.portable` 文本文件：数据落在 exe 旁边的 `envboard-data/`，
整个目录可整体搬走，删目录 = 卸载；一个副本只能起一个实例。

Releases 里的 `envboard-portable.zip` 就是这个形态：解压即用，exe 与标记已经放好。

便携副本与正式版的状态目录互相独立（账本、单实例锁、CA 各一份），可以同时开着。
**端口区间目前都是默认 `16000-16999`**：多份副本同时建环境时注意端口冲突
（`port_conflict` 显式报错，不偷偷换端口），用「换个端口」显式错开。

## 工作台

- **两页**：环境、证书。环境页有实例 / 规则两个子页签；实例列表列为
  环境·状态·监听·规则·状态原因·操作，顶部 chips 聚合 全部 / 运行中 / 已停止 / 需处理。
- **详情抽屉**：点整行打开（名称 / 状态 / 监听 / 上游 / 规则集 / 代理地址），
  底部动作：复制地址 / 换个端口 / 抓包日志 / 删除环境。
- **抓包 = 环境子页**：抽屉「抓包日志」进入；列表 + 详情面板（请求/响应两页签），
  状态/类型筛选，停止抓包 / 清空 / 导出 HAR（落下载目录，重名不覆盖）。
- **证书页**：安装 / 卸载都直接写**当前用户** ROOT 存储（免管理员、可逆）；
  计算机存储里的旧证书只提示、不代删。
- **设置**：主题三档 + 行为（常驻托盘 / 关窗即退）+ 关于。
- **托盘**：图标 tooltip 即窗口标题；左键唤出主窗口；右键菜单只有
  「显示窗口 / 退出」两项。「退出」走正常关闭路径：停所有实例 → WAL 合库 → 关窗。

## 配置

NSH 壳**没有命令行旗标**：启动方式只有环境变量与便携标记两条路。
宿主启动失败不拦着开窗——错误落在界面错误槽里（`code · message`），
状态条同步可见；`RUST_LOG=info` 可在 stderr 看装配日志。

### 状态目录回退链

`$ENVBOARD_STATE_DIR` → 便携标记 → 平台默认目录。读到的值会被**规范化**：
`~` 展开、相对路径补绝对、空串等同未设置。环境变量**压过**便携标记。

| 平台 | 默认 | 为什么是这儿 |
| --- | --- | --- |
| Windows | `%LOCALAPPDATA%\envboard` | `LocalAppData` 定义即 nonroaming；状态里含端口分配与单实例锁，漫游只会让两台机器打架 |
| macOS | `~/Library/Application Support/envboard` | macOS 只有这一档（没有 `state` / `logs` 目录类型） |
| Linux | `$XDG_STATE_HOME/envboard` → `~/.local/state/envboard` | XDG 的 state 语义与 nonroaming 是同一条轴 |

不放主目录根（`~/.envboard`）：Windows 明文禁止，macOS/Linux 也各有约定。读环境变量
守两条纪律：**空串等同未设置**；**相对路径的 `$XDG_STATE_HOME` 视为无效并忽略**。

（组合根 `HostOptions` 仍支持调用方直接指定 `state_dir`，供测试 / 示例用；
NSH 壳不暴露它。）

### 多副本与单实例

同一状态目录只许一个写者：第二个进程在启动早期经命名管道握手，被接受就退出、
主实例把窗口拉到前台（调用方进程 image 路径会被校验，只有同一个可执行文件才被
接受）。不同状态目录的副本互不激活，可以同时运行。

旧 Tauri 版用另一条管道名，NSH 的 preflight 看不见它；若它仍占着同一份状态目录，
NSH 启动时会撞单实例锁，弹框提示「先关闭旧版再启动」后退出。

注意**系统信任库是共享的**：每份状态目录会物化自己的 CA，点「安装证书」装进的是
同一份当前用户 ROOT 存储（多张证书可以并存，卸载按字节匹配，不会误删另一张）。

### 凭据与对外服务

- **凭据只进不出**：任何视图只有布尔（`has_auth` / `proxy_auth_enabled`），表单不回显。
- **代理访问鉴权**：`proxy_user` / `proxy_password` 同生共死，每段非空、不含 `:`/
  空白/控制字符；407 门在引擎内存里定长时间比对。
- **对外服务**：环境监听默认 `127.0.0.1`；「对外服务」开关换成 `0.0.0.0`，勾选而未填
  代理凭据时表单给警示。
- **CA 私钥边界**：私钥永远不出组合根（`envboard-host`），界面只拿 `CaControl` 操作面；
  证书页上出现的只有 `envboard-ca-cert.pem` 与摘要。
- **工作台不对外**：没有 HTTP 面，管理动作只在本进程内直调门面。

### 控制面

门面方法契约见 `envboard-admin` crate 文档；NSH 壳的绑定住在
`crates/app/nsh/src/service.rs`（进程内直调，异步动作经写车道回传）与
`actions.rs`（用例编排）。判定只在门面做一次，界面只是它的一个翻译面。

## 状态目录

```
<state_dir>/
├── state.db              环境账本 + 规则账本 + 期望状态（SQLite + WAL，单事务原子写）
├── ui.json               界面偏好：主题 + 窗口几何 + 常驻托盘开关
├── lock                  单实例锁（flock）
├── rules/<name>.rules    规则物化文件（可再生；给人看，运行时输入是账本 rendered）
├── logs/<env>.log        实例日志（copytruncate 轮转，保留一份 .1）
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

**`ui.json` 由壳原子写入**（临时文件 + rename）：主题、窗口几何与常驻托盘开关
跨会话保留；坏文件、未知值一律宽容回落，永不拦启动。窗口几何跨屏按物理像素还原。

### 启动诊断

发布构建是 windows 子系统、没有控制台：

- 宿主启动失败**不拦着开窗**：错误进界面错误槽与状态条（`code · message`），
  用户看得到是账本打不开还是环境没了。
- 单实例冲突（旧版占锁 / 激活被拒）弹**原生提示框**后按退出码退出。
- `RUST_LOG=info`（或 `debug`）时装配与托盘动作留痕 stderr；实例日志仍在
  `<state_dir>/logs/<env>.log`。
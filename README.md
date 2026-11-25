<p align="center">
  <img src="crates/app/nsh/assets/icon.png" alt="envboard" width="96">
</p>

<h1 align="center">envboard</h1>

<p align="center"><b>多环境代理管理器：一个环境 = 一个端口，几个环境同时活着。</b></p>

<p align="center">
  <a href="#它是什么">它是什么</a> ·
  <a href="#为什么需要它">为什么</a> ·
  <a href="#能力概览">能力</a> ·
  <a href="#架构一览">架构</a> ·
  <a href="#安装">安装</a> ·
  <a href="#文档">文档</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/platform-Windows%2010%2F11-0078d4?style=flat-square" alt="Windows 10/11">
  <img src="https://img.shields.io/badge/rust-1.98-000000?style=flat-square&logo=rust" alt="Rust 1.98">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-3da639?style=flat-square" alt="MIT License"></a>
</p>

## 它是什么

**envboard** 是 Windows 上的桌面代理工作台：每个「环境」是一个独立端口上的代理实例，
把客户端的 `http_proxy` / `https_proxy` 指到 `127.0.0.1:<环境端口>`，就是"在用那个
环境"。多套环境（dev / staging / 某人分支 / 离线复现）**同时活着**，切换只是换一个
端口号。

- **进程内引擎。** 一个环境 = 一个 OS 线程 + 一个 current-thread runtime + 一个监听
  端口，全部跑在 envboard 这一个进程里：没有子进程、没有轮询、没有第二种语言。
- **改配置当场生效。** 写操作经 `ArcSwap` 原子换入新快照，**返回后的第一个请求就是
  新配置**；没有收敛窗口、没有"等下一个心跳"。
- **只改"连到哪"。** 规则是 hosts 风格的静态覆盖表，在建连时改写上连目标 —— 请求
  内容、`Host` 头、SNI 基准都不动。
- **单二进制桌面形态。** NSH 原生壳（无 webview、无内嵌 SPA），管理面进程内直调
  门面，**不起任何监听端口**。发布物只有一颗 `envboard.exe`。

## 为什么需要它

- **系统代理只有一个。** 同时验证两套后端就得来回改系统设置，还容易忘。
- **hosts 文件不够用。** 要管理员权限、要刷 DNS、不能按端口区分、抓不到流量、也没法
  给不同环境配不同的上游与凭据。
- **抓包工具太重。** 大多要求你把全部流量解密；envboard 的 MITM 白名单只解密你点名的
  域名，其余 CONNECT 原样直通（路由照旧、鉴权照旧），CA 也是自己的、可独立轮换。

## 能力概览

| 方向 | 能力 |
| --- | --- |
| **多环境与端口** | 一个环境 = 一个进程内引擎实例 + 一个监听端口，多个环境同时在线；端口区间内自动分配，冲突显式报 `port_conflict`，不偷偷换端口 |
| **热应用** | 配置、规则绑定、上游绑定、`insecure_hosts` / `mitm_hosts` 都是热字段：写操作返回即新配置 |
| **规则** | hosts 风格覆盖表，只在建连时改写上连目标；导入同名规则集即对绑定环境逐个热应用 |
| **TLS 与证书** | `insecure_hosts` 精确域名放宽校验；`mitm_hosts` 白名单只解密点名的域名；自有 `envboard` 共享 CA：证书页一键安装 / 卸载到当前用户信任库（免管理员） |
| **上游代理与鉴权** | 代理账本按名绑定到环境；环境级 `proxy_user` / `proxy_password` → 407 门，凭据只进不出 |
| **抓包** | 环境子页的单例会话：实时增量推送、方法 / 状态 / 主机筛选、请求响应详情、导出 HAR |
| **桌面工作台** | 环境 / 证书两页，状态过滤 chips、排序；点行开详情抽屉（概览 / 规则 / 代理地址）；抓包子页（列表 + 详情面板） |
| **后台运行 / 托盘** | 关窗默认隐藏进托盘（可设关窗即退），进程与引擎线程常驻、代理照常服务；托盘「显示窗口 / 退出」两项；隐藏态近零 CPU，唤出即新鲜；窗口几何与主题跨会话记忆 |
| **账本与自愈** | SQLite + WAL 单事务原子写；引擎线程 panic 后按期望状态自动重启 |

不提供的能力（跨环境对比、远程管理面、HTTP/2、PAC / 透明代理 / SOCKS、开机自启）
与其余边界见 [已知限制](docs/limitations.md)。

## 架构一览

数据面是纯 Rust 的进程内代理：一个环境一个线程、一个 current-thread runtime、一个
监听端口；`ConnectTarget` 是唯一接缝（只改"连到哪"，不动请求内容与 SNI）；配置经
`ArcSwap` 原子换入快照，失败整套拒绝、旧快照继续服务；数据面写日志走有界总线，
永不阻塞。界面是 NSH 0.6.1 原生壳，管理动作进程内直调 `AdminService` 门面，与传输无关。

依赖单向：`protocol/events ← engine ← manager ← admin ← host ← nsh`；契约住在
各 crate 的 rustdoc 与 `tests/contract-tests/` 的 84 个 golden fixture 里，不在独立
文档层。分层、状态目录与契约位置见 [架构](docs/architecture.md)。

## 安装

从 [Releases](https://github.com/Slothtron/envboard/releases) 下载 Windows 构建：
`envboard.exe`（免安装）或 `envboard-portable.zip`（便携包，数据跟着目录走，可与
正式版并存）。NSH 原生壳，**不需要 WebView2**。未签名，首次运行按 SmartScreen
提示「仍要运行」。

从源码构建：

```bash
cargo build -p envboard-nsh --release   # target/release/envboard-nsh.exe（发布名 envboard.exe）
mise run dev                            # 开发运行（真宿主，RUST_LOG=info 看装配日志）
```

状态目录与旧 Tauri 版是同一份（`%LOCALAPPDATA%\envboard`），升级即迁移，两版不要
同时运行。要多开互不干扰的副本：`$ENVBOARD_STATE_DIR` 指到别处，或 exe 旁放
`envboard.portable` 标记（数据落在 exe 旁的 `envboard-data\`）。

前置工具链、三步入网、便携形态与状态目录见 [使用](docs/usage.md)；
发布链路与分发形态见 [构建与发布](docs/building.md)。

## 文档

| 我想了解 | 从这里开始 |
| --- | --- |
| 安装、接入与工作台使用 | [docs/usage.md](docs/usage.md) |
| 构建、发布与分发 | [docs/building.md](docs/building.md) |
| 引擎、分层与契约位置 | [docs/architecture.md](docs/architecture.md) |
| 验证、健康状态与排障 | [docs/operations.md](docs/operations.md) |
| 已知限制与非目标 | [docs/limitations.md](docs/limitations.md) |
| 门面 / 规则 / 错误码等契约 | `cargo doc --workspace --no-deps` |
| 版本记录 | [CHANGELOG.md](CHANGELOG.md) |

## 版本与许可

当前版本 **0.2.4**（根 `Cargo.toml` 的 `[workspace.package]`），与 git tag 一致是
发布前置条件。版本记录见 [CHANGELOG.md](CHANGELOG.md)。

[MIT](LICENSE) © Slothtron

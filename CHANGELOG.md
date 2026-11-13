# Changelog

envboard 只发一个产物：`envboard.exe`（桌面工作台 + 进程内引擎）。版本以 `vX.Y.Z`
标签发布，与根 `Cargo.toml` 的 `[workspace.package]` 一致。格式参考
[Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)。

## Unreleased

- **证书安装/卸载改为直接写当前用户存储**：证书页的「安装证书」不再打开系统证书导入
  向导，也不再让用户选当前用户/计算机——直接写当前用户 ROOT 存储（免管理员、幂等、
  可逆）；卸载只处理当前用户存储，检测到计算机存储里还有旧证书时只提示、不再拉起
  提权助手。
- **证书页移除下载与外部入口**：不再提供「下载根证书」与「在证书管理器中查看」；
  对应 IPC 命令（`ca_pem` / `save_text_as` / `open_cert_install_wizard` /
  `open_cert_manager`）与 `--machine-trust` 提权助手一并退场。
- **正式版 / 便携版 / 预览版 / 开发版身份隔离**：进程按形态标签使用各自的状态目录、
  WebView2 用户数据目录与默认端口段（非稳定形态统一 `17000-17999`），窗口标题与托盘
  提示带「（portable）」「（preview）」「（dev）」，可与已安装的正式版同时运行。
  标签优先级：`$ENVBOARD_PROFILE` → 版本号预发布段（`X.Y.Z-preview.N` → `preview`）→
  便携标记（`envboard.portable` → `portable`）→ `debug` 构建 → 正式版；
  `release` / `none` 显式恢复正式版身份。
- **preview 分发渠道**：`vX.Y.Z-preview.N` 标签发布 prerelease，只出免安装的
  `envboard-preview.exe`（不发 NSIS——perMachine 安装会覆盖正式版），数据落在
  `%LOCALAPPDATA%\envboard-preview`。
- **便携包随 Release 分发**：workflow 额外产出 `envboard-portable.zip`
  （`envboard.exe` + `envboard.portable` 标记；preview 渠道为
  `envboard-preview-portable.zip`），解压即用，数据落在解压目录的 `envboard-data\`。

## 0.1.0 — 2026-11-13

首个公开版本：Windows 桌面应用（Tauri 2 壳 + 进程内代理引擎），单颗 `envboard.exe`；
从 Releases 下载 NSIS 安装包或免安装 exe。

- **桌面工作台。** 环境 / 证书两视图；列表搜索、状态过滤、排序；双击行打开悬浮详情
  （概览 / 配置 / 规则）；顶栏指标聚合。
- **多环境与端口。** 一个环境 = 一个进程内引擎实例 + 一个监听端口，多个环境同时
  活着；端口在 `16000-16999` 区间自动分配，冲突不换端口而是显式报 `port_conflict`。
- **热应用。** 写操作返回后的第一个请求就是新配置（`ArcSwap` 原子换快照）；配置、
  规则绑定、上游绑定、`insecure_hosts` / `mitm_hosts` 都是热字段。
- **规则。** hosts 风格覆盖表，只在建连时改写上连目标；导入同名规则集即对绑定且在跑
  的环境逐个热应用。
- **TLS 与证书。** `insecure_hosts` 对精确域名放宽上游校验；`mitm_hosts` 白名单只
  解密点名的域名，其余 CONNECT 盲隧道；自有 `envboard` 共享 CA，安装走系统证书向导、
  卸载处理当前用户与计算机根、下载根证书走原生另存为。
- **上游代理。** 代理账本 + 环境按名绑定；TLS 目标走 CONNECT 隧道，明文 http 按
  absolute-URI 转发。
- **代理鉴权。** 环境级 `proxy_user` / `proxy_password` → 407 门；凭据只进不出。
- **抓包。** 环境子页的单例会话：实时增量推送、方法 / 状态 / 主机筛选、请求响应
  详情、导出 HAR。
- **后台运行 / 托盘。** 关窗销毁窗口与 WebView、进程与引擎线程常驻；托盘重建窗口；
  窗口几何与主题跨会话记忆；boot 自检屏只在进程首个窗口播放。
- **账本与自愈。** SQLite + WAL 单事务原子写；引擎线程 panic 后按期望状态自动重启。
- **壳加固。** 禁 WebView2 默认右键菜单与浏览器加速键，编辑键保留。
- **契约与验证。** rustdoc + 84 个 golden fixture + 命令面契约；`mise run verify`
  覆盖 Rust / 契约 / 桌面三层；发布物内容由 artifact 测试钉住。

**运行要求**

- Windows 10/11 + **WebView2** 运行时（系统通常已带 Evergreen 运行时）。
- 未签名 exe：首次运行会撞 SmartScreen，点「更多信息 → 仍要运行」。
- NSIS 安装包走 perMachine（装到 `C:\Program Files\envboard`，安装与卸载各需一次 UAC）。
- 只发 Windows x64。

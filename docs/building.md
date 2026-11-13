# 构建与发布

发布物只有一个：Tauri 桌面二进制 `envboard`（引擎 + 管理器 + 内嵌 SPA 全在其中）。
本地构建命令与离线口径见 [usage.md](usage.md#从源码构建)；本文只讲发布链路与分发形态。

## 发布流程

两个渠道共用一条 workflow，**版本号里的预发布段就是渠道身份**（ADR-014）：

| 渠道 | 标签 | 产物 | Release |
| --- | --- | --- | --- |
| stable | `vX.Y.Z` | `envboard.exe` + `envboard-portable.zip` + NSIS 安装包 | 正式版 |
| preview | `vX.Y.Z-preview.N` | `envboard-preview.exe` + `envboard-preview-portable.zip` | prerelease |

preview 不发 NSIS：安装包是 perMachine，会装到同一个 `C:\Program Files\envboard`、
覆盖用户装着的正式版；预览版按免安装分发，数据落在 `%LOCALAPPDATA%\envboard-preview`，
与正式版互不影响。

**便携包在 CI 里打包**：便携形态不是另一种编译产物，而是「同一个 exe + 同目录的
`envboard.portable` 标记」，所以 workflow 在收集产物后多打一个 zip（`Compress-Archive`，
内容是 `<exe>` + 标记文件）。裸 `envboard.exe` **不带**标记，仍是正式版身份
（数据在 `%LOCALAPPDATA%\envboard`，与安装版共用）——要与已安装版本并存，用便携包。

流程：

1. 推标签（或手动 `workflow_dispatch`，只出构建产物、不建 Release）；
2. 从根 `Cargo.toml` 读版本号并判定渠道（含 `-` = preview）；
3. 在 `windows-latest` 上安装 pnpm / Node 24.16 / Rust 1.98 / tauri-cli；
4. 先构建前端（`pnpm --dir frontend install && build`）—— `cargo tauri build`
   在调用 cargo 之前就要求 `frontendDist` 存在，而 `dist/` 不入库；
5. `cargo tauri build --bundles nsis`（stable）或 `--no-bundle`（preview），
   带 `ENVBOARD_SKIP_FRONTEND_BUILD=1` 避免 build.rs 再跑一遍 pnpm；
6. 收集产物：stable 收 `envboard.exe` 与 NSIS 安装包，preview 把 `envboard.exe`
   改名为 `envboard-preview.exe`；两种情况都把「exe + `envboard.portable` 标记」
   打成便携 zip，上传 artifact；
7. 打标签时创建 GitHub Release，preview 标 `prerelease`。

标签与版本号必须一致：workflow 会核对 `v$version` 与 `Cargo.toml` 的
`[workspace.package] version`，不一致直接失败。**发预览要先把版本号改成
`X.Y.Z-preview.N`**（根 `Cargo.toml` 与 `crates/app/desktop/Cargo.toml` 两处同步）。

```bash
cd crates/app/desktop
cargo tauri build              # envboard.exe + NSIS 安装包（target/release/bundle/）
cargo tauri build --bundles nsis   # 只出 NSIS（先 unset https_proxy：下载会被代理打断）
cargo tauri build --no-bundle      # 只出裸 exe（preview 渠道的形态）
```

本地联调不必改版本号：`debug` 构建自动带 `dev` 标签，`$ENVBOARD_PROFILE=preview`
可以把这个二进制临时当预览版跑，`release` / `none` 则恢复正式版身份。细节见
[使用 · 正式版 / 便携版 / 预览版 / 开发版](usage.md#正式版--便携版--预览版--开发版)。

## 分发形态

这个 exe 是**自足**的：前端资产编译期内嵌，运行时不读任何随附资源、配置文件或脚本，
**目标机器不需要 Node**；CRT 静态链接，不依赖 VC++ 可再发行包。运行时只依赖
**WebView2**（Windows 10/11 通常已带 Evergreen 运行时）。

- **NSIS 安装包**走 perMachine（安装与卸载各需要一次 UAC，静默 `/S`），装到
  `C:\Program Files\envboard`；状态目录仍按平台默认落在 `%LOCALAPPDATA%\envboard`。
  程序目录与数据目录分离：卸载只删程序，不动账本与 CA。
- **裸 exe** 免安装，但身份仍是正式版（数据 `%LOCALAPPDATA%\envboard`）：与已安装版本
  同时开时是同一个实例/同一份账本。
- **便携包 zip**（exe + `envboard.portable`）：解压即用，数据在解压目录的
  `envboard-data\`，WebView2 profile、标题与端口段都是 portable 形态，可与安装版并存；
  整个目录可搬走，删目录即卸载。
- **没有代码签名**：未签名 exe 第一次运行会撞 SmartScreen（"Windows 已保护你的
  电脑"），点「更多信息 → 仍要运行」。去掉提示只能买证书签名。
- **杀软误报**：单文件 Rust 二进制偶发被启发式引擎标记，加白或上报即可。
- **没有自动更新**：新版本从 Releases 重新下载覆盖即可；应用内不检查更新。
- **平台**：首个版本只发 Windows x64。

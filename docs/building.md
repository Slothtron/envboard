# 构建与发布

发布物只有一个：Tauri 桌面二进制 `envboard`（引擎 + 管理器 + 内嵌 SPA 全在其中）。
本地构建命令与离线口径见 [usage.md](usage.md#从源码构建)；本文只讲发布链路与分发形态。

## 发布流程

正式发布由 `.github/workflows/release.yml` 执行：

1. 推 `vX.Y.Z` 标签（或手动 `workflow_dispatch`，只出构建产物、不建 Release）；
2. 在 `windows-latest` 上安装 pnpm / Node 24.16 / Rust 1.98 / tauri-cli；
3. 先构建前端（`pnpm --dir frontend install && build`）—— `cargo tauri build`
   在调用 cargo 之前就要求 `frontendDist` 存在，而 `dist/` 不入库；
4. `cargo tauri build --bundles nsis`（带 `ENVBOARD_SKIP_FRONTEND_BUILD=1`，
   避免 build.rs 再跑一遍 pnpm）；
5. 收集 `envboard.exe` 与 NSIS 安装包，上传 artifact `envboard-windows-x64`；
6. 打标签时创建 GitHub Release 并附上两个文件。

标签与版本号必须一致：workflow 会核对 `v$version` 与 `Cargo.toml` 的
`[workspace.package] version`，不一致直接失败。

```bash
cd crates/app/desktop
cargo tauri build              # envboard.exe + NSIS 安装包（target/release/bundle/）
cargo tauri build --bundles nsis   # 只出 NSIS（先 unset https_proxy：下载会被代理打断）
```

## 分发形态

这个 exe 是**自足**的：前端资产编译期内嵌，运行时不读任何随附资源、配置文件或脚本，
**目标机器不需要 Node**；CRT 静态链接，不依赖 VC++ 可再发行包。运行时只依赖
**WebView2**（Windows 10/11 通常已带 Evergreen 运行时）。

- **NSIS 安装包**走 perMachine（安装与卸载各需要一次 UAC，静默 `/S`），装到
  `C:\Program Files\envboard`；状态目录仍按平台默认落在 `%LOCALAPPDATA%\envboard`。
  程序目录与数据目录分离：卸载只删程序，不动账本与 CA。
- **没有代码签名**：未签名 exe 第一次运行会撞 SmartScreen（"Windows 已保护你的
  电脑"），点「更多信息 → 仍要运行」。去掉提示只能买证书签名。
- **杀软误报**：单文件 Rust 二进制偶发被启发式引擎标记，加白或上报即可。
- **没有自动更新**：新版本从 Releases 重新下载覆盖即可；应用内不检查更新。
- **平台**：首个版本只发 Windows x64。

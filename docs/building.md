# 构建与发布

发布物只有一个：NSH 原生桌面二进制 `envboard.exe`（引擎 + 管理器 + NSH 壳全在其中，
无 webview、无内嵌 SPA）。本地构建命令与离线口径见 [usage.md](usage.md#从源码构建)；
本文只讲发布链路与分发形态。

## 发布流程

单一渠道：`vX.Y.Z` 标签 → 免安装 exe + 便携包 zip。

**预览渠道暂停**：Tauri 时代的形态标签隔离（ADR-014：preview/dev 各自的数据目录与
端口段）随壳退场，NSH 形态尚未实现等价隔离。为避免预览二进制与正式版共用账本，
workflow 对带预发布段的版本号（`X.Y.Z-preview.N` 等）**直接失败**。恢复预览渠道
需要先在 NSH 侧实现身份隔离，再改 workflow。

**便携包在 CI 里打包**：便携形态不是另一种编译产物，而是「同一个 exe + 同目录的
`envboard.portable` 标记」，所以 workflow 在收集产物后多打一个 zip（`Compress-Archive`，
内容是 `<exe>` + 标记文件）。裸 `envboard.exe` **不带**标记，数据在
`%LOCALAPPDATA%\envboard`——要多开互不干扰的副本，用便携包或 `$ENVBOARD_STATE_DIR`。

流程：

1. 推标签（或手动 `workflow_dispatch`，只出构建产物、不建 Release）；
2. 从根 `Cargo.toml` 读版本号，带预发布段即失败；
3. 在 `windows-latest` 上安装 Rust 1.98（与 `rust-toolchain.toml` 一致）；
4. `cargo build -p envboard-nsh --release --locked`（默认特征，`harness` 探针不进发布物）；
5. 收集产物：`target/release/envboard-nsh.exe` 改名为 `envboard.exe`（发布名是产品
   身份，crate 二进制名只是构建产物名），再把「exe + `envboard.portable` 标记」
   打成便携 zip，上传 artifact；
6. 打标签时创建 GitHub Release。

标签与版本号必须一致：workflow 会核对 `v$version` 与 `Cargo.toml` 的
`[workspace.package] version`，不一致直接失败。版本触点共三处，缺一会漂移：
根 `Cargo.toml` `[workspace.package]`、`README.md`「当前版本」、`CHANGELOG.md`；
改完跑 `cargo update -w --offline` 刷新 `Cargo.lock`。

```bash
cargo build -p envboard-nsh --release            # target/release/envboard-nsh.exe
cargo build -p envboard-nsh --release --features harness   # 验收用（golden / 压测探针）
```

## 分发形态

这个 exe 是**自足**的：运行时不读任何随附资源、配置文件或脚本，**目标机器不需要
Node，也不需要 WebView2**（NSH 原生渲染）。

- **裸 exe** 免安装：数据 `%LOCALAPPDATA%\envboard`，与旧 Tauri 版同一份账本，
  升级即迁移；两版不要同时运行（单实例锁会提示）。
- **便携包 zip**（exe + `envboard.portable`）：解压即用，数据在解压目录的
  `envboard-data\`，整个目录可搬走，删目录即卸载；把多份 exe 各放一个目录、
  各带一个标记，就是多套互不干扰的环境。
- **没有 NSIS 安装包**：Tauri bundler 随壳退场；免安装 + 便携包覆盖全部分发场景。
- **没有 Authenticode 签名，但有完整性与来源证明**：未签名 exe 第一次运行会撞
  SmartScreen（"Windows 已保护你的电脑"），点「更多信息 → 仍要运行」；去掉该提示
  只能买证书签名。Release 另附 `SHA256SUMS` 校验和与 cosign keyless 签名
  （`.sigstore`），用 `sha256sum -c SHA256SUMS` 与
  `cosign verify-blob --bundle <产物>.sigstore <产物>` 验证完整性与来源。
- **杀软误报**：单文件 Rust 二进制偶发被启发式引擎标记，加白或上报即可。
- **没有自动更新**：新版本从 Releases 重新下载覆盖即可；应用内不检查更新。
- **平台**：只发 Windows x64。
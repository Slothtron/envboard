//! 发布物内容校验：**唯一的那颗二进制里真的带着引擎与内嵌前端**（自 envboard-server 移植）。
//!
//! 只判**构建产物**：它必须住在产物 crate 的集成测试里，因为只有这里拿得到
//! `CARGO_BIN_EXE_envboard`。
//!
//! 判据：
//!
//! 1. 引擎在场：线程名标记 `envboard-engine-`（组合根要是被悄悄换成 IPC 客户端，这条就会红）；
//! 2. 产品身份在场：入口页标题字符串真的进了二进制（tauri.conf.json 的窗口标题，
//!    与 frontend/index.html 的 <title> 同源同值）；dist 的编译期内嵌由 tauri-build
//!    保证 —— dist 缺失时构建本身就响亮失败，不需要在二进制里 grep 压缩资产；
//! 3. dist 形态：体积下限（截断是最阴险的坏法）、外链资产（严格 CSP 的前提）、
//!    关键接线符号（IPC 命令名替代退场的 SSE 端点）、导航 ↔ 视图主体一一对应；
//! 4. 壳接线：generate_handler 注册了三条流命令；CSP 在 tauri.conf.json 里
//!    保持严格档（无 unsafe-inline + react-aria 样式 hash 豁免 + ipc: 通道）。
//!
//! 顺序上不需要"先 pnpm build"：build.rs 在 cargo build 时自动驱动前端构建，
//! cargo 又保证跑集成测试前已经构建好本 crate 的 bin。

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // crates/app/desktop → 仓库根（上三级）
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .canonicalize()
        .expect("repo root")
}

fn envboard_binary() -> &'static str {
    env!("CARGO_BIN_EXE_envboard")
}

/// 收集 TS 联合类型（`export type X = ... ;`）里的全部字符串字面量。
fn union_literals(text: &str, anchor: &str) -> std::collections::BTreeSet<String> {
    let mut found = std::collections::BTreeSet::new();
    let Some(start) = text.find(anchor) else {
        return found;
    };
    let rest = &text[start..];
    let region = match rest.find(';') {
        Some(end) => &rest[..end],
        None => rest,
    };
    let mut cursor = region;
    while let Some(open) = cursor.find('"') {
        cursor = &cursor[open + 1..];
        match cursor.find('"') {
            Some(close) => {
                found.insert(cursor[..close].to_string());
                cursor = &cursor[close + 1..];
            }
            None => break,
        }
    }
    found
}

/// 收集 TS/TSX 数组定义里 `key: "…"` 字面量的值（NAV 的视图键）。
fn key_literals(text: &str, anchor: &str) -> std::collections::BTreeSet<String> {
    let mut found = std::collections::BTreeSet::new();
    let needle = "key: \"";
    for (index, _) in text.match_indices(needle) {
        if index < text.find(anchor).unwrap_or(usize::MAX) {
            continue;
        }
        let rest = &text[index + needle.len()..];
        if let Some(close) = rest.find('"') {
            found.insert(rest[..close].to_string());
        }
    }
    found
}

#[test]
fn the_shipped_binary_carries_engine_and_embedded_frontend() {
    let root = repo_root();
    let mut problems = Vec::new();

    // 1. 进程内引擎在场（引擎线程名是引擎运行时的必然足迹）。
    let binary = std::fs::read(envboard_binary()).expect("built binary must be readable");
    const ENGINE_MARKER: &[u8] = b"envboard-engine-";
    if !binary
        .windows(ENGINE_MARKER.len())
        .any(|w| w == ENGINE_MARKER)
    {
        problems.push("二进制里找不到进程内引擎的线程名标记 —— 发布的还是上一代形态？".to_string());
    }

    // 2. 产品身份在场：窗口标题（tauri.conf.json）与入口页 <title> 同值。
    if !binary
        .windows("envboard 环境代理工作台".len())
        .any(|window| window == "envboard 环境代理工作台".as_bytes())
    {
        problems.push("二进制里找不到前端入口页标题 —— 产品身份漂移？".to_string());
    }

    // 3. dist 形态（能走到本测试说明 build.rs 的前端构建已过 —— 产物在位）。
    //    最阴险的坏法是"截断"：头部完好、尾部消失，页面白屏却处处绿灯。
    let dist_dir = root.join("frontend/dist");
    let dist_index = std::fs::read_to_string(dist_dir.join("index.html"))
        .expect("内嵌的 frontend/dist/index.html 必须存在（build.rs 驱动 pnpm build 的产物）");
    let assets_dir = dist_dir.join("assets");
    let mut js_files: Vec<_> = std::fs::read_dir(&assets_dir)
        .expect("frontend/dist/assets 必须存在")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "js"))
        .collect();
    let css_files: Vec<_> = std::fs::read_dir(&assets_dir)
        .expect("frontend/dist/assets 必须存在")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "css"))
        .collect();
    if js_files.is_empty() || css_files.is_empty() {
        problems.push("dist/assets 缺 JS 或 CSS 产物 —— 构建不完整？".to_string());
    }
    js_files.sort();
    let js_bundle = js_files
        .first()
        .map(|path| std::fs::read_to_string(path).expect("dist js 可读"))
        .unwrap_or_default();
    let css_bundle = css_files
        .first()
        .map(|path| std::fs::read_to_string(path).expect("dist css 可读"))
        .unwrap_or_default();
    if js_bundle.len() < 400_000 {
        problems.push(format!(
            "dist JS 只有 {} B —— 疑似截断（HeroUI+React 基线 400KB+）",
            js_bundle.len()
        ));
    }
    if css_bundle.len() < 300_000 {
        problems.push(format!(
            "dist CSS 只有 {} B —— 疑似截断（Tailwind+HeroUI 基线 300KB+）",
            css_bundle.len()
        ));
    }

    // 资产形态 = 外链（严格 CSP 的前提）：index.html 引 hashed 资源、无内联脚本、无 CDN。
    if !dist_index.contains("/assets/") {
        problems.push(
            "dist/index.html 没有引用 /assets/ 外链 —— 资产形态漂移（CSP 会拒绝内联）".to_string(),
        );
    }
    if dist_index.contains("<script>") {
        problems.push(
            "dist/index.html 出现内联 <script> —— 严格 CSP 下静默失效，且违反资产纪律".to_string(),
        );
    }
    if dist_index.contains("src=\"http") || dist_index.contains("href=\"http") {
        problems.push("dist/index.html 引用外部 CDN —— 离线单二进制的前提被破坏".to_string());
    }

    // 接线两端各钉一枚：前端连流（IPC 命令名）+ 关键交互文案在 bundle 里。
    for (needle, where_) in [
        ("工作台导航", "dist js"),
        ("watch_debug", "dist js"),
        ("watch_environments", "dist js"),
        ("此操作不可撤销", "dist js"),
        // 壳加固 JS 守卫（ADR-009）；真机行为另由 cdp-walk 断言钉。
        ("contextmenu", "dist js"),
    ] {
        if !js_bundle.contains(needle) {
            problems.push(format!("{where_} 缺 {needle:?} —— 前端接线/交互契约退场"));
        }
    }

    // 规范：明亮主题禁纯黑（v1 的教训：令牌缺席时 fallback 悄悄生效）。
    if css_bundle.contains("rgba(0, 0, 0") {
        problems.push("dist css 出现纯黑 rgba(0, 0, 0, …) —— 规范禁纯黑".to_string());
    }

    // "导航 ↔ 视图主体一一对应"：data.ts 的 ViewKey、SideBar 的 NAV keys、
    // Workbench 的 `view === "…"` 分支三个集合必须相等 —— 加视图漏任何一处，
    // 点它必然白屏（旧三件套时代真坏过）。
    let view_keys = union_literals(
        &std::fs::read_to_string(root.join("frontend/src/data.ts")).expect("data.ts"),
        "export type ViewKey",
    );
    let nav_keys = key_literals(
        &std::fs::read_to_string(root.join("frontend/src/SideBar.tsx")).expect("SideBar.tsx"),
        "const NAV",
    );
    let workbench_src =
        std::fs::read_to_string(root.join("frontend/src/Workbench.tsx")).expect("Workbench.tsx");
    let body_keys: std::collections::BTreeSet<String> = workbench_src
        .match_indices("view === \"")
        .filter_map(|(index, _)| {
            let rest = &workbench_src[index + "view === \"".len()..];
            rest.split('"').next().map(str::to_string)
        })
        .collect();
    if view_keys != nav_keys {
        problems.push(format!(
            "ViewKey {view_keys:?} 与侧栏导航 {nav_keys:?} 不一致 —— 加视图必须三处同改"
        ));
    }
    if view_keys != body_keys {
        problems.push(format!(
            "ViewKey {view_keys:?} 与 Workbench 渲染分支 {body_keys:?} 不一致 —— 漏分支的视图点开即白屏"
        ));
    }

    // 4. 壳接线：两条流命令必须真的注册进 generate_handler（漏注册 = 界面连不上流）。
    //    轨迹流随 ADR-012 退场：注册表与 dist 里都不得再出现它（防止死链路复活）。
    let lib_rs =
        std::fs::read_to_string(root.join("crates/app/desktop/src/lib.rs")).expect("lib.rs");
    for command in ["stream::watch_environments", "stream::watch_debug"] {
        if !lib_rs.contains(command) {
            problems.push(format!(
                "lib.rs 的 generate_handler 缺 {command} —— 实时流退场"
            ));
        }
    }
    if lib_rs.contains("watch_trajectory") {
        problems.push("lib.rs 仍注册了 watch_trajectory —— 轨迹流已随 ADR-012 退场".to_string());
    }
    if js_bundle.contains("watch_trajectory") {
        problems
            .push("dist js 仍带着 watch_trajectory 接线 —— 前端轨迹流绑定没有删干净".to_string());
    }
    // mock 传输（dev-tools）只在 VITE_TRANSPORT=mock 的 dev server 里经 alias 引入；
    // 发布产物里出现标记 = shim 漏进了生产构建（假引擎接管真 IPC 是最坏漂移）。
    if js_bundle.contains("__envboardMock") {
        problems
            .push("dist js 带着 __envboardMock 标记 —— mock 传输 shim 漏进了发布构建".to_string());
    }

    // boot 自检屏只属于「进程的首次窗口」（ADR-012）：壳注入端与前端读取端必须
    // 成对存在 —— 只丢一边的坏法都是静默的（重建窗重播 1s 动画，或标记没人消费）。
    let tray_rs =
        std::fs::read_to_string(root.join("crates/app/desktop/src/tray.rs")).expect("tray.rs");
    if !tray_rs.contains("__envboardRecreated") {
        problems.push(
            "tray.rs 不再注入 __envboardRecreated —— 托盘重建的窗口会重播 boot 自检屏".to_string(),
        );
    }
    if !js_bundle.contains("__envboardRecreated") {
        problems
            .push("dist js 不再读取 __envboardRecreated —— 壳注入的重建标记没人消费".to_string());
    }

    // CSP 钉在 tauri.conf.json：严格档 + react-aria 样式 hash + ipc 通道。
    let conf = std::fs::read_to_string(root.join("crates/app/desktop/tauri.conf.json"))
        .expect("tauri.conf.json");
    for needle in [
        "sha256-38RhXrc7EdReTKsOm23ZPOCUgniTUUcjky8QOOrQx6o=",
        "ipc: http://ipc.localhost",
    ] {
        if !conf.contains(needle) {
            problems.push(format!("tauri.conf.json 的 CSP 缺 {needle:?}"));
        }
    }
    if conf.contains("unsafe-inline") {
        problems.push("tauri.conf.json 的 CSP 出现 unsafe-inline —— 严格档被破坏".to_string());
    }

    if !problems.is_empty() {
        println!("artifact FAILED ({} problem(s)):", problems.len());
        for problem in &problems {
            println!("  {problem}");
        }
        panic!("artifact FAILED ({} problem(s))", problems.len());
    }

    let size = std::fs::metadata(envboard_binary())
        .map(|meta| meta.len())
        .unwrap_or(0);
    println!(
        "artifact OK (binary {} KiB、引擎与内嵌前端在场、导航 ↔ 视图主体 {} 对)",
        size / 1024,
        view_keys.len()
    );
}

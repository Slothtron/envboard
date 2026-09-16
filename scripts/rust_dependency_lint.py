#!/usr/bin/env python3
"""Rust 侧的依赖方向与纯度门禁 —— 分层不变量与 Rust 布局。

为什么需要它：Cargo 本身只禁止**循环**依赖，"domain 依赖 manager" 这种**向上**依赖
它完全允许。而分层一旦被这样穿破，core 就不再宿主无关，"加能力不用改适配器"这条
也跟着失效。所以这里把允许的边写成一张表，越界即失败。

命名规则（"发包标识不得进代码身份"）**不在这里检查** —— 那是 `naming_lint.py` 的
职责，它已经扫描全部 `.toml` 内容，重复实现只会多一处要维护的 token 字面量。

顺带守住三条工程约束（都是"忘了就出问题、但没有别的地方会发现"的类型）：

1. **纯逻辑 crate 不得依赖运行时库**：`core-api` / `domain` / `rules` 是纯逻辑，
   一旦有人为了省事在里面 `tokio::spawn` 或 `libc::flock`，它们就不再可测、可移植。
2. **workspace 根唯一**：包根 virtual manifest 是唯一的 `[workspace]`，
   `core/rs/` 下不允许出现第二份。
3. **`Cargo.lock` 与 `rust-toolchain.toml` 必须在位**：前者是可复现构建的前提
   （可复现构建的前提），后者是运行时下限的声明。
"""

from __future__ import annotations

import pathlib
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent
CRATES_DIR = ROOT / "core" / "rs" / "crates"

#: 允许的内部依赖边（"谁 → 可以依赖谁"）。不在表里的 crate 视为未登记。
#:
#: 这张表管的是**会进产物的依赖**（`dependencies` / `build-dependencies`）。
ALLOWED_INTERNAL: dict[str, set[str]] = {
    "envboard-core-api": set(),  # 根：契约词汇，不依赖任何内部 crate
    "envboard-domain": {"envboard-core-api"},
    "envboard-rules": {"envboard-core-api"},
    "envboard-core-fake": {"envboard-core-api", "envboard-rules"},
    "envboard-manager": {"envboard-core-api", "envboard-domain", "envboard-rules"},
    "envboard-cli": {
        "envboard-core-api",
        "envboard-domain",
        "envboard-core-fake",
        "envboard-core-mitmproxy",
        "envboard-manager",
        "envboard-web",
    },
    # 工作台：只认识管理器的公开 API，不认识具体 core（换 core 不影响它）
    "envboard-web": {"envboard-core-api", "envboard-domain", "envboard-manager"},
    "envboard-core-mitmproxy": {"envboard-core-api"},
    "envboard-contract-tests": {"envboard-core-api", "envboard-domain", "envboard-rules"},
}

#: 只允许出现在 `dev-dependencies` 里的额外边。
#:
#: 分层不变量约束的是**产物的依赖图**；测试目标用测试替身（`core-fake`）不但无害，
#: 正是"管理器测试不需要装 mitmproxy"要的效果。把这类边单列出来，
#: 比把 `core-fake` 加进上面那张表更清楚：它没有被发布出去。
ALLOWED_DEV_INTERNAL: dict[str, set[str]] = {
    "envboard-web": {"envboard-core-fake"},
    "envboard-manager": {"envboard-core-fake"},
    # mitmproxy core 的实机测试要经管理器跑完整编排（真进程、真端口、真改写）
    "envboard-core-mitmproxy": {"envboard-manager", "envboard-domain", "envboard-rules"},
    "envboard-contract-tests": {"envboard-core-api", "envboard-domain", "envboard-rules"},
}

#: 纯逻辑 crate：只允许标准库 + 序列化/错误这类"无副作用"的依赖。
PURE_CRATES = {"envboard-core-api", "envboard-domain", "envboard-rules"}

#: 纯逻辑 crate 禁止出现的运行时/系统依赖。
IMPURE_DEPS = {
    "tokio",
    "libc",
    "axum",
    "hyper",
    "hyper-util",
    "reqwest",
    "tower",
    "tower-http",
    "mio",
    "socket2",
}

def internal_deps(manifest: dict, sections: tuple[str, ...]) -> set[str]:
    """收集指定 section 里依赖的内部 crate 名。"""
    found: set[str] = set()
    for section in sections:
        for name, spec in (manifest.get(section) or {}).items():
            has_path = isinstance(spec, dict) and "path" in spec
            # 内部 crate 有两种写法：带 path，或走 workspace 依赖（指向同一 workspace）
            if has_path or name.startswith("envboard-"):
                found.add(name)
    return found


def external_deps(manifest: dict, sections: tuple[str, ...]) -> set[str]:
    found: set[str] = set()
    for section in sections:
        for name in manifest.get(section) or {}:
            if not name.startswith("envboard-"):
                found.add(name)
    return found


PROD_SECTIONS = ("dependencies", "build-dependencies")
DEV_SECTIONS = ("dev-dependencies",)


def main() -> int:
    problems: list[str] = []

    if not CRATES_DIR.is_dir():
        print(f"rust-dependency-lint FAILED: no crates at {CRATES_DIR}")
        return 1

    # workspace 根唯一（包根 virtual manifest）
    workspace_manifests = []
    for manifest in ROOT.rglob("Cargo.toml"):
        if "target" in manifest.parts:
            continue
        try:
            data = tomllib.loads(manifest.read_text(encoding="utf-8"))
        except tomllib.TOMLDecodeError as error:
            problems.append(f"{manifest.relative_to(ROOT)}: invalid TOML: {error}")
            continue
        if data.get("workspace") is not None:
            workspace_manifests.append(manifest.relative_to(ROOT))
    if workspace_manifests != [pathlib.Path("Cargo.toml")]:
        problems.append(
            f"the workspace root must be the single package-root Cargo.toml, found: "
            f"{[str(path) for path in workspace_manifests]}"
        )

    for required in ("Cargo.lock", "rust-toolchain.toml"):
        if not (ROOT / required).exists():
            problems.append(
                f"{required} is missing: it is a required artifact "
                "(`Cargo.lock` for reproducible builds, `rust-toolchain.toml` for the lower bound)"
            )

    seen: set[str] = set()
    for crate_dir in sorted(path for path in CRATES_DIR.iterdir() if path.is_dir()):
        manifest_path = crate_dir / "Cargo.toml"
        if not manifest_path.exists():
            problems.append(f"{crate_dir.name}/ has no Cargo.toml")
            continue
        manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
        name = manifest.get("package", {}).get("name")
        if not isinstance(name, str):
            problems.append(f"{crate_dir.name}: missing package.name")
            continue
        seen.add(name)

        if name not in ALLOWED_INTERNAL:
            problems.append(
                f"{name}: not registered in scripts/rust_dependency_lint.py — add its allowed "
                "dependency edges (and its layer) when introducing a crate"
            )
            continue

        allowed = ALLOWED_INTERNAL[name]
        allowed_dev = allowed | ALLOWED_DEV_INTERNAL.get(name, set())
        for dependency in sorted(internal_deps(manifest, PROD_SECTIONS)):
            if dependency not in allowed:
                problems.append(
                    f"{name} depends on {dependency}, which violates the layering "
                    f"(allowed: {sorted(allowed) or 'nothing'})"
                )
        for dependency in sorted(internal_deps(manifest, DEV_SECTIONS)):
            if dependency not in allowed_dev:
                problems.append(
                    f"{name} dev-depends on {dependency}, which is not a declared test-only edge "
                    f"(allowed dev edges: {sorted(allowed_dev - allowed) or 'none'})"
                )
        if name in PURE_CRATES:
            impure = sorted(external_deps(manifest, PROD_SECTIONS) & IMPURE_DEPS)
            if impure:
                problems.append(
                    f"{name} is a pure-logic crate but depends on {impure}: core must not reach "
                    "the runtime directly (core must go through a port)"
                )

    missing = sorted(set(ALLOWED_INTERNAL) - seen)
    if missing:
        problems.append(f"registered but missing on disk: {missing}")

    if problems:
        print("rust-dependency-lint FAILED:")
        for problem in problems:
            print(f"  - {problem}")
        return 1

    print(
        f"rust-dependency-lint OK ({len(seen)} crates, layering intact; "
        f"pure: {sorted(PURE_CRATES)})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

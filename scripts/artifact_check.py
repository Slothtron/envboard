#!/usr/bin/env python3
"""发布物内容校验（发布物必须**只含预期文件**）。

v1 用 `pack_check.py` 校验 wheel 内容；M4 之后本包不再发布 Python 分发物，
发布物是 **`envboard` 二进制**（外加它运行时物化出来的注入器）。所以校验对象换成：

1. **必需要在位**：`Cargo.lock`（可复现构建）、`rust-toolchain.toml`（运行时下限）、
   `LICENSE`、`README.md`、`CHANGELOG.md`；
2. **v1 的残留必须为零**：`src/`、`addons/`、`pyproject.toml`、`tests/` 都不该再存在 ——
   否则会出现"两套实现并存、但只有一套在用"的迷惑状态；
3. **注入器必须是单文件、且被二进制内嵌**：源码里不得出现对 v1 模块的 import，
   二进制里必须能找到它的标记字符串（`include_str!` 真的嵌进去了）；
4. **产物目录不得入库**：`target/` 必须在 `.gitignore` 里。
"""

from __future__ import annotations

import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

REQUIRED = ["Cargo.lock", "rust-toolchain.toml", "LICENSE", "README.md", "CHANGELOG.md"]
# 这几条**故意**不存在：它们断言 v1 的产物已清干净。
REMOVED_IN_V2 = [
    "src",
    "addons",
    "pyproject.toml",
    "tests",
    "scripts/pack_check.py",  # doc-scope-lint: allow
]
INJECTOR = ROOT / "adapters" / "mitmproxy" / "envboard_mitmproxy.py"
BINARY = ROOT / "target" / "debug" / "envboard"
#: 注入器里的一个独特字符串：二进制里能找到它，就说明源码真的被 include_str! 嵌入了。
EMBED_MARKER = b"envboard injector ready"


def main() -> int:
    problems: list[str] = []

    for name in REQUIRED:
        if not (ROOT / name).exists():
            problems.append(f"missing required artifact: {name}")

    for name in REMOVED_IN_V2:
        if (ROOT / name).exists():
            problems.append(
                f"{name} should have been removed in v2 — a leftover makes it ambiguous "
                "which implementation is authoritative"
            )

    if not INJECTOR.exists():
        problems.append(f"injector is missing: {INJECTOR.relative_to(ROOT)}")
    else:
        text = INJECTOR.read_text(encoding="utf-8")
        for banned in ("from envboard.", "import envboard", "mitmproxy_rs"):
            if banned in text:
                problems.append(
                    f"the injector must be self-contained (single file, stdlib + mitmproxy); "
                    f"found {banned!r}"
                )

    if not BINARY.exists():
        problems.append(
            "target/debug/envboard is missing — run `cargo build` before the artifact check"
        )
    elif INJECTOR.exists():
        blob = BINARY.read_bytes()
        if EMBED_MARKER not in blob:
            problems.append(
                "the binary does not contain the injector source (include_str! did not take "
                "effect?) — the release artifact would be unable to materialise it"
            )

    gitignore = (ROOT / ".gitignore").read_text(encoding="utf-8")
    if "target/" not in gitignore:
        problems.append("target/ must be git-ignored")

    if problems:
        print("artifact-check FAILED:")
        for problem in problems:
            print(f"  - {problem}")
        return 1

    size = BINARY.stat().st_size if BINARY.exists() else 0
    print(
        f"artifact-check OK (binary {size // 1024} KiB, injector {INJECTOR.stat().st_size} bytes "
        f"embedded, {len(REQUIRED)} required artifacts present, 0 v1 leftovers)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

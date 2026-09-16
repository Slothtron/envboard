#!/usr/bin/env python3
"""命名门禁 —— 把"发包标识不进代码"变成机器可验证的规则。

**M4 收敛后的形态**：本包不再向任何 registry 发布 Python 分发物（v1 的 pip 包已移除，
Python 这一侧只剩一个被二进制内嵌的单文件注入器）。所以规则从"分包标识只允许出现在
manifest 里"收紧成更简单的一条：

| 规则 | 内容 |
|---|---|
| 路径 | 任何目录名 / 文件名都不得含 `TOKEN` |
| 代码内容 | ``.py`` / ``.sh`` / ``.rs`` / ``.js`` / ``.css`` / ``.html`` / ``.json`` / ``.toml`` / ``.yml`` 里不得出现 `TOKEN`，唯一例外是**本脚本自身**（它必须写出这个 token 才能检查它） |
| 文档 | ``.md`` 不扫描 —— 文档需要能够命名这个约定 |

保留这个门禁的意义：将来任何新宿主适配器都可能重新引入"分发身份"，而"代码身份不得
含发包标识"这条纪律与仓库形态无关：代码身份属于代码，发布身份属于 manifest。
"""

from __future__ import annotations

import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

TOKEN = "slothtron"
SELF = "scripts/naming_lint.py"

#: 允许在**代码内容**里出现 TOKEN 的文件。只有本规则自身。
CONTENT_ALLOWLIST = {SELF}

#: 会被扫描内容的代码/配置后缀。``.md`` 刻意不在其中（见模块 docstring）。
CODE_SUFFIXES = {
    ".py",
    ".sh",
    ".bash",
    ".rs",
    ".js",
    ".mjs",
    ".css",
    ".html",
    ".json",
    ".toml",
    ".yml",
    ".yaml",
}

#: 不进入扫描的目录（产物与依赖）。
SKIP_DIRS = {".git", "target", "node_modules", "__pycache__", ".zvec-grep"}


def scan_paths() -> list[str]:
    problems: list[str] = []
    for path in sorted(ROOT.rglob("*")):
        if any(part in SKIP_DIRS for part in path.relative_to(ROOT).parts):
            continue
        if TOKEN in path.name:
            problems.append(
                f"{path.relative_to(ROOT)}: path contains {TOKEN!r} "
                "(it belongs in a manifest, never in a path)"
            )
    return problems


def scan_contents() -> list[str]:
    problems: list[str] = []
    for path in sorted(ROOT.rglob("*")):
        if not path.is_file() or path.suffix not in CODE_SUFFIXES:
            continue
        relative = path.relative_to(ROOT)
        if any(part in SKIP_DIRS for part in relative.parts):
            continue
        if str(relative) in CONTENT_ALLOWLIST:
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (UnicodeDecodeError, OSError):
            continue
        for number, line in enumerate(text.splitlines(), start=1):
            if TOKEN in line:
                problems.append(
                    f"{relative}:{number}: code mentions {TOKEN!r}; only "
                    f"{sorted(CONTENT_ALLOWLIST)} may mention it"
                )
    return problems


def count_scanned() -> tuple[int, int]:
    files = paths = 0
    for path in ROOT.rglob("*"):
        relative = path.relative_to(ROOT)
        if any(part in SKIP_DIRS for part in relative.parts):
            continue
        paths += 1
        if path.is_file() and path.suffix in CODE_SUFFIXES:
            files += 1
    return paths, files


def main() -> int:
    problems = scan_paths() + scan_contents()
    if problems:
        print("naming-lint FAILED:")
        for problem in problems:
            print(f"  - {problem}")
        return 1
    paths, files = count_scanned()
    print(
        f"naming-lint OK ({paths} paths scanned, {files} code files clean; "
        f"'slothtron' confined to {sorted(CONTENT_ALLOWLIST)})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""语法门禁：对全部 Python 源码做 AST 解析，不落任何 ``__pycache__`` 产物。"""

from __future__ import annotations

import ast
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
TARGETS = ["src", "addons", "scripts", "tests", "core"]


def main() -> int:
    files = 0
    problems: list[str] = []
    for target in TARGETS:
        root = ROOT / target
        if not root.exists():
            continue
        for path in sorted(root.rglob("*.py")):
            files += 1
            try:
                ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
            except SyntaxError as exc:
                problems.append(f"{path.relative_to(ROOT)}:{exc.lineno}: {exc.msg}")
    if problems:
        print("compile-check FAILED:")
        for problem in problems:
            print(f"  - {problem}")
        return 1
    print(f"compile-check OK ({files} files)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

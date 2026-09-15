#!/usr/bin/env python3
"""依赖方向门禁 —— AGENTS.md §1 不变量 1/3/5 的机器可验证版本。

分层与允许的 import 顶层名：

===========  ==========================================================
层            允许
===========  ==========================================================
``core/``     仅标准库
``infra/``    标准库、``mitmproxy_rs``，以及同包 ``core``
``adapter/``  标准库、``mitmproxy``、``mitmproxy_rs``、``tornado``，以及同包 ``core`` / ``infra``
===========  ==========================================================

同时禁止反向依赖：``core`` 不得 import ``infra``/``adapter``，``infra`` 不得 import ``adapter``。
"""

from __future__ import annotations

import ast
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
PKG = ROOT / "src" / "envboard"
PKG_NAME = "envboard"

ALLOWED_EXTERNAL: dict[str, set[str]] = {
    "root": set(),
    "core": set(),
    "infra": {"mitmproxy_rs"},
    "adapter": {"mitmproxy", "mitmproxy_rs", "tornado"},
}

ALLOWED_INTERNAL: dict[str, set[str]] = {
    # 同层 import 是正常的包内组织方式；这里约束的是**跨层方向**：
    # core 不得依赖 infra/adapter，infra 不得依赖 adapter。
    "root": {"core", "infra", "adapter"},
    "core": {"core"},
    "infra": {"core", "infra"},
    "adapter": {"core", "infra", "adapter"},
}


def layer_of(path: pathlib.Path) -> str:
    rel = path.relative_to(PKG)
    return rel.parts[0] if len(rel.parts) > 1 else "root"


def top_level_of(node: ast.Import | ast.ImportFrom, path: pathlib.Path) -> list[str]:
    """返回该 import 语句引用的顶层模块名列表（已剥离包自身前缀）。"""
    if isinstance(node, ast.Import):
        return [alias.name.split(".")[0] for alias in node.names]

    module = node.module or ""
    if node.level:  # 相对 import：from ..core.ports import X
        rel = path.relative_to(PKG)
        parts = list(rel.parts[:-1])  # 去掉文件名
        drop = node.level - 1
        if drop:
            parts = parts[:-drop] if drop <= len(parts) else []
        prefix = ".".join(parts)
        module = f"{prefix}.{module}" if module else prefix
    if module == PKG_NAME:
        return []
    if module.startswith(PKG_NAME + "."):
        return [module[len(PKG_NAME) + 1 :].split(".")[0]]
    return [module.split(".")[0]] if module else []


def check() -> list[str]:
    problems: list[str] = []
    if not PKG.is_dir():
        return [f"package directory not found: {PKG}"]

    for path in sorted(PKG.rglob("*.py")):
        layer = layer_of(path)
        allowed_ext = ALLOWED_EXTERNAL.get(layer)
        allowed_int = ALLOWED_INTERNAL.get(layer)
        if allowed_ext is None:
            problems.append(f"{path}: unknown layer {layer!r}")
            continue
        try:
            tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        except SyntaxError as exc:
            problems.append(f"{path}: syntax error: {exc}")
            continue

        for node in ast.walk(tree):
            if not isinstance(node, (ast.Import, ast.ImportFrom)):
                continue
            for top in top_level_of(node, path):
                if not top or top == "__future__":
                    continue
                if top == PKG_NAME:
                    continue
                if top in sys.stdlib_module_names:
                    continue
                if top in ALLOWED_INTERNAL:
                    if top not in allowed_int:
                        problems.append(
                            f"{path.relative_to(ROOT)}: layer {layer!r} must not import "
                            f"internal layer {top!r} (allowed: {sorted(allowed_int) or 'none'})"
                        )
                    continue
                if top not in allowed_ext:
                    problems.append(
                        f"{path.relative_to(ROOT)}: layer {layer!r} must not import {top!r} "
                        f"(allowed external: {sorted(allowed_ext) or 'none'})"
                    )
    return problems


def main() -> int:
    problems = check()
    if problems:
        print("dependency-lint FAILED:")
        for problem in problems:
            print(f"  - {problem}")
        return 1
    print(f"dependency-lint OK ({len(list(PKG.rglob('*.py')))} modules, layering intact)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""命名门禁 —— 把"发包标识不进代码路径"变成机器可验证的规则。

规则出处见 `README.md` 的「命名模型」一节；本脚本是它的机器可验证实现。

规则：

1. **路径**：任何目录名或文件名都不得包含 ``slothtron``（无例外）。
2. **代码内容**：``.py`` / ``.sh`` / ``.html`` / ``.json`` / ``.toml`` / ``.yml`` 等
   代码与配置文件里不得出现 ``slothtron``，只有两个例外 ——
   ``pyproject.toml``（分发名，发布身份）与 ``scripts/naming_lint.py``（本规则自身）。
   ``.md`` **不扫描**：文档需要能够命名这个约定，包括 README 的命名模型一节。
3. **分发名**：``pyproject.toml`` 的 ``name`` 必须以 ``slothtron-`` 开头
   （规则的另一半：发包标识**必须**留在 manifest 里）。
4. **导入名与位置**：wheel 声明的包目录**必须**存在、**必须**声明为 ``src/<name>``
   （本项目用 src 布局）、**必须**只有一个，且 ``<name>`` 不得含 ``slothtron``、
   必须是合法的小写标识符。删掉 ``src/`` 里那层 ``<name>`` 会让各层失去共同父包，
   跨层相对 import 直接崩塌 —— 所以这一层是导入包名本体，不是冗余。
"""

from __future__ import annotations

import pathlib
import re
import tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent

TOKEN = "slothtron"
SELF = "scripts/naming_lint.py"

#: 允许在**代码内容**里出现 TOKEN 的文件。
CONTENT_ALLOWLIST = {"pyproject.toml", SELF}

#: 会被扫描内容的代码/配置后缀。``.md`` 刻意不在其中（见模块 docstring 规则 2）。
CODE_SUFFIXES = {
    ".py",
    ".sh",
    ".bash",
    ".js",
    ".ts",
    ".json",
    ".toml",
    ".yml",
    ".yaml",
    ".cfg",
    ".ini",
    ".html",
}

#: 分发名必须以此开头。
DIST_PREFIX = "slothtron-"

SKIP_DIRS = {".git", "__pycache__", "node_modules", ".venv", "dist", "build", ".mypy_cache"}


def iter_files() -> list[pathlib.Path]:
    out: list[pathlib.Path] = []
    for path in ROOT.rglob("*"):
        if any(part in SKIP_DIRS for part in path.parts):
            continue
        if path.is_file():
            out.append(path)
    return sorted(out)


def check_paths(files: list[pathlib.Path]) -> list[str]:
    problems: list[str] = []
    for path in files:
        rel = path.relative_to(ROOT)
        for part in rel.parts:
            if TOKEN in part:
                problems.append(
                    f"path contains the publishing token: {rel} "
                    f"(rename it; {TOKEN} belongs in pyproject.toml only)"
                )
                break
    return problems


def check_contents(files: list[pathlib.Path]) -> list[str]:
    problems: list[str] = []
    for path in files:
        rel = path.relative_to(ROOT).as_posix()
        if rel in CONTENT_ALLOWLIST or path.suffix.lower() not in CODE_SUFFIXES:
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="ignore")
        except OSError as exc:
            problems.append(f"cannot read {rel}: {exc}")
            continue
        if TOKEN in text:
            line = next((i for i, l in enumerate(text.splitlines(), 1) if TOKEN in l), 1)
            problems.append(
                f"{rel}:{line}: code mentions {TOKEN!r}; "
                f"only {sorted(CONTENT_ALLOWLIST)} may mention it"
            )
    return problems


def check_manifest() -> list[str]:
    problems: list[str] = []
    manifest = ROOT / "pyproject.toml"
    if not manifest.is_file():
        return ["pyproject.toml is missing"]

    data = tomllib.loads(manifest.read_text(encoding="utf-8"))
    name = data.get("project", {}).get("name", "")
    if not name.startswith(DIST_PREFIX):
        problems.append(
            f"pyproject.toml: distribution name {name!r} must start with {DIST_PREFIX!r} "
            "(the publishing identifier belongs here)"
        )

    targets = data.get("tool", {}).get("hatch", {}).get("build", {}).get("targets", {})
    declared = targets.get("wheel", {}).get("packages", [])
    if not declared:
        problems.append("pyproject.toml: [tool.hatch.build.targets.wheel] packages is empty")
        return problems

    if len(declared) != 1:
        problems.append(
            f"pyproject.toml: expected exactly one import package, got {declared!r} "
            "— layers must be subpackages of it, not sibling distributions"
        )

    for entry in declared:
        package_dir = ROOT / entry
        if not package_dir.is_dir():
            problems.append(f"pyproject.toml: declared package dir does not exist: {entry}")
            continue
        parts = pathlib.PurePosixPath(entry).parts
        if len(parts) != 2 or parts[0] != "src":
            problems.append(
                f"pyproject.toml: import package must be declared as 'src/<name>', "
                f"got {entry!r} — this project uses the src layout"
            )
        leaf = parts[-1]
        if TOKEN in leaf:
            problems.append(
                f"pyproject.toml: import package {leaf!r} must not contain {TOKEN!r}"
            )
        if not re.fullmatch(r"[a-z][a-z0-9_]*", leaf):
            problems.append(
                f"pyproject.toml: import package {leaf!r} must be a lowercase identifier"
            )
    return problems


def main() -> int:
    files = iter_files()
    problems = [*check_paths(files), *check_contents(files), *check_manifest()]
    if problems:
        print("naming-lint FAILED:")
        for problem in problems:
            print(f"  - {problem}")
        return 1

    code_files = sum(
        1
        for path in files
        if path.suffix.lower() in CODE_SUFFIXES
        and path.relative_to(ROOT).as_posix() not in CONTENT_ALLOWLIST
    )
    print(
        f"naming-lint OK ({len(files)} files scanned, {code_files} code files clean; "
        f"{TOKEN!r} confined to {sorted(CONTENT_ALLOWLIST)})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

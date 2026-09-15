#!/usr/bin/env python3
"""打包白名单检查（对应 AGENTS.md §7 的 ``npm pack --dry-run``）。

断言将要发布的内容"只含预期文件"，且不含任何真实凭据或本地状态。
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
PKG = ROOT / "src" / "envboard"

REQUIRED = [
    "pyproject.toml",
    "README.md",
    "LICENSE",
    "CHANGELOG.md",
    "addons/envboard.py",
    "src/envboard/__init__.py",
    "src/envboard/adapter/addon.py",
    "src/envboard/adapter/web.py",
    "src/envboard/core/model.py",
    "src/envboard/core/registry.py",
    "src/envboard/core/mapping.py",
    "src/envboard/core/resolver.py",
    "src/envboard/core/ports.py",
    "src/envboard/core/errors.py",
    "src/envboard/core/rules.py",
    "src/envboard/infra/dns_forward.py",
    "src/envboard/infra/rules_store.py",
    "src/envboard/infra/store.py",
    "src/envboard/infra/system_dns.py",
    "src/envboard/web/index.html",
    "src/envboard/web/app.js",
    "scripts/dependency_lint.py",
    "scripts/naming_lint.py",
    "core/spec/errors.md",
    "core/spec/capabilities.md",
]

#: 任何一项出现在仓库里都说明"跑过一次"，绝不能进发布物。
FORBIDDEN_DIR_NAMES = {"__pycache__", ".pytest_cache", ".mypy_cache", ".ruff_cache"}
FORBIDDEN_FILE_SUFFIXES = {".pyc", ".pyo", ".tmp", ".log"}
FORBIDDEN_FILE_PREFIXES = (".envboard-",)
FORBIDDEN_PATTERNS = [
    (re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----"), "embedded private key"),
    (re.compile(r"\bAKIA[0-9A-Z]{16}\b"), "AWS access key id"),
    (re.compile(r"\bsk-[A-Za-z0-9]{20,}\b"), "OpenAI-style secret"),
    (re.compile(r"\bgh[pousr]_[A-Za-z0-9]{20,}\b"), "GitHub token"),
]
SCAN_SUFFIXES = {".py", ".html", ".toml", ".md", ".json", ".sh", ".yml", ".yaml"}


def find_forbidden_artefacts() -> list[str]:
    """返回污染路径（目录只报目录本身，避免逐文件刷屏）。"""
    offenders: set[str] = set()
    for path in ROOT.rglob("*"):
        if ".git" in path.parts:
            continue
        if path.is_dir():
            if path.name in FORBIDDEN_DIR_NAMES:
                offenders.add(str(path.relative_to(ROOT)))
            continue
        if path.suffix in FORBIDDEN_FILE_SUFFIXES or path.name.startswith(
            FORBIDDEN_FILE_PREFIXES
        ):
            offenders.add(str(path.relative_to(ROOT)))
    return sorted(offenders)


def check_dashboard_is_csp_safe() -> list[str]:
    """Dashboard 的 JS 必须是同源外部文件，禁止内联 <script>。

    mitmweb 下发的 CSP 是 ``default-src 'self'; ...; style-src 'self' 'unsafe-inline'``：
    没有给 ``script-src`` 单独开口，于是回落到 ``'self'``，**内联 <script> 会被浏览器
    拒绝执行** —— 页面照样 200、照样渲染，只是一行 JS 都不跑。这种故障 curl 查不出来，
    只有真浏览器会暴露，所以这里做成静态门禁。
    """
    problems: list[str] = []
    page = PKG / "web" / "index.html"
    if not page.is_file():
        return [f"dashboard page missing: {page.relative_to(ROOT)}"]

    text = page.read_text(encoding="utf-8")
    for match in re.finditer(r"<script\b[^>]*>", text, re.IGNORECASE):
        tag = match.group(0)
        if "src=" not in tag.lower():
            line = text[: match.start()].count("\n") + 1
            problems.append(
                f"{page.relative_to(ROOT)}:{line}: inline <script> is blocked by mitmweb's "
                "CSP (default-src 'self') — serve it as a same-origin file instead"
            )
    if "app.js" not in text:
        problems.append(
            f"{page.relative_to(ROOT)}: does not reference app.js — the dashboard's "
            "JavaScript must be served as an external same-origin asset"
        )
    return problems


def main() -> int:
    problems: list[str] = []

    for rel in REQUIRED:
        if not (ROOT / rel).is_file():
            problems.append(f"missing required file: {rel}")

    problems.extend(check_dashboard_is_csp_safe())

    for rel in find_forbidden_artefacts():
        problems.append(
            f"forbidden artefact present: {rel} (run `bash ci/verify.sh clean`)"
        )

    shipped: list[str] = []
    for base in (PKG, ROOT / "addons", ROOT / "core" / "spec"):
        if not base.exists():
            continue
        for path in sorted(base.rglob("*")):
            if not path.is_file():
                continue
            shipped.append(str(path.relative_to(ROOT)))
            if path.suffix.lower() not in SCAN_SUFFIXES:
                continue
            try:
                text = path.read_text(encoding="utf-8", errors="ignore")
            except OSError as exc:
                problems.append(f"cannot read {path.relative_to(ROOT)}: {exc}")
                continue
            for regex, label in FORBIDDEN_PATTERNS:
                if regex.search(text):
                    problems.append(f"{path.relative_to(ROOT)}: looks like a {label}")

    if problems:
        print("pack-check FAILED:")
        for problem in problems:
            print(f"  - {problem}")
        return 1
    print(f"pack-check OK ({len(shipped)} files would ship)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

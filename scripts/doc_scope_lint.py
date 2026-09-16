#!/usr/bin/env python3
"""包内文本自包含门禁 —— 禁止引用仓外的本地文档。

为什么需要它：对 clone 本仓的人，指向仓外文档的路径不存在、章节号无处可查，
那条线索读到就断。规范与契约必须"只看本仓就能完整理解"。

判据（三条，都可机械验证）：

1. **禁用词零命中**：`设计稿`、`仓外`、`AGENTS.md`、仓外文档的文件名
   （设计稿、开发计划、SOP、架构文档）。工作区规范文件也在禁列 —— 它在包仓库之外。
2. **指向本仓的文档路径必须存在**：`docs/…`、`core/spec/…`、`scripts/…` 这类引用
   一律要能在包内找到；否则多半是从仓外搬过来的（或写完没建）。
3. **`§` 引用必须带本仓锚点**：任何含 `§` 的行都要同时给出本仓内的文件
   （`core/spec/…`、`capabilities.md`、`README.md` …）或写"本文件" ——
   裸 `§7.2` 离开它出处的那份文档就无从解析。

不在禁止之列的（可复核的外部事实坐标，不是"仓外本地文档"）：宿主/上游源码坐标
（`mitmproxy/addons/tlsconfig.py:291`）、实测命令、版本号、协议编号、外部 URL。

用法：`python3 scripts/doc_scope_lint.py`（退出码非 0 即失败）。
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

#: 仓外本地文档的指称 —— 零命中。
BANNED_TOKENS = [
    "设计稿",
    "仓外",
    "envboard-v2-design",
    "envboard-design",
    "plugin-repo-architecture",
    "dsh-plugin-dev-SOP",
    "Octop插件开发SOP",
    # 工作区规范文件在包仓库之外，按文件名指称它同样不可解析
    "AGENTS.md",
]

#: 只检查这些后缀。
SUFFIXES = {
    ".md", ".rs", ".py", ".js", ".css", ".html", ".toml", ".sh",
    ".yml", ".yaml", ".json",
    # 部署工件也是"面向读者的文本"：unit 文件里的注释同样不许指向仓外
    ".service", ".conf", ".env",
}

#: **无后缀**的文本文件要单独列出来：`.gitignore` 的注释一样是"面向读者的文本"，
#: 而按后缀筛会把它们整类漏掉（实测漏掉过一条指向仓外规范文件的引用）。
EXTRA_FILES = {".gitignore", ".dockerignore"}

#: `.agents/` 是**本机工作材料**（设计稿 / 开发计划 / 实机验收转录），不入库、
#: 因而对 clone 本仓的人根本不存在 —— 它**不在自包含的适用范围里**，也不该被本门禁扫到。
#: 判据：门禁只审"入库文本"，而 `.agents/**` 被全局 ignore 覆盖。
SKIP_DIRS = {"target", "node_modules", ".git", "__pycache__", ".zvec-grep", ".agents"}
#: 门禁脚本自己要在词表里写出这些词，所以豁免它。
SELF = {"doc_scope_lint.py"}

#: 含 `§` 的行必须出现下列之一（本仓锚点）。
ANCHOR = re.compile(
    r"core/spec/[\w./-]+|README\.md|CHANGELOG\.md|docs/[\w./-]+"
    r"|capabilities\.md|rules\.md|errors\.md|fixtures/[\w./-]+|本文件"
)

#: 本仓内的文档/代码路径引用（要在包内真实存在）。
#: 注意 `.json` 必须排在 `.js` 前面，并且尾部不允许再接字母数字 —— 否则
#: `x/normalization.json` 会被切成 `x/normalization.js`（贪心匹配踩过）。
PATH_REF = re.compile(
    r"(?:docs|core/spec|core/rs|adapters|platforms|scripts|ci)/[\w./-]+\.(?:md|rs|py|json|js|css|html|toml|sh)(?![A-Za-z0-9])"
)

#: **历史记录**豁免"路径必须存在"这一条：CHANGELOG 的 Removed 段与 v1 的验收转录
#: 本来就要提到当时存在、现在已经删掉的文件。它们不是"指向别处的线索"。
HISTORICAL = {"CHANGELOG.md", "docs/acceptance/v0.1.0.md"}

#: 行内豁免标记：路径**故意**不存在（如"断言它已被删除"的清单）。
ALLOW_MARKER = "doc-scope-lint: allow"


def text_files() -> list[pathlib.Path]:
    found = []
    for path in sorted(ROOT.rglob("*")):
        if not path.is_file() or not (path.suffix in SUFFIXES or path.name in EXTRA_FILES):
            continue
        if any(part in SKIP_DIRS for part in path.parts):
            continue
        if path.name in SELF:
            continue
        found.append(path)
    return found


def main() -> int:
    problems: list[str] = []

    for path in text_files():
        relative = path.relative_to(ROOT)
        lines = path.read_text(encoding="utf-8").splitlines()
        for number, line in enumerate(lines, start=1):
            for token in BANNED_TOKENS:
                if token in line:
                    problems.append(
                        f"{relative}:{number}: 引用仓外本地文档（禁用词 {token!r}）："
                        f"{line.strip()[:100]}"
                    )
            if "§" in line and not ANCHOR.search(line):
                problems.append(
                    f"{relative}:{number}: `§` 引用没有本仓锚点（要写清是哪份本仓文件，"
                    f"或写「本文件」）：{line.strip()[:100]}"
                )
            for candidate in PATH_REF.findall(line):
                if str(relative) in HISTORICAL or ALLOW_MARKER in line:
                    continue
                if not (ROOT / candidate).exists():
                    problems.append(
                        f"{relative}:{number}: 指向的本仓路径不存在：{candidate}"
                    )

    if problems:
        print(f"doc-scope-lint FAILED ({len(problems)} problem(s)):")
        for problem in problems:
            print("  " + problem)
        print(
            "\n规则（见 README.md 的「验证」一节）：包内文本不得引用本仓之外的本地文档 —— "
            "把结论直接写在这里，或指向本仓内真实存在的文件与标题。"
        )
        return 1

    print(
        f"doc-scope-lint OK ({len(text_files())} files: 禁用词零命中、"
        "本仓路径引用都真实存在、§ 引用都带本仓锚点)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

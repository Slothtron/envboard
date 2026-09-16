#!/usr/bin/env python3
"""包内文本自包含门禁 —— 禁止引用**未入库**的本地文档。

为什么需要它：对 clone 本仓的人，指向一份不在仓库里的文档的路径不存在、章节号
无处可查，那条线索读到就断。规范与契约必须"只看本仓就能完整理解"。

本仓的入库文档只有 `README.md` / `CHANGELOG.md` / `core/spec/**`；设计、计划、
实机验收等工作材料放在包内 `.agents/`（该目录不入库）。所以：

判据（三条，都可机械验证）：

1. **禁用词零命中**：`AGENTS.md`、工作区规范文件名、以及 `.agents/` ——
   后者是**未入库**的本机工作材料，入库文本指向它对读者不可解析。
2. **指向本仓的文档路径必须存在**：`core/spec/…`、`README.md` 这类引用
   一律要能在包内找到；否则多半是搬过来时漏了（或写完没建）。
3. **`§` 引用必须带本仓锚点**：任何含 `§` 的行都要同时给出本仓内的文件
   （`core/spec/…`、`capabilities.md`、`README.md` …）或写"本文件" ——
   裸 `§7.2` 离开它出处的那份文档就无从解析。

不在禁止之列的（可复核的外部事实坐标，不是"另一份只在本机存在的文档"）：宿主/上游
源码坐标（`mitmproxy/addons/tlsconfig.py:291`）、实测命令、版本号、协议编号、外部 URL。

用法：`python3 scripts/doc_scope_lint.py`（退出码非 0 即失败）。
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

#: 未入库的本地文档的指称 —— 零命中（例外见 LOCAL_ONLY_TOKEN_ALLOWED）。
BANNED_TOKENS = [
    "plugin-repo-architecture",
    "dsh-plugin-dev-SOP",
    "Octop插件开发SOP",
    # 工作区规范文件在包仓库之外，按文件名指称它同样不可解析
    "AGENTS.md",
]

#: 工作材料的目录名。它不入库（全局 ignore 覆盖），所以入库文本指向它 = 死指针。
LOCAL_ONLY_TOKEN = ".agents/"

#: 例外：面向人的发布文档 —— 约定要在仓库里能被发现，否则下一个人会把设计文档
#: 重新写回 `docs/`；以及 `.gitignore`，它必须**写出**这个目录名才能忽略它。
#: 与"分发标识只许出现在面向人的发布文档"是同一手法。
LOCAL_ONLY_TOKEN_ALLOWED = {"README.md", "CHANGELOG.md", ".gitignore"}

#: 只检查这些后缀。部署工件也是"面向读者的文本"：unit 文件里的注释同样不许
#: 指向未入库的文档。
SUFFIXES = {
    ".md", ".rs", ".py", ".js", ".css", ".html", ".toml", ".sh",
    ".yml", ".yaml", ".json",
    # 部署工件也是"面向读者的文本"：unit 文件里的注释同样不许指向仓外
    ".service", ".conf", ".env",
}

#: **无后缀**的文本文件要单独列出来：`.gitignore` 的注释一样是"面向读者的文本"，
#: 而按后缀筛会把它们整类漏掉（实测漏掉过一条指向仓外规范文件的引用）。
EXTRA_FILES = {".gitignore", ".dockerignore"}

SKIP_DIRS = {"target", "node_modules", ".git", "__pycache__", ".zvec-grep", ".agents"}
#: 门禁脚本自己要在词表里写出这些词，所以豁免它。
SELF = {"doc_scope_lint.py"}

#: 含 `§` 的行必须出现下列之一（本仓锚点）。
ANCHOR = re.compile(
    r"core/spec/[\w./-]+|README\.md|CHANGELOG\.md"
    r"|capabilities\.md|rules\.md|errors\.md|fixtures/[\w./-]+|本文件"
)

#: 本仓内的文档/代码路径引用（要在包内真实存在）。
#: 注意 `.json` 必须排在 `.js` 前面，并且尾部不允许再接字母数字 —— 否则
#: `x/normalization.json` 会被切成 `x/normalization.js`（贪心匹配踩过）。
#: 裸文件名那一支用定宽 lookbehind 排除"前半截是路径"的情况（`core/spec/README.md`
#: 不该被当成包根那份 README）。
PATH_REF = re.compile(
    r"(?:core/spec|core/rs|adapters|platforms|scripts|ci)/[\w./-]+\.(?:md|rs|py|json|js|css|html|toml|sh)(?![A-Za-z0-9])"
    r"|(?<![\w./-])(?:README\.md|CHANGELOG\.md|LICENSE)(?![A-Za-z0-9])"
)

#: **历史记录**豁免"路径必须存在"这一条：CHANGELOG 的 Removed 段本来就要提到当时
#: 存在、现在已经删掉的文件。它们不是"指向别处的线索"。
HISTORICAL = {"CHANGELOG.md"}

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
                        f"{relative}:{number}: 引用未入库的本地文档（禁用词 {token!r}）："
                        f"{line.strip()[:100]}"
                    )
            if LOCAL_ONLY_TOKEN in line and str(relative) not in LOCAL_ONLY_TOKEN_ALLOWED:
                problems.append(
                    f"{relative}:{number}: 指向未入库的工作材料目录（{LOCAL_ONLY_TOKEN!r}）——"
                    f"对 clone 本仓的人那是死指针：{line.strip()[:100]}"
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
            "\n规则（见 README.md 的「验证」一节）：入库文本不得引用不在仓库里的本地文档 —— "
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

#!/usr/bin/env python3
"""跨语言对拍：Rust 与 Python 两份 hosts 解析实现必须输出**逐字节**一致。

为什么需要它：`core/spec/capabilities.md`（「多语言实现的仲裁规则」）允许 —— 甚至要求
—— 两份实现并存（Rust 管导入/规范化/落盘，Python 注入器管运行时解析），
重复的代价必须由契约测试兜住。`rules.md` 的 8 个 fixture 只覆盖了常规路径，
这里再补一批**对抗性输入**（BOM、孤儿 CR、制表符、注释截断、非法 label、
超长 label、IPv6 各种写法、冲突、空文本、无尾换行…），因为这些才是两份实现
容易分叉的地方。

**已知分歧用白名单显式声明**，并做双向检查：

* 出现未声明的分歧 → 失败（这是真的 bug 或真的契约变更）；
* 声明过的分歧**消失了** → 也失败（说明白名单过期了，要删掉对应条目）。

后者是为了防止"修好之后白名单偷偷掩盖了新的分歧"。
"""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
RUST_BIN = ROOT / "target" / "debug" / "render_fixture"
#: Python 侧参考实现 = **注入器本体**（M2 起不再是 v1 的临时代码）。
INJECTOR = ROOT / "adapters" / "mitmproxy" / "envboard_mitmproxy.py"
RULES_FIXTURES = ROOT / "core" / "spec" / "fixtures" / "rules"

#: `{case: 原因}` —— 契约按"更严的一侧"取，因此 v1 的 Python 参考实现在这些输入上
#: 与 Rust 不同，且**是 Python 侧不合契约**。注入器（M2）落地后这些条目应逐个消失。
#: `{case: 原因}` —— 已知分歧。**现在是空的**，这正是目标状态：两份实现完全一致。
#:
#: 保留这张表（而不是删掉机制）是因为它承载了一条纪律：出现分歧必须显式声明，
#: 且声明过期（分歧消失）同样失败 —— 否则白名单会慢慢变成"什么都放行"。
#:
#: 历史条目：`v6_zone_id`（Python 的 `ipaddress` 接受带 zone id 的 IPv6、Rust 不接受）。
#: M2 的注入器按契约显式拒绝 `%`，分歧消失，条目随之删除。
KNOWN_DIVERGENCES: dict[str, str] = {}

#: 对抗性输入。刻意包含"契约外的行分隔符"之外的所有边界。
ADVERSARIAL: dict[str, str] = {
    "empty": "",
    "only_nl": "\n",
    "three_nl": "\n\n\n",
    "crlf_only": "\r\n",
    "no_trailing_nl": "10.0.0.1 a.example.com",
    "trailing_nl": "10.0.0.1 a.example.com\n",
    "extra_nl": "10.0.0.1 a.example.com\n\n",
    "bom_once": "\ufeff10.0.0.1 a.example.com",
    "bom_twice": "\ufeff\ufeff10.0.0.1 a.example.com",
    "crlf_pair": "10.0.0.1 a.example.com\r\n10.0.0.2 b.example.com\r\n",
    "lone_cr": "10.0.0.1 a.example.com\r10.0.0.2 b.example.com",
    "tabs_and_indent": "  10.0.0.1\ta.example.com  ",
    "trailing_comment": "10.0.0.1 a.example.com # trailing",
    "multi_hash": "10.0.0.1 a.example.com  # c1 # c2",
    "only_comment": "# only comment",
    "indented_comment": "   # indented",
    "blank_variants": "\t\n  \n",
    "too_few_ip": "10.0.0.1",
    "too_few_host": "a.example.com",
    "two_ips": "1.2.3.4 5.6.7.8",
    "ip_in_host_position": "10.0.0.1 300.1.2.3",
    "multi_host": "10.0.0.1 a.example.com b.example.com c.example.com",
    "host_first_multi": "a.example.com b.example.com 10.0.0.1",
    "wildcard_prefix": "10.0.0.1 *.wild.example.com",
    "double_wildcard": "10.0.0.1 *.*.a.example.com",
    "upper_and_root_dot": "10.0.0.1 API.Example.COM.",
    "leading_dash_label": "10.0.0.1 -bad.example.com",
    "trailing_dash_label": "10.0.0.1 bad-.example.com",
    "double_dot": "10.0.0.1 a..b",
    "underscore_label": "10.0.0.1 a_b.com",
    "dash_label": "10.0.0.1 a-b.com",
    "underscore_only": "10.0.0.1 _",
    "dot_only": "10.0.0.1 .",
    "label_63": "10.0.0.1 " + "a" * 63 + ".example.com",
    "label_64": "10.0.0.1 " + "a" * 64 + ".example.com",
    "non_ascii_label": "10.0.0.1 \u00c4.example.com",
    "uppercase_host_only": "10.0.0.1 COM",
    "single_char_host": "10.0.0.1 a.example.com b",
    "v6_bare": "2001:db8::1 v6.example.com",
    "v6_uncanonical": "2001:0db8::1 v6.example.com",
    "v6_brackets": "[2001:db8::2] v6b.example.com",
    "v6_zone_id": "fe80::1%eth0 v6c.example.com",
    "ipv4_mapped_v6": "::ffff:10.0.0.1 mapped.example.com",
    "zero_address": "0.0.0.0 zero.example.com",
    "broadcast": "255.255.255.255 bcast.example.com",
    "leading_zero_v4": "010.0.0.1 a.example.com",
    "five_octets": "1.2.3.4.5 a.example.com",
    "conflict_two": "10.0.0.1 a.example.com\n10.0.0.2 a.example.com",
    "conflict_three": "10.0.0.1 a.example.com\n10.0.0.2 a.example.com\n10.0.0.3 a.example.com",
    "same_ip_twice": "10.0.0.1 a.example.com\n10.0.0.1 a.example.com",
    "split_host_and_ip": "10.0.0.1\na.example.com",
    "hash_inside_host": "10.0.0.1 a.example.com#b.example.com",
    "mixed_document": (
        "10.0.0.1 a.example.com\nbad line here\n# comment\n\n"
        "10.0.0.1 b.example.com 10.0.0.2 c.example.com\r\n"
    ),
    "many_hosts_one_ip": "10.0.0.9 " + " ".join(f"h{i}.example.com" for i in range(20)),
    "shared_ip_across_lines": "10.0.0.9 a.example.com\n10.0.0.9 b.example.com\n10.0.0.9 c.example.com",
}


def run(command: list[str], fixture: pathlib.Path) -> tuple[int, str]:
    result = subprocess.run(command + [str(fixture)], capture_output=True, text=True)
    return result.returncode, result.stdout


def check_case(name: str, text: str, directory: pathlib.Path) -> tuple[bool, str]:
    fixture = directory / f"{name}.json"
    fixture.write_text(
        json.dumps(
            {"capability": "rules.parse", "description": name, "input": {"text": text}},
            ensure_ascii=False,
        ),
        encoding="utf-8",
    )
    rust_code, rust_out = run([str(RUST_BIN)], fixture)
    py_code, py_out = run([sys.executable, str(INJECTOR), "--render", "--fixture"], fixture)
    if rust_code == py_code and rust_out == py_out:
        return True, ""
    detail = (
        f"rust(rc={rust_code}) {json.dumps(rust_out[:200], ensure_ascii=False)}\n"
        f"    py  (rc={py_code}) {json.dumps(py_out[:200], ensure_ascii=False)}"
    )
    return False, detail


def main() -> int:
    if not RUST_BIN.exists():
        print(f"verify-dual FAILED: {RUST_BIN} not built (run `cargo build` first)")
        return 1

    problems: list[str] = []
    diverged: set[str] = set()
    checked = 0

    with tempfile.TemporaryDirectory() as tmp:
        directory = pathlib.Path(tmp)
        # 1) 契约 fixture 必须逐字节一致
        for fixture in sorted(RULES_FIXTURES.glob("*.json")):
            case = json.loads(fixture.read_text(encoding="utf-8"))
            text = (case.get("input") or {}).get("text") or ""
            checked += 1
            ok, detail = check_case(f"fixture_{fixture.stem}", text, directory)
            if not ok:
                problems.append(f"fixture {fixture.name} diverged:\n    {detail}")

        # 2) 对抗性输入
        for name, text in ADVERSARIAL.items():
            checked += 1
            ok, detail = check_case(name, text, directory)
            if not ok:
                if name in KNOWN_DIVERGENCES:
                    diverged.add(name)
                    continue
                problems.append(f"case {name} diverged (undeclared):\n    {detail}")

    stale = sorted(set(KNOWN_DIVERGENCES) - diverged)
    for name in stale:
        problems.append(
            f"declared divergence {name!r} no longer diverges — remove it from "
            "KNOWN_DIVERGENCES (a stale allowlist can hide new divergences)"
        )

    if problems:
        print("verify-dual FAILED:")
        for problem in problems:
            print(f"  - {problem}")
        return 1

    print(f"verify-dual OK ({checked} cases byte-identical, {len(diverged)} declared divergence(s))")
    for name in sorted(diverged):
        print(f"  known divergence {name}: {KNOWN_DIVERGENCES[name]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

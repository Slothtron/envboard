#!/usr/bin/env python3
"""契约测试 —— 跑 ``core/spec/fixtures`` 下的 golden case。

契约见 ``core/spec/capabilities.md``（能力与语义）、``core/spec/rules.md``（规则语法 BNF）、
``core/spec/errors.md``（错误码）。

fixture 形如::

    {
      "capability": "environment.validate",
      "description": "listen.host 省略时取默认 127.0.0.1",
      "input": {"name": "prod", "listen": {"port": 16600}},
      "expect": {"ok": true, "name": "prod", "listen": {"host": "127.0.0.1", "port": 16600}}
    }

或失败用例::

    {"expect": {"ok": false, "code": "invalid_config", "field": "environment.listen.port"}}

**v2 的过渡状态（别把"全绿"读成"都实现了"）**：v2 把核心实现搬到 Rust
（``core/rs/crates``）。本脚本是 **Python 侧**的参考实现宿主，因此：

* 有 Python 参考实现的能力（当前只有 ``rules.parse``，注入器也需要一份解析器）：
  真正跑一遍、逐字段比对，失败即 verify 失败 —— 这是真的回归信号。
* 只有 Rust 实现的能力（``environment.validate`` / ``environment.merge`` /
  ``port.allocate`` / ``instance.reconcile``）：本脚本**只校验 fixture 的形状与归属**
  （能力 id 已登记、目录对应、`expect` 有 ok/code），计入 ``consumed_by_rust`` 并打印。
  这些 fixture 的真断言在 `cargo test -p envboard-contract-tests` 里，
  那条步骤同样在 `ci/verify.sh` 的默认层里跑。

两条防呆，避免"绿得没有内容"：

1. 若 ``rules.parse`` 的 Python 参考实现缺位（例如 M1 里删掉了
   ``src/envboard/core/rules.py`` 却没把 runner 指向注入器），本脚本**失败**而不是降级为
   pending —— 否则唯一的真断言会静默消失。
2. 若出现未登记的能力目录，或登记了却没有 fixture 目录，本脚本**失败**。
"""

from __future__ import annotations

import importlib.util
import json
import pathlib
import sys
from collections import Counter
from typing import Any, Callable

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "src"))

FIXTURES = ROOT / "core" / "spec" / "fixtures"


# --------------------------------------------------------------------------- #
# 能力注册表：id → (fixture 目录, Python 参考实现或 None)
# runner 为 None = "本脚本不实现，由 Rust 侧消费"（见模块 docstring）。
# --------------------------------------------------------------------------- #


INJECTOR = ROOT / "adapters" / "mitmproxy" / "envboard_mitmproxy.py"


def _load_injector():
    """按路径加载注入器 —— M2 起它就是 ``rules.parse`` 的 Python 参考实现。

    刻意**不**把它做成 Python 包再 import：注入器是"单文件、纯标准库、被二进制内嵌"
    的形态（单文件、纯标准库、被二进制内嵌），按路径加载正是它被 mitmdump `-s` 加载的方式。
    v1 的 Python 包已在 M4 移除，所以这里没有"回退到旧实现"的余地 —— 找不到注入器
    就是硬失败。
    """
    spec = importlib.util.spec_from_file_location("envboard_mitmproxy", INJECTOR)
    assert spec and spec.loader, f"cannot load injector at {INJECTOR}"
    module = importlib.util.module_from_spec(spec)
    sys.modules["envboard_mitmproxy"] = module
    spec.loader.exec_module(module)
    return module


def _runner_rules(text: str, _case: dict | None = None):
    return _load_injector().parse_hosts_text(text)


def _runner_insecure(case: dict | None = None):
    """`insecure.hosts` 的 Python 参考实现也在注入器里（运行时用的就是它）。"""
    assert case is not None
    return _load_injector().insecure_match(case["input"])


CAPABILITIES: dict[str, tuple[str, Callable[..., Any] | None]] = {
    "environment.validate": ("environment", None),
    "environment.merge": ("merge", None),
    "rules.parse": ("rules", _runner_rules),
    "insecure.hosts": ("insecure", _runner_insecure),
    "port.allocate": ("ports", None),
    "instance.reconcile": ("lifecycle", None),
}


def view_of(capability: str, result: Any) -> dict[str, Any]:
    """把结果折成可比较的扁平视图。

    规则解析的 ``skipped`` 里有行号等噪声，直接全量比对会让 fixture 又长又脆；
    这里换成稳定的形状（原因码、detail、``[host, dropped, kept]``）。
    """
    if capability == "rules.parse":
        return _rules_view(result)
    if capability == "insecure.hosts":
        # 注入器的参考实现直接返回一个朴素 dict（它就是判定本身，没有别的形状）。
        return result
    return result.to_json()


def _rules_view(result: Any) -> dict[str, Any]:
    return {
        "entries": dict(result.entries),
        "accepted": result.accepted,
        "ips": result.ips,
        "conflicts": [[c.host, c.dropped, c.kept] for c in result.conflicts],
        "skipped_reasons": [s.reason for s in result.skipped],
        "skipped_details": [s.detail for s in result.skipped],
        "stats": result.to_json()["stats"],
    }


def _check_rules_determinism(case: dict, result: Any) -> list[str]:
    """契约里的"输出确定性"条款：渲染必须逐字节可复现，且 parse(render(x)) 稳定。"""
    module = _load_injector()
    parse_hosts_text, render_rules = module.parse_hosts_text, module.render_rules

    problems: list[str] = []
    source = case.get("source") or ""
    first = render_rules(result.entries, source=source)
    reparsed = parse_hosts_text(first)
    second = render_rules(reparsed.entries, source=source)
    if first != second:
        problems.append("rendering is not byte-stable (parse∘render∘parse changed the output)")
    if dict(reparsed.entries) != dict(result.entries):
        problems.append("parse(render(entries)) did not round-trip to the same entries")
    return problems


def _check_shape(path: pathlib.Path, case: dict) -> list[str]:
    """不管能力是否已实现，fixture 自身的形状必须先合法。"""
    problems: list[str] = []
    capability = case.get("capability")
    if capability not in CAPABILITIES:
        problems.append(f"unknown capability {capability!r} (add it to core/spec/capabilities.md)")
        return problems
    expected_dir = CAPABILITIES[capability][0]
    if path.parent.name != expected_dir:
        problems.append(
            f"capability {capability!r} must live in fixtures/{expected_dir}/, "
            f"found it in fixtures/{path.parent.name}/"
        )
    if "input" not in case:
        problems.append("missing 'input'")
    expect = case.get("expect")
    if not isinstance(expect, dict) or "ok" not in expect:
        problems.append("missing 'expect.ok'")
    elif expect["ok"] is False and not expect.get("code"):
        problems.append("failure case must pin 'expect.code'")
    elif expect["ok"] is True and len(expect) < 2:
        problems.append("success case must pin at least one normalized field")
    if not case.get("description"):
        problems.append("missing 'description' (fixtures are read by humans)")
    return problems


def check_case(case: dict) -> list[str]:
    problems: list[str] = []
    capability = case["capability"]
    expect = case["expect"]
    runner = CAPABILITIES[capability][1]
    assert runner is not None
    try:
        result = (
            runner(case["input"].get("text") or "", case)
            if capability == "rules.parse"
            else runner(case)
        )
    except Exception as exc:  # noqa: BLE001
        from envboard.core.errors import EnvBoardError

        if isinstance(exc, EnvBoardError):
            if expect.get("ok"):
                problems.append(f"expected success, got {exc.code}: {exc.message}")
            else:
                if expect.get("code") and exc.code != expect["code"]:
                    problems.append(f"expected code {expect['code']!r}, got {exc.code!r}")
                if expect.get("field") and exc.field != expect["field"]:
                    problems.append(f"expected field {expect['field']!r}, got {exc.field!r}")
            return problems
        problems.append(f"unexpected {type(exc).__name__}: {exc}")
        return problems

    if not expect.get("ok"):
        problems.append(f"expected failure ({expect}), but the call succeeded")
        return problems

    actual = view_of(capability, result)
    for key, value in expect.items():
        if key == "ok":
            continue
        if actual.get(key) != value:
            problems.append(f"field {key!r} expected {value!r}, got {actual.get(key)!r}")
    if capability == "rules.parse":
        problems.extend(_check_rules_determinism(case, result))
    return problems


def _python_reference_ok() -> str | None:
    """``rules.parse`` 的 Python 参考实现必须在位（见模块 docstring 的防呆 1）。"""
    try:
        _runner_rules("")
    except ImportError as exc:
        return f"rules.parse 的 Python 参考实现（注入器）不可用：{exc}"
    return None


def main() -> int:
    if not FIXTURES.is_dir():
        print(f"verify-contract FAILED: no fixtures at {FIXTURES}")
        return 1

    problems: list[str] = []
    reference_problem = _python_reference_ok()
    if reference_problem:
        problems.append(reference_problem)
    runs_rules = reference_problem is None

    passed = 0
    pending: Counter[str] = Counter()

    for path in sorted(FIXTURES.rglob("*.json")):
        try:
            case = json.loads(path.read_text(encoding="utf-8"))
        except ValueError as exc:
            problems.append(f"{path.name}: invalid JSON: {exc}")
            continue
        shape_problems = _check_shape(path, case)
        if shape_problems:
            problems.extend(f"{path.relative_to(FIXTURES)}: {p}" for p in shape_problems)
            continue
        capability = case["capability"]
        implemented = CAPABILITIES[capability][1] is not None and runs_rules
        if not implemented:
            pending[capability] += 1
        case_problems = check_case(case) if implemented else []
        if case_problems:
            problems.extend(f"{path.relative_to(FIXTURES)}: {p}" for p in case_problems)
        elif implemented:
            passed += 1

    declared = {capability: spec[0] for capability, spec in CAPABILITIES.items()}
    present_dirs = {p.name for p in FIXTURES.iterdir() if p.is_dir()}
    for capability, directory in declared.items():
        if directory not in present_dirs:
            problems.append(
                f"capability {capability!r} declares fixtures/{directory}/ but it is missing"
            )
    for directory in sorted(present_dirs - set(declared.values())):
        problems.append(
            f"fixtures/{directory}/ has no capability registered in scripts/verify_contract.py"
        )

    if problems:
        print("verify-contract FAILED:")
        for problem in problems:
            print(f"  - {problem}")
        return 1

    total_pending = sum(pending.values())
    detail = ", ".join(f"{name}={count}" for name, count in sorted(pending.items())) or "none"
    print(f"verify-contract OK ({passed} passed here, {total_pending} checked shape-only)")
    if total_pending:
        print(f"  这些能力的实断言在 Rust 侧（cargo test -p envboard-contract-tests）：{detail}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

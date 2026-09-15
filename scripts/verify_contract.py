#!/usr/bin/env python3
"""契约测试 —— 跑 ``core/spec/fixtures`` 下的 golden case。

fixture 形如::

    {
      "capability": "environment.validate",
      "description": "bare IPv6 is accepted",
      "input": {"name": "prod", "dns_servers": ["::1"]},
      "expect": {"ok": true, "name": "prod"}
    }

或失败用例::

    {"expect": {"ok": false, "code": "invalid_config", "field": "dns_servers[0]"}}
"""

from __future__ import annotations

import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "src"))

from envboard.core.errors import EnvBoardError  # noqa: E402
from envboard.core.model import Environment  # noqa: E402
from envboard.core.rules import parse_hosts_text  # noqa: E402

FIXTURES = ROOT / "core" / "spec" / "fixtures"


def run_case(capability: str, payload: dict):
    if capability == "environment.validate":
        return Environment.from_json(payload, path="environment")
    if capability == "environment.merge":
        base = Environment.from_json(payload["base"], path="base")
        return base.merged(payload["patch"])
    if capability == "rules.parse":
        return parse_hosts_text(payload.get("text") or "")
    raise AssertionError(f"unknown capability {capability!r}")


def view_of(capability: str, result) -> dict:
    """把结果折成可比较的扁平视图。

    规则解析的 ``skipped`` / ``conflicts`` 里有行号等噪声，直接全量比对会让 fixture
    又长又脆；这里换成稳定的形状（原因码、detail、``[host, dropped, kept]``）。
    """
    if capability != "rules.parse":
        return result.to_json()
    return {
        "entries": dict(result.entries),
        "accepted": result.accepted,
        "ips": result.ips,
        "conflicts": [[c.host, c.dropped, c.kept] for c in result.conflicts],
        "skipped_reasons": [s.reason for s in result.skipped],
        "skipped_details": [s.detail for s in result.skipped],
        "stats": result.to_json()["stats"],
    }


def check_case(path: pathlib.Path, case: dict) -> list[str]:
    problems: list[str] = []
    capability = case.get("capability") or path.parent.name
    expect = case.get("expect") or {}
    try:
        result = run_case(capability, case.get("input") or {})
    except EnvBoardError as exc:
        if expect.get("ok"):
            problems.append(f"{path.name}: expected success, got {exc.code}: {exc.message}")
        else:
            if expect.get("code") and exc.code != expect["code"]:
                problems.append(
                    f"{path.name}: expected code {expect['code']!r}, got {exc.code!r}"
                )
            if expect.get("field") and exc.field != expect["field"]:
                problems.append(
                    f"{path.name}: expected field {expect['field']!r}, got {exc.field!r}"
                )
        return problems
    except Exception as exc:  # noqa: BLE001
        problems.append(f"{path.name}: unexpected {type(exc).__name__}: {exc}")
        return problems

    if not expect.get("ok"):
        problems.append(f"{path.name}: expected failure ({expect}), but the call succeeded")
        return problems
    actual = view_of(capability, result)
    for key, value in expect.items():
        if key == "ok":
            continue
        if actual.get(key) != value:
            problems.append(f"{path.name}: field {key!r} expected {value!r}, got {actual.get(key)!r}")
    return problems


def main() -> int:
    if not FIXTURES.is_dir():
        print(f"verify-contract FAILED: no fixtures at {FIXTURES}")
        return 1

    problems: list[str] = []
    total = 0
    for path in sorted(FIXTURES.rglob("*.json")):
        try:
            case = json.loads(path.read_text(encoding="utf-8"))
        except ValueError as exc:
            problems.append(f"{path.name}: invalid JSON: {exc}")
            continue
        total += 1
        problems.extend(check_case(path, case))

    if problems:
        print("verify-contract FAILED:")
        for problem in problems:
            print(f"  - {problem}")
        return 1
    print(f"verify-contract OK ({total} cases)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

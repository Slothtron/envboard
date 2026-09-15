#!/usr/bin/env bash
# envboard 的单一验证入口（AGENTS.md §7）。
#
# 托管方无关：任何 CI 直接调用 `bash ci/verify.sh` 即可；本仓库不绑定 GitHub Actions。
# 也可通过仓库根的 `npm run verify` 调用。
#
# 用法：
#   bash ci/verify.sh            # 跑全部
#   bash ci/verify.sh unit       # 只跑单测
#   PYTHON=... bash ci/verify.sh # 指定解释器（必须能看到 mitmproxy 才能跑 smoke）
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ -z "${PYTHON:-}" ]]; then
  if command -v python3 >/dev/null 2>&1; then
    PYTHON=python3
  else
    PYTHON=python
  fi
fi

export PYTHONPATH="$ROOT/src${PYTHONPATH:+:$PYTHONPATH}"
export PYTHONDONTWRITEBYTECODE=1

STEP="${1:-all}"
FAILED=0

run() {
  local name="$1"; shift
  echo ""
  echo "=== $name ==="
  if "$@"; then
    return 0
  fi
  echo "!!! $name FAILED"
  FAILED=1
  return 0
}

step_clean() {
  # 实机跑过 mitmweb 之后必然留下 __pycache__；发布物检查前先清掉。
  find "$ROOT" -path "$ROOT/.git" -prune -o \
    \( -name '__pycache__' -o -name '*.pyc' -o -name '*.pyo' \) -print0 2>/dev/null |
    xargs -0 -r rm -rf
  echo "clean: removed bytecode caches"
}

step_compile()   { "$PYTHON" scripts/compile_check.py; }
step_lint()      { "$PYTHON" scripts/dependency_lint.py; }
step_naming()    { "$PYTHON" scripts/naming_lint.py; }
step_unit()      { "$PYTHON" -m unittest discover -s tests -t "$ROOT" -v; }
step_contract()  { "$PYTHON" scripts/verify_contract.py; }
step_pack()      { step_clean; "$PYTHON" scripts/pack_check.py; }
step_live()      { MITMWEB="${MITMWEB:-}" bash "$ROOT/scripts/verify_live.sh"; }

step_typecheck() {
  if "$PYTHON" -c "import mypy" >/dev/null 2>&1; then
    "$PYTHON" -m mypy --ignore-missing-imports \
      src/envboard/core src/envboard/infra src/envboard/adapter
  else
    echo "typecheck SKIPPED: mypy is not installed in this interpreter."
    echo "                   (compile-check + dependency-lint still gate the tree)"
  fi
}

step_smoke() {
  if ! "$PYTHON" -c "import mitmproxy" >/dev/null 2>&1; then
    echo "smoke SKIPPED: mitmproxy is not importable by $PYTHON."
    return 0
  fi
  "$PYTHON" - <<'PY'
import importlib.util, pathlib, sys

root = pathlib.Path.cwd()
spec = importlib.util.spec_from_file_location("envboard_entry", root / "addons" / "envboard.py")
assert spec and spec.loader, "cannot build spec for addons/envboard.py"
module = importlib.util.module_from_spec(spec)
sys.modules["envboard_entry"] = module
spec.loader.exec_module(module)

addons = getattr(module, "addons", None)
assert isinstance(addons, list) and len(addons) == 1, "addons/envboard.py must export [EnvBoardAddon()]"
addon = addons[0]
assert addon.name == "envboard", f"unexpected addon name: {addon.name!r}"
hooks = [h for h in ("load", "configure", "running", "done", "dns_response", "request", "response")
         if callable(getattr(addon, h, None))]
missing = {"load", "configure", "running", "done", "dns_response"} - set(hooks)
assert not missing, f"addon is missing hooks: {sorted(missing)}"
commands = sorted(
    getattr(obj, "command_name") for obj in vars(type(addon)).values()
    if isinstance(getattr(obj, "command_name", None), str)
)
assert len(commands) >= 8, f"expected >=8 commands, got {commands}"
print(f"smoke OK: addon={addon.name} hooks={hooks}")
print(f"          commands={commands}")
PY
}

case "$STEP" in
  all)
    run "compile-check"  step_compile
    run "dependency-lint" step_lint
    run "naming-lint"    step_naming
    run "unit-tests"     step_unit
    run "contract-tests" step_contract
    run "typecheck"      step_typecheck
    run "pack-check"     step_pack
    run "smoke"          step_smoke
    ;;
  compile)    step_compile ;;
  lint)       step_lint ;;
  naming)     step_naming ;;
  unit)       step_unit ;;
  contract)   step_contract ;;
  typecheck)  step_typecheck ;;
  pack)       step_pack ;;
  smoke)      step_smoke ;;
  clean)      step_clean ;;
  live)       step_live ;;   # 需要 mitmweb + 网络，默认不纳入 all
  *)
    echo "unknown step: $STEP" >&2
    exit 2
    ;;
esac

echo ""
if [[ "$FAILED" -ne 0 ]]; then
  echo "verify: FAILED"
  exit 1
fi
echo "verify: OK"

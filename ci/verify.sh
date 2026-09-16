#!/usr/bin/env bash
# envboard 的单一验证入口：CI 只调它一个。
#
# 托管方无关：任何 CI 直接调用 `bash ci/verify.sh` 即可；本仓库不绑定 GitHub Actions。
# 也可通过仓库根的 `npm run verify` 调用。
#
# 用法：
#   bash ci/verify.sh            # 跑全部（默认层：不碰网络、不需要 mitmproxy）
#   bash ci/verify.sh rust       # 只跑 Rust 相关门禁
#   bash ci/verify.sh live       # 实机层：真 mitmdump + 真浏览器前置条件（需要宿主）
#   PYTHON=... bash ci/verify.sh # 指定解释器（只用于跑 Python 侧门禁脚本）
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
step_naming()    { "$PYTHON" scripts/naming_lint.py; }
# 包内文本自包含：不得引用本仓之外的本地文档（见 README.md 的「验证」一节）
step_doc_scope() { "$PYTHON" scripts/doc_scope_lint.py; }
step_contract()  { "$PYTHON" scripts/verify_contract.py; }
step_pack()      { step_clean; "$PYTHON" scripts/artifact_check.py; }

# 冒烟（默认层，不需要 mitmproxy）：二进制能跑、注入器物化与 --render 都通。
# 取代了 v1 那个"加载 Python addon 看 hooks"的 smoke —— 被测对象换了。
step_smoke() {
  # 注意：不要 `local work` + `trap ... RETURN` —— `set -u` 下 trap 在函数返回后执行，
  # 那时变量已出作用域，会以 "unbound variable" 让整步失败（这个坑踩过一次）。
  SMOKE_DIR="$(mktemp -d)"
  local work="$SMOKE_DIR"
  "$ROOT/target/debug/envboard" --help >/dev/null
  "$ROOT/target/debug/envboard" --state-dir "$work" --core fake status >/dev/null
  "$ROOT/target/debug/envboard" --state-dir "$work" --core fake env add smoke --port 16666 >/dev/null
  "$ROOT/target/debug/envboard" --state-dir "$work" --core fake env list | grep -q smoke
  # 注入器必须能被物化，且 --render 在**没有 mitmproxy**的解释器下也能跑
  "$ROOT/target/debug/envboard" --state-dir "$work" --core fake env add probe --port 16667 >/dev/null
  local rendered
  rendered="$("$PYTHON" "$ROOT/adapters/mitmproxy/envboard_mitmproxy.py" --render \
    --fixture "$ROOT/core/spec/fixtures/rules/normalization.json")"
  case "$rendered" in
    *"# envboard rules file"*) ;;
    *) echo "injector --render produced unexpected output"; return 1 ;;
  esac
  rm -rf "$SMOKE_DIR"
  echo "smoke OK (binary + fake core + injector --render)"
}

# 实机层：需要真 mitmdump（与真浏览器前置条件）。默认**不纳入 all**。
step_live() {
  "$PYTHON" scripts/spike_m0_5.py
  cargo test --locked --offline -p envboard-core-mitmproxy --test live_manager -- --nocapture
  cargo test --locked --offline -p envboard-cli --test thin_client -- --nocapture
  "$PYTHON" scripts/verify_live_v2.py
}
step_live_v2()   { "$PYTHON" scripts/verify_live_v2.py; }

# --------------------------------------------------------------------------- #
# Rust 侧（v2 核心）
#
# 全部带 `--locked --offline`：
#   --locked   Cargo.lock 必须与 manifest 一致（可复现构建的前提）
#   --offline  "离线可构建"是验收项；依赖因此必须克制
# --------------------------------------------------------------------------- #

step_rust_dep()   { "$PYTHON" scripts/rust_dependency_lint.py; }
step_rust_check() { cargo check --workspace --all-targets --locked --offline; }
step_rust_build() { cargo build --workspace --locked --offline; }
step_rust_test()  { cargo test --workspace --locked --offline; }

step_rust_fmt() {
  if cargo fmt --version >/dev/null 2>&1; then
    cargo fmt --all --check
  else
    echo "rust-fmt SKIPPED: cargo-fmt (rustfmt component) is not available."
  fi
}

step_rust_clippy() {
  if cargo clippy --version >/dev/null 2>&1; then
    cargo clippy --workspace --all-targets --offline -- -D warnings
  else
    echo "rust-clippy SKIPPED: clippy component is not available."
  fi
}

# 跨语言对拍：Rust 与 Python 两份 hosts 解析实现必须逐字节一致。
# 依赖 rust-build 的产物（target/debug/render_fixture），所以在它之后跑。
step_dual() { "$PYTHON" scripts/verify_dual_impl.py; }

# 实机层：真 mitmdump + 真改写 + 真管理器（端到端集成）。
# 默认**不进 all** —— 它要求本机装了 mitmdump，属于 live 那一层。
step_rust_live() { cargo test --locked --offline -p envboard-core-mitmproxy --test live_manager -- --nocapture; }
step_spike() { "$PYTHON" scripts/spike_m0_5.py; }

step_typecheck() {
  if "$PYTHON" -c "import mypy" >/dev/null 2>&1; then
    "$PYTHON" -m mypy --ignore-missing-imports \
      adapters/mitmproxy scripts
  else
    echo "typecheck SKIPPED: mypy is not installed in this interpreter."
    echo "                   (compile-check + dependency-lint still gate the tree)"
  fi
}


case "$STEP" in
  all)
    run "compile-check"  step_compile
    run "naming-lint"    step_naming
    run "doc-scope-lint" step_doc_scope
    run "rust-dep-lint"  step_rust_dep
    run "rust-fmt"       step_rust_fmt
    run "rust-clippy"    step_rust_clippy
    run "rust-check"     step_rust_check
    run "rust-build"     step_rust_build
    run "rust-test"      step_rust_test
    run "contract-dual"  step_dual
    run "contract-tests" step_contract
    run "typecheck"      step_typecheck
    run "pack-check"     step_pack
    run "smoke"          step_smoke
    ;;
  compile)     step_compile ;;
  naming)      step_naming ;;
  doc-scope)   step_doc_scope ;;
  contract)    step_contract ;;
  typecheck)   step_typecheck ;;
  pack)        step_pack ;;
  smoke)       step_smoke ;;
  rust)        step_rust_dep; step_rust_check; step_rust_build; step_rust_test; step_dual ;;
  rust-live)   step_rust_live ;;
  live-v2)     step_live_v2 ;;
  spike)       step_spike ;;
  rust-dep)    step_rust_dep ;;
  rust-check)  step_rust_check ;;
  rust-build)  step_rust_build ;;
  rust-test)   step_rust_test ;;
  rust-fmt)    step_rust_fmt ;;
  rust-clippy) step_rust_clippy ;;
  dual)        step_dual ;;
  clean)       step_clean ;;
  live)        step_live ;;   # 需要 mitmweb + 网络，默认不纳入 all
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

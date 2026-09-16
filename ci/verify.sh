#!/usr/bin/env bash
# envboard 的单一验证入口：CI 只调它一个。
#
# 托管方无关：任何 CI 直接调用 `bash ci/verify.sh` 即可；本仓库不绑定 GitHub Actions。
#
# 本脚本**只做编排**：分层、顺序、依赖探测与响亮失败。判据一律在 Rust 测试里
# （`core/rs/crates/envboard-policy-tests` 一门禁一文件，可单跑），脚本里不写断言。
#
# 层：
#   policy   仓库纪律（工具链收敛 / 命名 / 文本自包含 / 依赖方向）—— 纯 Rust，无外部语言
#   rust     fmt / clippy / check / build / test —— 纯 Rust
#   contract 契约 fixture 的形状与消费
#   artifact 发布物与产物纪律
#   adapter  宿主适配器层（对拍、冒烟）—— 这一层要适配器宿主解释器
#   live     真 mitmdump + 真网络，默认**不在** all 里
#
# 用法：
#   bash ci/verify.sh            # 默认层：policy + rust + contract + artifact + adapter
#   bash ci/verify.sh rust       # 纯 Rust 子集（policy + rust；迁移期仍需要解释器）
#   bash ci/verify.sh live       # 实机层（需要宿主）
#   PYTHON=... bash ci/verify.sh # 指定适配器宿主的解释器
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# 一律带 `--locked --offline`：
#   --locked   Cargo.lock 必须与 manifest 一致（可复现构建的前提）
#   --offline  "离线可构建"是验收项；依赖因此必须克制
LOCKED=(--locked --offline)

# 适配器宿主的解释器。**只用于跑适配器脚本**，探测不到时由 require_interpreter 响亮失败。
if [[ -z "${PYTHON:-}" ]]; then
  if command -v python3 >/dev/null 2>&1; then
    PYTHON=python3
  elif command -v python >/dev/null 2>&1; then
    PYTHON=python
  fi
fi
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

# 缺解释器**响亮失败**，并给出两条出路；不静默跳过（跳过等于护栏消失）。
require_interpreter() {
  if [[ -n "${PYTHON:-}" ]]; then
    return 0
  fi
  echo ""
  echo "!!! 这一层需要一个 Python 解释器来跑宿主适配器脚本，但 PATH 上没有 python3 / python。"
  echo "    两条出路："
  echo "      1) 装适配器宿主（mitmproxy 自带一个解释器），或用 PYTHON=<路径> 指定；"
  echo "      2) 跑不吃解释器的子集：bash ci/verify.sh rust"
  FAILED=1
  return 1
}

# --------------------------------------------------------------------------- #
# policy 层：仓库纪律
# --------------------------------------------------------------------------- #

step_policy() {
  run "policy/gates" cargo test -p envboard-policy-tests "${LOCKED[@]}"
  # ↓↓↓ 迁移期：编译检查还是 Python 脚本；`dual` 落地后它也被 Rust 侧取代
  #     （见 envboard-policy-tests 的 PENDING 白名单）。
  require_interpreter || return 0
  run "policy/compile" step_compile
}

step_compile()   { "$PYTHON" scripts/compile_check.py; }

# --------------------------------------------------------------------------- #
# rust 层：格式 / lint / 编译 / 单测
#
# fmt 与 clippy 随 toolchain（rust-toolchain.toml 钉死的 1.98.0）一起来，
# 所以它们是**必需**步骤 —— 缺了就红，不再有 "SKIPPED" 这条路。
# --------------------------------------------------------------------------- #

step_rust_fmt()    { cargo fmt --all --check; }
step_rust_clippy() { cargo clippy --workspace --all-targets --offline -- -D warnings; }
step_rust_check()  { cargo check --workspace --all-targets "${LOCKED[@]}"; }
step_rust_build()  { cargo build --workspace "${LOCKED[@]}"; }
step_rust_test()   { cargo test --workspace "${LOCKED[@]}"; }

step_rust() {
  run "rust-fmt"    step_rust_fmt
  run "rust-clippy" step_rust_clippy
  run "rust-check"  step_rust_check
  run "rust-build"  step_rust_build
  run "rust-test"   step_rust_test
}

# --------------------------------------------------------------------------- #
# contract 层：fixture 形状 + 两侧消费
# --------------------------------------------------------------------------- #

step_contract_script() { "$PYTHON" scripts/verify_contract.py; }
step_contract_rust()   { cargo test -p envboard-contract-tests "${LOCKED[@]}"; }

step_contract() {
  require_interpreter || return 0
  run "contract-shape" step_contract_script
  run "contract-tests" step_contract_rust
}

# --------------------------------------------------------------------------- #
# artifact 层：发布物内容
# --------------------------------------------------------------------------- #

step_clean() {
  # 实机跑过 mitmweb 之后必然留下 __pycache__；发布物检查前先清掉。
  find "$ROOT" -path "$ROOT/.git" -prune -o \
    \( -name '__pycache__' -o -name '*.pyc' -o -name '*.pyo' \) -print0 2>/dev/null |
    xargs -0 -r rm -rf
  echo "clean: removed bytecode caches"
}

step_pack() { step_clean; "$PYTHON" scripts/artifact_check.py; }

step_artifact() { run "artifact" step_pack; }

# --------------------------------------------------------------------------- #
# adapter 层：宿主适配器（对拍 + 冒烟）
#
# 对拍是两份 hosts 解析实现（Rust 与注入器）一致性的唯一护栏，所以它留在默认层；
# 它需要解释器，而且解释器缺失时要**响亮失败**，不静默跳过。
# --------------------------------------------------------------------------- #

step_dual() { "$PYTHON" scripts/verify_dual_impl.py; }

# 冒烟（不需要 mitmproxy）：二进制能跑、注入器物化与 --render 都通。
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

step_adapter() {
  require_interpreter || return 0
  run "adapter/dual"  step_dual
  run "adapter/smoke" step_smoke
}

# --------------------------------------------------------------------------- #
# live 层：真宿主（真 mitmdump + 真网络）。默认**不进 all**。
# --------------------------------------------------------------------------- #

step_live_manager() { cargo test "${LOCKED[@]}" -p envboard-core-mitmproxy --test live_manager -- --nocapture; }
step_live_thin()    { cargo test "${LOCKED[@]}" -p envboard-cli --test thin_client -- --nocapture; }
step_spike()        { "$PYTHON" scripts/spike_m0_5.py; }
step_live_v2()      { "$PYTHON" scripts/verify_live_v2.py; }

step_live() {
  require_interpreter || return 0
  run "live/spike"       step_spike
  run "live/manager"     step_live_manager
  run "live/thin-client" step_live_thin
  run "live/workbench"   step_live_v2
}

case "$STEP" in
  all)
    step_policy
    step_rust
    step_contract
    step_artifact
    step_adapter
    ;;
  policy)        step_policy ;;
  rust)          step_policy; step_rust ;;
  contract)      step_contract ;;
  artifact|pack) step_artifact ;;
  adapter)       step_adapter ;;
  smoke)         step_smoke ;;
  dual)          step_dual ;;
  compile)       step_compile ;;
  rust-fmt)      step_rust_fmt ;;
  rust-clippy)   step_rust_clippy ;;
  rust-check)    step_rust_check ;;
  rust-build)    step_rust_build ;;
  rust-test)     step_rust_test ;;
  rust-live)     step_live_manager ;;
  live-v2)       step_live_v2 ;;
  spike)         step_spike ;;
  clean)         step_clean ;;
  live)          step_live ;;
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

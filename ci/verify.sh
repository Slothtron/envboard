#!/usr/bin/env bash
# envboard 的单一验证入口：CI 只调它一个。
#
# 托管方无关：任何 CI 直接调用 `bash ci/verify.sh` 即可；本仓库不绑定 GitHub Actions。
#
# 本脚本**只做编排**：分层、顺序、依赖探测与响亮失败。判据一律在 Rust 测试里
# （`tests/policy-tests` 一门禁一文件，可单跑），脚本里不写断言。
#
# 层：
#   policy   仓库纪律（工具链收敛 / 命名 / 文本自包含 / 依赖方向）—— 纯 Rust，无外部语言
#   rust     fmt / clippy / check / build / test —— 纯 Rust
#   contract 契约 fixture 的形状与消费
#   artifact 发布物与产物纪律
#   frontend Vite 工程（typecheck + build + dist drift）—— 显式层，**不在** all 里，
#            构建调用圈禁在 frontend/verify-frontend.sh（toolchain 边界）
#   live     实机层（真网络、真进程起停；仅需 openssl/curl），默认**不在** all 里
#
# 用法：
#   bash ci/verify.sh            # 默认层：policy + rust + contract + artifact
#   bash ci/verify.sh rust       # 纯 Rust 子集（policy + rust）
#   bash ci/verify.sh frontend   # 前端工程层（需要前端工具链，见 frontend/verify-frontend.sh）
#   bash ci/verify.sh live       # 实机层（需要宿主）
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# 一律带 `--locked --offline`：
#   --locked   Cargo.lock 必须与 manifest 一致（可复现构建的前提）
#   --offline  "离线可构建"是验收项；依赖因此必须克制
LOCKED=(--locked --offline)

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
# --------------------------------------------------------------------------- #
# policy 层：仓库纪律
# --------------------------------------------------------------------------- #

step_policy() {
  run "policy/gates" cargo test -p envboard-policy-tests "${LOCKED[@]}"
}

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

step_contract() {
  run "contract-tests" cargo test -p envboard-contract-tests "${LOCKED[@]}"
}

# --------------------------------------------------------------------------- #
# artifact 层：发布物内容
# --------------------------------------------------------------------------- #

step_artifact() {
  run "artifact" cargo test -p envboard-server --test artifact "${LOCKED[@]}"
}

# --------------------------------------------------------------------------- #
# frontend 层：Vite 工程（显式层；调用体在 frontend/ 边界内，默认路径 cargo-only）。
# --------------------------------------------------------------------------- #

step_frontend() {
  run "frontend" bash frontend/verify-frontend.sh
}

# --------------------------------------------------------------------------- #
# live 层：真宿主（真网络、真进程起停；仅需 openssl/curl）。默认**不进 all**。
# --------------------------------------------------------------------------- #

step_live_workbench() { cargo test "${LOCKED[@]}" -p envboard-server --test live_workbench -- --ignored --nocapture; }
step_live_manager()   { cargo test "${LOCKED[@]}" -p envboard-engine --test live_manager -- --ignored --nocapture; }

step_live() {
  run "live/workbench"   step_live_workbench
  run "live/manager"     step_live_manager
}

case "$STEP" in
  all)      step_policy; step_rust; step_contract; step_artifact ;;
  policy)   step_policy ;;
  rust)     step_policy; step_rust ;;
  contract) step_contract ;;
  artifact) step_artifact ;;
  frontend) step_frontend ;;
  live)     step_live ;;
  *)
    echo "unknown step: $STEP" >&2
    echo "可选：all（默认）/ policy / rust / contract / artifact / frontend / live" >&2
    exit 2
    ;;
esac

echo ""
if [[ "$FAILED" -ne 0 ]]; then
  echo "verify: FAILED"
  exit 1
fi
echo "verify: OK"

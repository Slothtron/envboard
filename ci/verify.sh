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
#   PYTHON=... bash ci/verify.sh # 指定适配器宿主的解释器（转发为 ENVBOARD_PYTHON）
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# 一律带 `--locked --offline`：
#   --locked   Cargo.lock 必须与 manifest 一致（可复现构建的前提）
#   --offline  "离线可构建"是验收项；依赖因此必须克制
LOCKED=(--locked --offline)

# 适配器宿主的解释器。**只用于跑适配器脚本**，探测不到时由 require_interpreter 响亮失败。
#
# 探测不到时把变量**赋成空串**而不是留着不定义：`set -u` 下未定义会让 `"$PYTHON" …`
# 以 "unbound variable" 直接终止整个脚本（实测踩过：默认层跑到 artifact 就没了下文，
# 连"缺解释器"这句话都来不及打）。空串只会让那一步失败，`run` 照旧继续往下走。
PYTHON="${PYTHON:-}"
if [[ -z "$PYTHON" ]]; then
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
  if [[ -n "$PYTHON" ]]; then
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
  run "artifact" cargo test -p envboard-cli --test artifact "${LOCKED[@]}"
}

# --------------------------------------------------------------------------- #
# adapter 层：宿主适配器（对拍 + 冒烟）
#
# 对拍是两份 hosts 解析实现（Rust 与注入器）一致性的唯一护栏，所以它留在默认层；
# 它需要解释器，而且解释器缺失时要**响亮失败**，不静默跳过。
# --------------------------------------------------------------------------- #

# 把本脚本探测到的解释器**转发**给对拍测试（`ENVBOARD_PYTHON` 是那个测试认的开关），
# 这样 `PYTHON=<路径> bash ci/verify.sh` 一个旋钮就同时管住探测与测试。
step_dual() { ENVBOARD_PYTHON="$PYTHON" cargo test -p envboard-rules --test dual_impl "${LOCKED[@]}" -- --ignored; }

step_adapter() {
  require_interpreter || return 0
  run "adapter/dual" step_dual
}

# --------------------------------------------------------------------------- #
# live 层：真宿主（真 mitmdump + 真网络）。默认**不进 all**。
# --------------------------------------------------------------------------- #

step_live_workbench() { cargo test "${LOCKED[@]}" -p envboard-cli --test live_workbench -- --ignored --nocapture; }
step_live_manager()   { cargo test "${LOCKED[@]}" -p envboard-core --test live_manager -- --ignored --nocapture; }
step_live_thin()      { cargo test "${LOCKED[@]}" -p envboard-cli --test thin_client -- --nocapture; }

step_live() {
  run "live/workbench"   step_live_workbench
  run "live/manager"     step_live_manager
  run "live/thin-client" step_live_thin
}

case "$STEP" in
  all)      step_policy; step_rust; step_contract; step_artifact; step_adapter ;;
  policy)   step_policy ;;
  rust)     step_policy; step_rust ;;
  contract) step_contract ;;
  artifact) step_artifact ;;
  adapter)  step_adapter ;;
  live)     step_live ;;
  *)
    echo "unknown step: $STEP" >&2
    echo "可选：all（默认）/ policy / rust / contract / artifact / adapter / live" >&2
    exit 2
    ;;
esac

echo ""
if [[ "$FAILED" -ne 0 ]]; then
  echo "verify: FAILED"
  exit 1
fi
echo "verify: OK"

#!/usr/bin/env python3
# envboard 的单一验证入口：CI 只调它一个。
#
# 托管方无关：任何 CI 直接调用 `python scripts/verify.py` 即可；本仓库不绑定 GitHub
# Actions。仅用标准库，跨平台（Linux / macOS / Windows 原生），不依赖 bash。
#
# 本脚本**只做编排**：分层、顺序、依赖探测与响亮失败。判据一律在 Rust 测试里
# （`tests/policy-tests` 一门禁一文件，可单跑），脚本里不写断言。
#
# 层：
#   policy   仓库纪律（工具链收敛 / 命名 / 文本自包含 / 依赖方向）—— 纯 Rust，无外部语言
#   rust     fmt / clippy / check / build / test —— 纯 Rust
#   contract 契约 fixture 的形状与消费
#   artifact 发布物与产物纪律
#   frontend 前端工程（typecheck + build）—— **Rust 构建的前置**，在 all 最前；
#            pnpm 调用圈禁在 frontend/ 边界内（本脚本对 frontend/ 之外不碰 pnpm，
#            见 toolchain 门禁 R3）。无前端工具链但 dist 已在位时以既有产物通过
#            （cargo 只消费 dist）。
#   live     实机层（真网络、真进程起停；仅需 openssl/curl），默认**不在** all 里
#
# 用法：
#   python scripts/verify.py            # 默认层：frontend + policy + rust + contract + artifact
#   python scripts/verify.py rust       # 纯 Rust 子集（policy + rust；要求 dist 已在位）
#   python scripts/verify.py frontend   # 只跑前端构建层
#   python scripts/verify.py live       # 实机层（需要宿主）
"""__doc__ 由上方注释承担，模块本体只做编排。"""

import os
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FRONTEND = os.path.join(ROOT, "frontend")
DIST_INDEX = os.path.join(FRONTEND, "dist", "index.html")

# 一律带 `--locked --offline`：
#   --locked   Cargo.lock 必须与 manifest 一致（可复现构建的前提）
#   --offline  "离线可构建"是验收项；依赖因此必须克制
LOCKED = ["--locked", "--offline"]

FAILED = False


def tool(name: str) -> str:
    """解析可执行文件绝对路径；缺解释器**响亮失败**，并给出两条出路；不静默跳过
    （跳过等于护栏消失）。兼容 mise shims 与 Windows（.exe/.cmd 由 which 兜住）。"""
    path = shutil.which(name)
    if path is None:
        sys.exit(f"!!! verify: 未找到 {name} —— 出路一：mise install（工具版本由 mise.toml 锁定）；"
                 f"出路二：用系统包管理器安装后重跑")
    return path


def run(name: str, argv: list[str], cwd: str = ROOT) -> bool:
    global FAILED
    print()
    print(f"=== {name} ===")
    proc = subprocess.run([*argv], cwd=cwd)
    if proc.returncode != 0:
        print(f"!!! {name} FAILED")
        FAILED = True
        return False
    return True


# --------------------------------------------------------------------------- #
# policy 层：仓库纪律
# --------------------------------------------------------------------------- #

def step_policy() -> None:
    run("policy/gates", [tool("cargo"), "test", "-p", "envboard-policy-tests", *LOCKED])


# --------------------------------------------------------------------------- #
# rust 层：格式 / lint / 编译 / 单测
#
# fmt 与 clippy 随 toolchain（rust-toolchain.toml 钉死的 1.98.0）一起来，
# 所以它们是**必需**步骤 —— 缺了就红，不再有 "SKIPPED" 这条路。
# --------------------------------------------------------------------------- #

def step_rust() -> None:
    cargo = tool("cargo")
    run("rust-fmt", [cargo, "fmt", "--all", "--check"])
    run("rust-clippy", [cargo, "clippy", "--workspace", "--all-targets",
                        "--offline", "--", "-D", "warnings"])
    run("rust-check", [cargo, "check", "--workspace", "--all-targets", *LOCKED])
    run("rust-build", [cargo, "build", "--workspace", *LOCKED])
    run("rust-test", [cargo, "test", "--workspace", *LOCKED])


# --------------------------------------------------------------------------- #
# contract 层：fixture 形状 + 两侧消费
# --------------------------------------------------------------------------- #

def step_contract() -> None:
    run("contract-tests", [tool("cargo"), "test", "-p", "envboard-contract-tests", *LOCKED])


# --------------------------------------------------------------------------- #
# artifact 层：发布物内容
# --------------------------------------------------------------------------- #

def step_artifact() -> None:
    run("artifact", [tool("cargo"), "test", "-p", "envboard-server", "--test", "artifact", *LOCKED])


# --------------------------------------------------------------------------- #
# frontend 层：Vite 工程（显式层；pnpm 调用圈禁在 frontend/ 边界内）。
# --------------------------------------------------------------------------- #

def step_frontend() -> None:
    global FAILED
    frontend_pnpm = shutil.which("pnpm")  # frontend 边界内的前端工具链
    if frontend_pnpm is None:
        # 无前端工具链：dist 已在位则以既有产物通过（cargo 只消费产物）；
        # 缺 dist 才失败 —— 构建顺序是先前端后 Rust，两条出路都要响亮给出。
        if os.path.isfile(DIST_INDEX):
            print("frontend: SKIP —— 未找到 pnpm，使用已在位的 dist/"
                  "（改 frontend 源码需装 pnpm 重跑本层）")
            return
        print("!!! frontend: 未找到 pnpm 且 dist/ 缺失 —— 构建顺序是先前端后 Rust",
              file=sys.stderr)
        print("    出路一：mise install（frontend 工具链 node/pnpm 版本由 mise.toml 锁定）后重跑",
              file=sys.stderr)
        print("    出路二：在有前端工具链的机器上构建，把 dist/ 带到本地（产物不入库）",
              file=sys.stderr)
        FAILED = True
        return

    run("frontend/install", [frontend_pnpm, "install", "--frozen-lockfile"], cwd=FRONTEND)
    run("frontend/typecheck", [frontend_pnpm, "run", "typecheck"], cwd=FRONTEND)
    run("frontend/build", [frontend_pnpm, "run", "build"], cwd=FRONTEND)

    # 产物在位即达成本层职责（cargo 侧由 crates/web/build.rs 校验并消费）。
    if not os.path.isfile(DIST_INDEX):
        print("!!! frontend: 构建后 dist/index.html 仍缺失", file=sys.stderr)
        FAILED = True
        return
    print("frontend: OK（dist 已就绪，可进入 cargo 构建）")


# --------------------------------------------------------------------------- #
# live 层：真宿主（真网络、真进程起停；仅需 openssl/curl）。默认**不进 all**。
# --------------------------------------------------------------------------- #

def step_live() -> None:
    cargo = tool("cargo")
    run("live/workbench", [cargo, "test", *LOCKED, "-p", "envboard-server",
                           "--test", "live_workbench", "--", "--ignored", "--nocapture"])
    run("live/manager", [cargo, "test", *LOCKED, "-p", "envboard-engine",
                         "--test", "live_manager", "--", "--ignored", "--nocapture"])


STEPS = {
    "all": [step_frontend, step_policy, step_rust, step_contract, step_artifact],
    "policy": [step_policy],
    "rust": [step_policy, step_rust],
    "contract": [step_contract],
    "artifact": [step_artifact],
    "frontend": [step_frontend],
    "live": [step_live],
}


def main() -> int:
    name = sys.argv[1] if len(sys.argv) > 1 else "all"
    steps = STEPS.get(name)
    if steps is None:
        print(f"unknown step: {name}", file=sys.stderr)
        print("可选：all（默认）/ frontend / policy / rust / contract / artifact / live",
              file=sys.stderr)
        return 2
    for step in steps:
        step()
    print()
    if FAILED:
        print("verify: FAILED")
        return 1
    print("verify: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())

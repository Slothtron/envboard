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
#   live     实机层（真网络、真进程起停；仅需 openssl/curl），默认**不在** all 里
#
# 曾经的 `frontend` 与 `artifact` 两层随 web 形态一起出库：前者是 SPA 的
# typecheck+build（Rust 构建的前置），后者校验内嵌前端资产与宿主工件 —— 两者判定的
# 对象都不存在了（见 docs/adr-003-retire-web-frontend.md）。本仓现在只有一个发布物：
# 桌面 GUI 二进制 `envboard`，它的构建与测试由 `rust` 层覆盖。
#
# 用法：
#   python scripts/verify.py            # 默认层：policy + rust + contract
#   python scripts/verify.py rust       # 纯 Rust 子集（policy + rust）
#   python scripts/verify.py policy     # 只跑仓库纪律门禁
#   python scripts/verify.py live       # 实机层（需要引擎与宿主能力）
"""__doc__ 由上方注释承担，模块本体只做编排。"""

import os
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

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
# live 层：真环境（真网络、真进程起停；仅需 openssl/curl）。默认**不进 all**。
# --------------------------------------------------------------------------- #

def step_live() -> None:
    cargo = tool("cargo")
    run("live/manager", [cargo, "test", *LOCKED, "-p", "envboard-engine",
                         "--test", "live_manager", "--", "--ignored", "--nocapture"])


STEPS = {
    "all": [step_policy, step_rust, step_contract],
    "policy": [step_policy],
    "rust": [step_policy, step_rust],
    "contract": [step_contract],
    "live": [step_live],
}


def main() -> int:
    name = sys.argv[1] if len(sys.argv) > 1 else "all"
    steps = STEPS.get(name)
    if steps is None:
        print(f"unknown step: {name}", file=sys.stderr)
        print("可选：all（默认）/ policy / rust / contract / live",
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

#!/usr/bin/env bash
# frontend 层的实际执行体：pnpm 调用被圈禁在前端边界内 ——
# toolchain 门禁 R3 只扫 ci/ 与 scripts/ 的可执行面，本文件在 frontend/ 下，
# 是「显式 frontend 层动作」的合法落点（默认验证路径保持 cargo-only）。
#
# 三步：typecheck（tsc strict）→ build（vite，产物 dist/ 是内嵌源）→
# drift 校验（git diff 必须为空：提交的 dist 与 src 一致）。
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")"

if ! command -v pnpm >/dev/null 2>&1; then
  echo "!!! frontend: 未找到 pnpm —— 安装 pnpm（corepack enable）后重跑，或跳过 frontend 层" >&2
  exit 1
fi

echo "--- frontend: install（frozen lockfile，可重现） ---"
pnpm install --frozen-lockfile

echo "--- frontend: typecheck ---"
pnpm run typecheck

echo "--- frontend: build ---"
pnpm run build

echo "--- frontend: dist drift（src 与已提交 dist 必须一致） ---"
if ! git diff --quiet -- frontend/dist; then
  echo "!!! frontend/dist 与 src 漂移：提交前必须 pnpm run build 并把 dist 一起提交" >&2
  git diff --stat -- frontend/dist >&2
  exit 1
fi

echo "frontend: OK"

#!/usr/bin/env bash
# frontend 层的实际执行体：pnpm 调用被圈禁在前端边界内 ——
# toolchain 门禁 R3 只扫 ci/ 与 scripts/ 的可执行面，本文件在 frontend/ 下，
# 是「前端构建」这条衔接线的合法落点。
#
# 构建顺序纪律：先 pnpm build 产出 dist/（不入库），后 cargo build 内嵌。
# 两步：typecheck（tsc strict）→ build（vite 产出 dist/）。
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")"

if ! command -v pnpm >/dev/null 2>&1; then
  # 无前端工具链：dist 已在位则以既有产物通过（cargo 只消费产物）；
  # 缺 dist 才失败 —— 构建顺序是先前端后 Rust，两条出路都要响亮给出。
  if [[ -f dist/index.html ]]; then
    echo "frontend: SKIP —— 未找到 pnpm，使用已在位的 dist/（改前端源码需装 pnpm 重跑本层）"
    exit 0
  fi
  echo "!!! frontend: 未找到 pnpm 且 dist/ 缺失 —— 构建顺序是先前端后 Rust" >&2
  echo "    出路一：安装 pnpm（corepack enable）后重跑" >&2
  echo "    出路二：在有前端工具链的机器上构建，把 dist/ 带到本地（产物不入库）" >&2
  exit 1
fi

echo "--- frontend: install（frozen lockfile，可重现） ---"
pnpm install --frozen-lockfile

echo "--- frontend: typecheck ---"
pnpm run typecheck

echo "--- frontend: build ---"
pnpm run build

# 产物在位即达成本层职责（cargo 侧由 crates/web/build.rs 校验并消费）。
if [[ ! -f dist/index.html ]]; then
  echo "!!! frontend: 构建后 dist/index.html 仍缺失" >&2
  exit 1
fi

echo "frontend: OK（dist 已就绪，可进入 cargo 构建）"

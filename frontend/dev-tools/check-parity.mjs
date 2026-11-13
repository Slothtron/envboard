/**
 * 命令面 parity 门禁（dev-tools）：mock 注册表必须与壳的 generate_handler! 逐条对齐。
 *
 * 纯 node、无依赖：从 crates/app/desktop/src/lib.rs 提取 commands::* / stream::*，
 * 与 fake-engine.ts 的 MOCK_COMMANDS 双向比对。新增命令时这里先红，逼着 mock 跟上。
 * 挂接：mise run verify-parity（并入 verify-frontend）。
 */

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "..", "..");

const libRs = readFileSync(join(repoRoot, "crates/app/desktop/src/lib.rs"), "utf8");
const handlerBlock = libRs.match(/generate_handler!\[([\s\S]*?)\]/);
if (!handlerBlock) {
  console.error("parity: lib.rs 里找不到 generate_handler! 块");
  process.exit(1);
}
const shellCommands = new Set(
  [...handlerBlock[1].matchAll(/(?:commands|stream)::([a-z_]+)/g)].map((m) => m[1]),
);

const engineTs = readFileSync(join(here, "fake-engine.ts"), "utf8");
const registryBlock = engineTs.match(/MOCK_COMMANDS[^=]*=\s*\[([\s\S]*?)\]/);
if (!registryBlock) {
  console.error("parity: fake-engine.ts 里找不到 MOCK_COMMANDS 注册表");
  process.exit(1);
}
const mockCommands = new Set(
  [...registryBlock[1].matchAll(/"([a-z_]+)"/g)].map((m) => m[1]),
);

const missing = [...shellCommands].filter((c) => !mockCommands.has(c));
const extra = [...mockCommands].filter((c) => !shellCommands.has(c));

if (missing.length > 0 || extra.length > 0) {
  if (missing.length > 0) console.error(`parity: mock 缺命令 → ${missing.join(", ")}`);
  if (extra.length > 0) console.error(`parity: mock 多出命令（壳已退场？）→ ${extra.join(", ")}`);
  process.exit(1);
}
console.log(`parity OK：${shellCommands.size} 条命令逐条对齐（壳 = mock）`);

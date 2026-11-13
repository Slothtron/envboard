/**
 * `@tauri-apps/api/core` 的浏览器侧替身（dev-tools，不进发布产物）。
 *
 * 只在 `VITE_TRANSPORT=mock` 时由 vite.config 的 alias 引入——发布构建不设该
 * 变量，本文件物理不可达（artifact 测试另钉了一条「dist 不得含 __envboardMock」）。
 *
 * 前端对 Tauri 的全部消费只有两个导出：`invoke`（client.ts）与
 * `Channel, invoke`（stream.ts）。错误信封裸抛出，与真 IPC 的 reject 形状一致
 * （client.ts 的 call() 按 {code,message} 识别）。
 */

import { dispatch, registerStream, type ChannelLike } from "./fake-engine";

/** 真 Channel 的最小替身：stream.ts 只赋 onmessage、只经 invoke 传回壳侧。 */
export class Channel<T = unknown> implements ChannelLike {
  onmessage: ((message: T) => void) | null = null;
}

/** mock 标记：artifact 测试 grep dist 不得含它；走查脚本也可据此识别 mock 档。 */
declare global {
  interface Window {
    __envboardMock?: boolean;
    __envboardRecreated?: boolean;
  }
}

window.__envboardMock = true;
// boot 自检屏只属于「进程的首次窗口」（ADR-012）：浏览器走查不是首窗，跳过。
window.__envboardRecreated = true;
console.info(
  "[envboard mock] 假引擎已接管 IPC：内存账本 + 模拟推送，刷新即重置（主题除外）。不起 Tauri 壳、不碰真端口/信任库。",
);

export async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  // 一点真实延迟：让加载态/乐观更新的时序在走查里可见。
  await new Promise((resolve) => setTimeout(resolve, 10));
  const payload = args ?? {};
  if (command === "watch_environments" || command === "watch_debug") {
    const channel = payload.onEvent;
    if (!channel || typeof (channel as ChannelLike).onmessage === "undefined") {
      throw { code: "invalid_config", message: "watch command requires an onEvent channel" };
    }
    return registerStream(command, channel as ChannelLike) as T;
  }
  return dispatch(command, payload) as T;
}

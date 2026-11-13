/**
 * 推送流绑定（Tauri Channel，ADR-006）。两条流的游标域不同，不得混用
 * （envboard-protocol crate 文档）：
 * - watch_environments：快照流，帧 `snapshot` = SnapshotFrame，cursor 为 advisory 代次；
 * - watch_debug：抓包流，`snapshot`（整幅）+ `events`（增量，request_id 域）。
 *
 * 每帧 = `{event, data}`（event 名沿用退场 SSE 的词汇）。命令返回流 id，
 * `close()` 调 `unwatch(id)` 取消 Rust 侧轮询任务 —— 只丢 Channel 不够，
 * 那会泄漏任务（见 crates/app/desktop/src/stream.rs 头注释）。
 * 连接态回调给 UI（侧栏底部指示 + 顶栏徽章）：命令登记成功 = 已连接。
 */

import { Channel, invoke } from "@tauri-apps/api/core";

import type { DebugEvents, DebugSnapshot, SnapshotFrame } from "./types";

export interface StreamHandle {
  close(): void;
}

interface StreamFrame {
  event: string;
  data: unknown;
}

function open(
  command: string,
  args: Record<string, unknown>,
  handlers: Record<string, (frame: unknown) => void>,
  onConn: (connected: boolean) => void,
): StreamHandle {
  let closed = false;
  let streamId: number | null = null;
  const channel = new Channel<StreamFrame>();
  channel.onmessage = (message) => {
    if (closed) return;
    const handler = handlers[message.event];
    if (handler) {
      handler(message.data);
    } else {
      // 未知帧名 = 契约漂移，响亮但不断流（UI 保留上一帧）
      console.error(`IPC 流帧名未识别：${command} / ${message.event}`);
    }
  };
  invoke<number>(command, { ...args, onEvent: channel })
    .then((id) => {
      if (closed) {
        // 登记完成前就关了：补一次取消，不留孤儿任务。
        void invoke("unwatch", { id });
        return;
      }
      streamId = id;
      onConn(true);
    })
    .catch(() => onConn(false));
  return {
    close: () => {
      closed = true;
      if (streamId !== null) void invoke("unwatch", { id: streamId });
    },
  };
}

/** 快照流：每秒一帧全环境视图。 */
export function openSnapshotStream(
  onFrame: (frame: SnapshotFrame) => void,
  onConn: (connected: boolean) => void,
): StreamHandle {
  return open(
    "watch_environments",
    {},
    { snapshot: (frame) => onFrame(frame as SnapshotFrame) },
    onConn,
  );
}

/** 调试抓包流。 */
export function openDebugStream(
  onSnapshot: (frame: DebugSnapshot) => void,
  onEvents: (frame: DebugEvents) => void,
  onConn: (connected: boolean) => void,
): StreamHandle {
  return open(
    "watch_debug",
    {},
    {
      snapshot: (frame) => onSnapshot(frame as DebugSnapshot),
      events: (frame) => onEvents(frame as DebugEvents),
    },
    onConn,
  );
}

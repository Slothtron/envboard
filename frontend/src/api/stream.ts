/**
 * SSE 流绑定（EventSource）。三条流的游标域不同，不得混用（spec/protocol.md）：
 * - /api/events：快照流，帧 `snapshot` = SnapshotFrame，cursor 为 advisory 代次；
 * - /api/debug/stream：抓包流，`snapshot`（整幅）+ `events`（增量，request_id 域）；
 * - /api/environments/:name/trajectory/stream：轨迹流，`baseline` + `events`（字节偏移域）。
 *
 * token 只能走 query（EventSource 不支持自定义头）。断线由 EventSource 自动重连，
 * 连接态回调给 UI（侧栏底部指示 + 顶栏徽章）。
 */

import { tokenQuery, tokenValue } from "./client";
import type {
  DebugEvents,
  DebugSnapshot,
  SnapshotFrame,
  TrajectoryWindow,
} from "./types";

export interface StreamHandle {
  close(): void;
}

function open(url: string, onConn: (connected: boolean) => void): EventSource {
  const source = new EventSource(url);
  source.onopen = () => onConn(true);
  source.onerror = () => onConn(false);
  return source;
}

function onJson<T>(source: EventSource, event: string, handler: (frame: T) => void): void {
  source.addEventListener(event, (message) => {
    try {
      handler(JSON.parse((message as MessageEvent).data) as T);
    } catch {
      // 帧解析失败 = 契约漂移，响亮但不断流（UI 保留上一帧）
      console.error(`SSE 帧解析失败：${event}`);
    }
  });
}

/** 快照流：每秒一帧全环境视图。 */
export function openSnapshotStream(
  onFrame: (frame: SnapshotFrame) => void,
  onConn: (connected: boolean) => void,
): StreamHandle {
  const source = open(`/api/events${tokenQuery()}`, onConn);
  onJson<SnapshotFrame>(source, "snapshot", onFrame);
  return { close: () => source.close() };
}

/** 调试抓包流。 */
export function openDebugStream(
  onSnapshot: (frame: DebugSnapshot) => void,
  onEvents: (frame: DebugEvents) => void,
  onConn: (connected: boolean) => void,
): StreamHandle {
  const source = open(`/api/debug/stream${tokenQuery()}`, onConn);
  onJson<DebugSnapshot>(source, "snapshot", onSnapshot);
  onJson<DebugEvents>(source, "events", onEvents);
  return { close: () => source.close() };
}

/** 轨迹流（环境详情「轨迹」页签）；cursor 为字节偏移，断点续传。 */
export function openTrajectoryStream(
  env: string,
  cursor: number | null,
  onBaseline: (frame: TrajectoryWindow) => void,
  onEvents: (frame: TrajectoryWindow) => void,
  onConn: (connected: boolean) => void,
): StreamHandle {
  const base = `/api/environments/${encodeURIComponent(env)}/trajectory/stream`;
  const params = new URLSearchParams();
  if (cursor !== null) params.set("cursor", String(cursor));
  const token = tokenValue();
  if (token) params.set("token", token);
  const query = params.toString();
  const source = open(query ? `${base}?${query}` : base, onConn);
  onJson<TrajectoryWindow>(source, "baseline", onBaseline);
  onJson<TrajectoryWindow>(source, "events", onEvents);
  return { close: () => source.close() };
}

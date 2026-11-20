import {
  memo,
  useCallback,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";

import { formatBytes } from "../data";
import { MethodChip, statusTone } from "../shared";
import type { CaptureRecord } from "../api/types";
import { TruncatedText } from "./CaptureView";

/**
 * 抓包记录表：原生 <table> + 手写窗口化（只渲染可视区 ± 缓冲行）。
 *
 * 为什么不用 HeroUI Table：react-aria Collection 无虚拟化，500ms 一帧的增量
 * 推送会全量重渲染所有行（每行还挂 2 个 Tooltip 的 press/hover 管理），实测
 * 渲染耗时随行数线性涨（76 行 P50≈244ms），长会话直接卡死。原生行 + memo：
 * merge 保持旧记录引用不变 → 未变化的行整棵跳过；窗口化把 DOM 钉在 ~60 行。
 *
 * 窗口化用上下 spacer <tr> 撑出总高（不用 transform：table 布局里 transform
 * 会破坏 sticky 表头与列宽）。行高钉死 --wb-cap-row-h，spacer 高度 = 行数×行高，
 * 与实际渲染行高一致是窗口不抖的前提（走查断言 28–36px 同口径）。
 *
 * 滚动锚定：贴底时新记录自动跟随（DevTools 行为）；用户上滚阅读时不抢占滚动条。
 */

/** 与 globals.css 的 --wb-cap-row-h 同值（29px = 20px chip + 8px padding + 1px 边）。 */
const ROW_H = 29;
/** 可视区上下各多渲染的行数：快速滚动时减少白屏。 */
const OVERSCAN = 12;

interface CaptureTableProps {
  records: CaptureRecord[];
  selectedId: number | null;
  onSelect: (id: number) => void;
  onOpen: (cap: CaptureRecord) => void;
  /** 过滤后为空时的占位（工具条「清除过滤」入口在里面）。 */
  emptyState: ReactNode;
  /** 抽屉打开时列宽让位（min-w 收起，与旧 HeroUI 行为一致）。 */
  drawerOpen: boolean;
}

export function CaptureTable({ records, selectedId, onSelect, onOpen, emptyState, drawerOpen }: CaptureTableProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewH, setViewH] = useState(0);
  /* 贴底跟随：初始贴底；用户滚离底部即停止跟随，滚回底部恢复。
     following 进 state 供「回到底部」按钮显隐（stickRef 是它的同步镜像，
     供 layout effect 读最新值，避免闭包拿到旧 state）。 */
  const [following, setFollowing] = useState(true);
  const stickRef = useRef(true);
  const setStick = useCallback((v: boolean) => {
    stickRef.current = v;
    setFollowing(v);
  }, []);

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setViewH(el.clientHeight));
    ro.observe(el);
    setViewH(el.clientHeight);
    return () => ro.disconnect();
  }, []);

  /* 新记录到达：贴底状态下跟随到最新（layout effect 里 DOM 已含新 spacer）。
     只依赖「记录条数变化」——records 引用每帧都变但条数通常不变，
     若按引用触发会把用户的滚轮/阅读位置每 500ms 抢回底部。 */
  const count = records.length;
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el || !stickRef.current) return;
    el.scrollTop = el.scrollHeight;
    setScrollTop(el.scrollTop);
  }, [count, viewH]);

  const onScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    setScrollTop(el.scrollTop);
    setStick(el.scrollHeight - el.scrollTop - el.clientHeight < ROW_H * 2);
  }, [setStick]);

  /* 「回到底部」：滚到最新并恢复自动跟随。 */
  const scrollToBottom = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    setStick(true);
    el.scrollTop = el.scrollHeight;
    setScrollTop(el.scrollTop);
  }, [setStick]);

  /* 可视窗口：首尾各留 OVERSCAN；总高由 spacer 撑出。 */
  const total = records.length;
  const start = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
  const end = Math.min(total, Math.ceil((scrollTop + viewH) / ROW_H) + OVERSCAN);
  const windowRows = records.slice(start, end);
  const padTop = start * ROW_H;
  const padBottom = (total - end) * ROW_H;

  return (
    <div className="relative h-full min-h-0">
      <div
        className="wb-cap-scroll h-full min-h-0 overflow-auto"
        data-full-bleed
        onScroll={onScroll}
        ref={scrollRef}
      >
        <table
          aria-label="抓包记录"
          className={`wb-cap-table table-fixed ${drawerOpen ? "min-w-0" : "min-w-240"}`}
        >
          <colgroup>
            <col className="w-14" />
            <col className="w-20" />
            <col className="w-16" />
            <col className="w-[30%]" />
            <col />
            <col className="w-24" />
          </colgroup>
          <thead className="sticky top-0 z-10">
            <tr>
              <th scope="col">#</th>
              <th scope="col">方法</th>
              <th scope="col">状态</th>
              <th scope="col">域名</th>
              <th scope="col">路径</th>
              <th scope="col" className="text-end">大小</th>
            </tr>
          </thead>
          {total === 0 ? (
            <tbody>
              <tr className="wb-cap-empty">
                <td colSpan={6}>{emptyState}</td>
              </tr>
            </tbody>
          ) : (
            <>
              {padTop > 0 ? (
                <tbody aria-hidden="true">
                  <tr className="wb-cap-spacer" style={{ height: padTop }} />
                </tbody>
              ) : null}
              <tbody>
                {windowRows.map((cap) => (
                  <CaptureRow
                    cap={cap}
                    key={cap.request_id}
                    selected={cap.request_id === selectedId}
                    onSelect={onSelect}
                    onOpen={onOpen}
                  />
                ))}
              </tbody>
              {padBottom > 0 ? (
                <tbody aria-hidden="true">
                  <tr className="wb-cap-spacer" style={{ height: padBottom }} />
                </tbody>
              ) : null}
            </>
          )}
        </table>
      </div>
      {/* 非贴底时的「回到底部」快捷按钮：点击滚到最新并恢复自动跟随。
          定位避开右侧滚动条（right-4）与底部，悬浮不占布局。 */}
      {!following && total > 0 ? (
        <button
          aria-label="滚动到底部并跟随最新"
          className="wb-cap-tobottom"
          onClick={scrollToBottom}
          title="滚动到底部并跟随最新"
          type="button"
        >
          <svg aria-hidden="true" className="size-4" fill="none" stroke="currentColor" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" viewBox="0 0 24 24">
            <path d="M12 4v13" />
            <path d="m6 12 6 6 6-6" />
            <path d="M5 21h14" />
          </svg>
        </button>
      ) : null}
    </div>
  );
}

/* ------------------------------------------------------------------ */

interface CaptureRowProps {
  cap: CaptureRecord;
  selected: boolean;
  onSelect: (id: number) => void;
  onOpen: (cap: CaptureRecord) => void;
}

/**
 * 单行 memo：cap 引用由 mergeRecords 保持稳定（未变化的记录不换对象），
 * selected/onSelect/onOpen 也是稳定引用 → 增量帧只重渲染新增/变化的行。
 * 键盘焦点走 roving tabindex：选中行 tabIndex=0，其余 -1（避免几百个停靠点）。
 */
const CaptureRow = memo(function CaptureRow({ cap, selected, onSelect, onOpen }: CaptureRowProps) {
  const status = cap.response?.status ?? 0;
  return (
    <tr
      aria-selected={selected}
      className="wb-cap-row cursor-pointer"
      data-cap-row={cap.request_id}
      data-selected={selected || undefined}
      onClick={() => onSelect(cap.request_id)}
      onDoubleClick={() => onOpen(cap)}
      tabIndex={selected ? 0 : -1}
    >
      <td>
        <span className="font-mono text-sm text-muted">{cap.request_id}</span>
      </td>
      <td>
        <MethodChip method={cap.request.method} />
      </td>
      <td>
        <span
          className={
            cap.error !== null
              ? "font-mono font-semibold text-danger"
              : cap.response
                ? statusTone(status)
                : "font-mono font-semibold text-muted"
          }
        >
          {cap.error !== null ? "ERR" : (cap.response?.status ?? "—")}
        </span>
      </td>
      <td>
        <TruncatedText text={cap.request.authority} className="font-mono text-sm text-muted" />
      </td>
      <td>
        <TruncatedText text={cap.request.path} className="font-mono text-sm text-foreground" />
      </td>
      <td className="text-end">
        <span className="font-mono text-sm text-muted">
          {formatBytes((cap.request.body?.size ?? 0) + (cap.response?.body?.size ?? 0))}
        </span>
      </td>
    </tr>
  );
});
import { useCallback, useEffect, useRef, useState, type CSSProperties, type HTMLAttributes, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { GripVertical } from "lucide-react";
import { defaultRangeExtractor, useVirtualizer, type Range } from "@tanstack/react-virtual";
import { cx } from "./cx";
import s from "./SortableVirtualList.module.css";

export interface RowCtx {
  index: number;
  /** This is the row being dragged (its slot, not the floating ghost). */
  dragging: boolean;
  /** Rendering the floating ghost copy. */
  ghost: boolean;
  /** False when reorder is disabled (search active, sort not load order...). Hide the handle then. */
  reorderable: boolean;
  /** Spread on the grab handle. */
  handleProps: HTMLAttributes<HTMLElement>;
}

interface Props<T> {
  items: T[];
  getKey: (item: T) => string;
  renderRow: (item: T, ctx: RowCtx) => ReactNode;
  /** `beforeKey` is the key of the row the item now sits before (null = last). Update `items` optimistically. */
  onMove: (key: string, beforeKey: string | null, from: number, to: number) => void;
  rowHeight?: number;
  overscan?: number;
  disabled?: boolean;
  /** Accessible name of a row's handle. */
  rowLabel?: (item: T) => string;
  label?: string;
  className?: string;
  /** Changing this (e.g. the active playset) cancels a drag in progress. */
  scope?: string;
}

const EDGE = 48;
const MAX_SPEED = 18;

interface Drag { key: string; from: number; over: number; y: number; grab: number; left: number; width: number; top: number; bottom: number; }

/** `<GripVertical>` handle for a row; null when reorder is disabled so the slot collapses. */
export const dragHandle = (ctx: RowCtx) =>
  ctx.reorderable ? (
    <span {...ctx.handleProps} className={cx(s.handle, ctx.handleProps.className)}>
      <GripVertical />
    </span>
  ) : null;

/**
 * Fixed-height virtual list with pointer reorder: pointer capture, rAF edge autoscroll, Esc cancels,
 * Alt+Up/Down on a focused handle. Drop slot = floor((pointerY - listTop + scrollTop) / rowHeight).
 * The dragged row stays mounted while scrolled out of view; a fixed ghost follows the pointer.
 */
export function SortableVirtualList<T>({ items, getKey, renderRow, onMove, rowHeight = 56, overscan = 8, disabled, rowLabel, label, className, scope }: Props<T>) {
  const scroller = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<Drag | null>(null);
  const dragRef = useRef<Drag | null>(null);
  const [said, setSaid] = useState("");
  // Same signature the reset effect below uses; the commit re-checks it in case the rows changed before that effect ran.
  const keysSig = items.map(getKey).join("\u0000");
  const live = useRef({ items, getKey, onMove, rowHeight, keysSig, scope, disabled });
  live.current = { items, getKey, onMove, rowHeight, keysSig, scope, disabled };
  const stop = useRef<(() => void) | null>(null);

  const rangeExtractor = useCallback((r: Range) => {
    const base = defaultRangeExtractor(r);
    const f = dragRef.current?.from;
    if (f != null && !base.includes(f)) {
      base.push(f);
      base.sort((a, b) => a - b);
    }
    return base;
  }, []);
  const v = useVirtualizer({ count: items.length, getScrollElement: () => scroller.current, estimateSize: () => rowHeight, overscan, rangeExtractor });

  const setBoth = (d: Drag | null) => ((dragRef.current = d), setDrag(d));
  useEffect(() => () => stop.current?.(), []);
  // The rows under the pointer changed (reload, other playset, reorder switched off): a drag over old slots would drop in the wrong place.
  useEffect(() => { stop.current?.(); }, [keysSig, scope, disabled]);

  const start = (e: React.PointerEvent<HTMLElement>, index: number) => {
    const el = scroller.current;
    if (disabled || e.button !== 0 || !el || dragRef.current) return; // a second pointer must not start a second drag
    e.preventDefault();
    const { items, getKey, rowHeight: h, keysSig: sig0, scope: scope0 } = live.current;
    const rect = el.getBoundingClientRect();
    const grab = e.clientY - (rect.top - el.scrollTop + index * h);
    try { e.currentTarget.setPointerCapture(e.pointerId); } catch { /* handle may be gone; window listeners still work */ }
    setBoth({ key: getKey(items[index]), from: index, over: index, y: e.clientY, grab, left: rect.left, width: el.clientWidth, top: rect.top, bottom: rect.bottom });
    const body = document.body.style;
    const prev = [body.userSelect, body.cursor];
    body.userSelect = "none";
    body.cursor = "grabbing";
    let y = e.clientY;
    let raf = 0;
    const pid = e.pointerId;

    const tick = () => {
      const d = dragRef.current;
      if (!d) return;
      const r = el.getBoundingClientRect();
      const { items, rowHeight: h } = live.current;
      if (y < r.top + EDGE) el.scrollTop -= Math.ceil(MAX_SPEED * Math.min(1, (r.top + EDGE - y) / EDGE));
      else if (y > r.bottom - EDGE) el.scrollTop += Math.ceil(MAX_SPEED * Math.min(1, (y - (r.bottom - EDGE)) / EDGE));
      const over = Math.max(0, Math.min(items.length - 1, Math.floor((y - r.top + el.scrollTop) / h)));
      if (over !== d.over || y !== d.y) setBoth({ ...d, over, y, top: r.top, bottom: r.bottom, left: r.left });
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);

    const move = (ev: PointerEvent) => { if (ev.pointerId === pid) y = ev.clientY; };
    const finish = (commit: boolean) => {
      stop.current = null;
      cancelAnimationFrame(raf);
      removeEventListener("pointermove", move);
      removeEventListener("pointerup", up);
      removeEventListener("pointercancel", cancel);
      removeEventListener("keydown", key, true);
      removeEventListener("blur", cancel);
      [body.userSelect, body.cursor] = prev;
      const d = dragRef.current;
      setBoth(null);
      const now = live.current;
      if (commit && d && d.over !== d.from && !now.disabled && now.keysSig === sig0 && now.scope === scope0 && now.items[d.from] && now.getKey(now.items[d.from]) === d.key) {
        const { items, getKey, onMove } = now;
        const before = d.over < d.from ? items[d.over] : items[d.over + 1];
        onMove(d.key, before ? getKey(before) : null, d.from, d.over);
        setSaid(`Moved to position ${d.over + 1}`);
      }
    };
    const up = (ev: PointerEvent) => { if (ev.pointerId === pid) finish(true); };
    const cancel = (ev?: Event) => { if (!(ev instanceof PointerEvent) || ev.pointerId === pid) finish(false); };
    const key = (ev: KeyboardEvent) => { if (ev.key === "Escape") { ev.preventDefault(); ev.stopPropagation(); finish(false); } };
    addEventListener("pointermove", move);
    addEventListener("pointerup", up);
    addEventListener("pointercancel", cancel);
    addEventListener("keydown", key, true);
    addEventListener("blur", cancel);
    stop.current = () => finish(false);
  };

  const keyMove = (e: React.KeyboardEvent<HTMLElement>, index: number) => {
    if (disabled || !e.altKey || (e.key !== "ArrowUp" && e.key !== "ArrowDown")) return;
    e.preventDefault();
    const { items, getKey, onMove } = live.current;
    const up = e.key === "ArrowUp";
    const to = index + (up ? -1 : 1);
    if (to < 0 || to >= items.length) return;
    const before = up ? items[index - 1] : items[index + 2];
    onMove(getKey(items[index]), before ? getKey(before) : null, index, to);
    setSaid(`Moved to position ${to + 1}`);
    requestAnimationFrame(() => v.scrollToIndex(to, { align: "auto" }));
  };

  const slotOf = (i: number) => {
    if (!drag) return i;
    const { from, over } = drag;
    if (i === from) return over;
    if (from < over && i > from && i <= over) return i - 1;
    if (from > over && i >= over && i < from) return i + 1;
    return i;
  };

  const ctxFor = (i: number, ghost: boolean, item: T): RowCtx => ({
    index: i, dragging: !ghost && drag?.from === i, ghost, reorderable: !disabled,
    handleProps: ghost || disabled ? {} : {
      role: "button", tabIndex: 0, "aria-roledescription": "reorder handle",
      "aria-label": `${rowLabel ? rowLabel(item) : `Row ${i + 1}`}, position ${i + 1}. Alt plus arrow keys to move.`,
      onPointerDown: (e) => start(e, i),
      onKeyDown: (e) => keyMove(e, i),
      style: { touchAction: "none", userSelect: "none", WebkitUserSelect: "none", cursor: "grab" } as CSSProperties,
    },
  });

  const gi = drag ? drag.from : -1;
  return (
    <div ref={scroller} role="list" aria-label={label} className={cx(s.scroller, drag && s.active, className)}>
      <div className={s.inner} style={{ height: v.getTotalSize() }}>
        {v.getVirtualItems().map((vi) => {
          const item = items[vi.index];
          if (!item) return null;
          const isSrc = vi.index === gi;
          return (
            <div
              key={getKey(item)} role="listitem" data-index={vi.index}
              className={cx(s.row, isSrc && s.slot)}
              style={{ height: rowHeight, transform: `translateY(${slotOf(vi.index) * rowHeight}px)` }}
            >
              {renderRow(item, ctxFor(vi.index, false, item))}
            </div>
          );
        })}
      </div>
      {drag && items[drag.from] && createPortal(
        <div
          className={s.ghost} aria-hidden
          style={{ left: drag.left, width: drag.width, height: rowHeight, top: Math.max(drag.top - rowHeight / 2, Math.min(drag.bottom - rowHeight / 2, drag.y - drag.grab)) }}
        >
          {renderRow(items[drag.from], ctxFor(drag.from, true, items[drag.from]))}
        </div>,
        document.body,
      )}
      <span className={s.sr} aria-live="polite">{said}</span>
    </div>
  );
}

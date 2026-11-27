import { useEffect, useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import { createPortal } from "react-dom";
import { cx } from "./cx";
import s from "./Popover.module.css";

/** Floating panel anchored under `anchor`; closes on outside press, Esc, resize. Flips above when short of room. */
export function Popover({ anchor, open, onClose, align = "start", width, className, children, role }: {
  anchor: RefObject<HTMLElement | null>; open: boolean; onClose: () => void; align?: "start" | "end";
  width?: number | "anchor"; className?: string; children: ReactNode; role?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ top: number; left: number; minWidth?: number } | null>(null);

  useLayoutEffect(() => {
    if (!open || !anchor.current || !ref.current) return setPos(null);
    const a = anchor.current.getBoundingClientRect();
    const p = ref.current.getBoundingClientRect();
    let top = a.bottom + 6;
    if (top + p.height > innerHeight - 8 && a.top - 6 - p.height > 8) top = a.top - 6 - p.height;
    let left = align === "end" ? a.right - p.width : a.left;
    left = Math.max(8, Math.min(left, innerWidth - p.width - 8));
    setPos({ top, left, minWidth: width === "anchor" ? a.width : undefined });
  }, [open, anchor, align, width, children]);

  useEffect(() => {
    if (!open) return;
    const down = (e: PointerEvent) => {
      const t = e.target as Node;
      if (!ref.current?.contains(t) && !anchor.current?.contains(t)) onClose();
    };
    const key = (e: KeyboardEvent) => e.key === "Escape" && (e.stopPropagation(), onClose(), anchor.current?.focus());
    addEventListener("pointerdown", down, true);
    addEventListener("keydown", key, true);
    addEventListener("resize", onClose);
    return () => {
      removeEventListener("pointerdown", down, true);
      removeEventListener("keydown", key, true);
      removeEventListener("resize", onClose);
    };
  }, [open, onClose, anchor]);

  if (!open) return null;
  return createPortal(
    <div
      ref={ref} role={role} className={cx(s.pop, className)}
      style={{ top: pos?.top ?? 0, left: pos?.left ?? 0, minWidth: pos?.minWidth, width: typeof width === "number" ? width : undefined, visibility: pos ? "visible" : "hidden" }}
    >
      {children}
    </div>,
    document.body,
  );
}

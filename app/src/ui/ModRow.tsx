import type { ReactNode } from "react";
import { cx } from "./cx";
import s from "./ModRow.module.css";

/**
 * Two-line mod row: handle, load order, thumbnail, line 1 = name + status chips,
 * line 2 = meta items (one child per item), toggle at the far right.
 */
export function ModRow({ handle, order, thumb, name, chips, meta, toggle, off, className }: {
  handle?: ReactNode; order?: number; thumb: ReactNode; name: string; chips?: ReactNode; meta?: ReactNode; toggle?: ReactNode; off?: boolean; className?: string;
}) {
  return (
    <div className={cx(s.row, off && s.off, className)}>
      {handle}
      {order != null && <span className={s.order}>{String(order).padStart(3, "0")}</span>}
      {thumb}
      <div className={s.text}>
        <div className={s.line1}>
          <span className={s.name} title={name}>{name}</span>
          {chips && <span className={s.chips}>{chips}</span>}
        </div>
        {meta && <div className={s.meta}>{meta}</div>}
      </div>
      {toggle}
    </div>
  );
}

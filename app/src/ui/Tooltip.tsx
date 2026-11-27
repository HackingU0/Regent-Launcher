import type { ReactNode } from "react";
import s from "./Tooltip.module.css";

/** CSS-only tooltip: shows on hover and keyboard focus of the wrapped control. */
export function Tooltip({ text, children }: { text: string; children: ReactNode }) {
  return (
    <span className={s.wrap}>
      {children}
      <span role="tooltip" className={s.tip}>{text}</span>
    </span>
  );
}

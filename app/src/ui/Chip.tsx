import type { ReactNode } from "react";
import { cx } from "./cx";
import s from "./Chip.module.css";

export type Tone = "neutral" | "ok" | "warn" | "danger" | "accent";
/** Small status mark: a shape glyph plus words, never colour alone (e.g. "▲ made for 4.4"). */
export function Chip({ tone = "neutral", glyph, children, className, onClick, title }: {
  tone?: Tone; glyph?: "ok" | "warn" | "conflict" | string; children: ReactNode; className?: string; onClick?: () => void; title?: string;
}) {
  const g = glyph === "ok" ? "●" : glyph === "warn" ? "▲" : glyph === "conflict" ? "◆" : glyph;
  const body = (
    <>
      {g && <span className={s.glyph} aria-hidden>{g}</span>}
      {children}
    </>
  );
  return onClick
    ? <button type="button" title={title} className={cx(s.chip, s[tone], s.click, className)} onClick={onClick}>{body}</button>
    : <span title={title} className={cx(s.chip, s[tone], className)}>{body}</span>;
}

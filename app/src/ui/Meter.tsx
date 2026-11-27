import { cx } from "./cx";
import s from "./Meter.module.css";

/** Thin horizontal bar (crash suspects, sizes). `value` 0..1. */
export function Meter({ value, tone = "accent", label, className }: { value: number; tone?: "accent" | "warn" | "danger" | "ok" | "muted"; label?: string; className?: string }) {
  return (
    <span role="meter" aria-label={label} aria-valuemin={0} aria-valuemax={1} aria-valuenow={value} className={cx(s.track, className)}>
      <span className={cx(s.fill, s[tone])} style={{ width: `${Math.max(0, Math.min(1, value)) * 100}%` }} />
    </span>
  );
}

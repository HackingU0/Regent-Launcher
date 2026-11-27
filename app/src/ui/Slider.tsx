import { cx } from "./cx";
import s from "./Slider.module.css";

export function Slider({ value, min = 0, max = 100, step = 1, onChange, label, disabled, className }: {
  value: number; min?: number; max?: number; step?: number; onChange: (v: number) => void; label: string; disabled?: boolean; className?: string;
}) {
  const pct = max === min ? 0 : ((value - min) / (max - min)) * 100;
  return (
    <span className={cx(s.wrap, className)}>
      <input
        className={s.range} type="range" aria-label={label} min={min} max={max} step={step} value={value} disabled={disabled}
        style={{ "--pct": `${pct}%` } as React.CSSProperties}
        onChange={(e) => onChange(Number(e.target.value))}
      />
      <output className={s.out}>{value}</output>
    </span>
  );
}

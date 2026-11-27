import { type SettingsField } from "../../api";
import { Select, Slider, TextField, Toggle, cx } from "../../ui";
import s from "./Settings.module.css";

const norm = (f: SettingsField, v: string) =>
  f.kind === "slider" || f.kind === "int" ? String(Number(v)) : f.kind === "resolution" ? v.trim().toLowerCase() : v;
export const isChanged = (f: SettingsField, v: string) => norm(f, v) !== norm(f, f.value as string);

/** A message when the draft is not acceptable; the backend validates again on Apply. */
export function fieldInvalid(f: SettingsField, v: string): string | null {
  if (f.kind === "resolution") return /^\d{1,5}\s*x\s*\d{1,5}$/i.test(v.trim()) ? null : "Use WIDTHxHEIGHT, like 1920x1080";
  if (f.kind === "int") {
    const n = Number(v);
    const max = f.max ?? 99;
    return v.trim() !== "" && Number.isInteger(n) && n >= 0 && n <= max ? null : `A whole number from 0 to ${max}`;
  }
  return null;
}

const opts = (f: SettingsField) => f.options ?? [];

export function Control({ field: f, value, onChange, disabled, changed }: {
  field: SettingsField; value: string; onChange: (v: string) => void; disabled?: boolean; changed: boolean;
}) {
  const bad = fieldInvalid(f, value);
  let input;
  switch (f.kind) {
    case "toggle":
      input = <Toggle checked={value === "yes"} label={f.label} disabled={disabled} onChange={(on) => onChange(on ? "yes" : "no")} />;
      break;
    case "slider":
      input = (
        <Slider
          label={f.label} value={Number(value)} min={f.min} max={f.max} step={f.step} disabled={disabled}
          onChange={(v) => onChange(String(Number(v.toFixed(3))))}
        />
      );
      break;
    case "int":
      input = <TextField mono type="number" aria-label={f.label} min={0} max={f.max} value={value} disabled={disabled} className={s.short} onChange={(e) => onChange(e.target.value)} />;
      break;
    case "resolution":
      input = <TextField mono aria-label={f.label} value={value} disabled={disabled} className={s.short} placeholder="1920x1080" onChange={(e) => onChange(e.target.value)} />;
      break;
    default: {
      const o = opts(f);
      input = o.length
        ? <Select aria-label={f.label} options={o.some((x) => x.value === value) ? o : [{ value, label: value }, ...o]} value={value} disabled={disabled} onChange={onChange} />
        : <TextField mono aria-label={f.label} value={value} disabled={disabled} className={s.short} onChange={(e) => onChange(e.target.value)} />;
    }
  }
  return (
    <div className={cx(s.row, changed && s.changed)}>
      <div className={s.label}>
        <span className={s.name}>{f.label}</span>
        {f.note && <span className={s.note}><span aria-hidden>▲ </span>{f.note}</span>}
        {bad && <span className={s.bad}>{bad}</span>}
      </div>
      <div className={s.control}>{input}</div>
    </div>
  );
}

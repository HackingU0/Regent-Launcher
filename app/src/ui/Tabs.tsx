import { cx } from "./cx";
import s from "./Tabs.module.css";

export interface TabDef<T extends string> { id: T; label: string; count?: number; }
/** Segmented control used as page tabs (e.g. Mods | DLC below 1100px). */
export function Tabs<T extends string>({ tabs, value, onChange, label }: { tabs: TabDef<T>[]; value: T; onChange: (v: T) => void; label: string }) {
  return (
    <div role="tablist" aria-label={label} className={s.tabs}>
      {tabs.map((t) => (
        <button key={t.id} role="tab" type="button" aria-selected={t.id === value} className={cx(s.tab, t.id === value && s.on)} onClick={() => onChange(t.id)}>
          {t.label}
          {t.count != null && <span className={s.count}>{t.count}</span>}
        </button>
      ))}
    </div>
  );
}

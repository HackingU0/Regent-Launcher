import type { SelectHTMLAttributes } from "react";
import { ChevronDown } from "lucide-react";
import { cx } from "./cx";
import s from "./Select.module.css";

export interface Option { value: string; label: string; }
interface Props extends Omit<SelectHTMLAttributes<HTMLSelectElement>, "onChange"> {
  options: Option[];
  value: string;
  onChange: (value: string) => void;
  /** Small muted caption shown inside the control, left of the value (e.g. "Sort"). */
  prefix?: string;
}

/** Native <select> (the OS menu is accessible and correct), restyled closed. */
export function Select({ options, value, onChange, prefix, className, ...rest }: Props) {
  return (
    <span className={cx(s.wrap, rest.disabled && s.disabled, className)}>
      {prefix && <span className={s.prefix}>{prefix}</span>}
      <select className={s.select} value={value} onChange={(e) => onChange(e.target.value)} {...rest}>
        {options.map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}
      </select>
      <ChevronDown className={s.chev} />
    </span>
  );
}

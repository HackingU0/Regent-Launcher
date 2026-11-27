import type { InputHTMLAttributes, Ref } from "react";
import { Search, X } from "lucide-react";
import { cx } from "./cx";
import s from "./TextField.module.css";

type InputProps = InputHTMLAttributes<HTMLInputElement> & { ref?: Ref<HTMLInputElement> };

export function TextField({ className, mono, ...rest }: InputProps & { mono?: boolean }) {
  return <input className={cx(s.field, mono && s.mono, className)} spellCheck={false} autoComplete="off" {...rest} />;
}

export function SearchField({ value, onChange, placeholder = "Search", className, ...rest }: Omit<InputProps, "onChange" | "value"> & { value: string; onChange: (v: string) => void }) {
  return (
    <span className={cx(s.search, className)}>
      <Search className={s.icon} />
      <input className={s.bare} type="text" value={value} placeholder={placeholder} spellCheck={false} autoComplete="off" onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => e.key === "Escape" && value && (e.stopPropagation(), onChange(""))} {...rest} />
      {value && (
        <button type="button" className={s.clear} aria-label="Clear search" onClick={() => onChange("")}><X /></button>
      )}
    </span>
  );
}

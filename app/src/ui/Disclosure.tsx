import { useState, type ReactNode } from "react";
import { ChevronRight } from "lucide-react";
import { cx } from "./cx";
import s from "./Disclosure.module.css";

export function Disclosure({ title, children, defaultOpen = false, meta }: { title: string; children: ReactNode; defaultOpen?: boolean; meta?: ReactNode }) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <div className={s.wrap}>
      <button type="button" className={s.head} aria-expanded={open} onClick={() => setOpen(!open)}>
        <ChevronRight className={cx(s.chev, open && s.open)} />
        <span className={s.title}>{title}</span>
        {meta && <span className={s.meta}>{meta}</span>}
      </button>
      {open && <div className={s.body}>{children}</div>}
    </div>
  );
}

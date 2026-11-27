import { useCallback, useRef, useState, type ReactNode } from "react";
import { ChevronDown } from "lucide-react";
import { MenuPanel, type MenuItem } from "./Menu";
import { cx } from "./cx";
import s from "./SplitButton.module.css";

/** Primary action plus a chevron that opens a menu of alternatives (Launch / Launch with debug_mode ...). */
export function SplitButton({ icon, children, onClick, items, disabled, size = "lg" }: {
  icon?: ReactNode; children: ReactNode; onClick: () => void; items: MenuItem[]; disabled?: boolean; size?: "md" | "lg";
}) {
  const group = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const close = useCallback(() => setOpen(false), []);
  return (
    <div ref={group} className={cx(s.group, size === "lg" && s.lg)}>
      <button type="button" className={cx(s.main)} disabled={disabled} onClick={onClick}>{icon}{children}</button>
      <button type="button" className={s.more} disabled={disabled} aria-label="More launch options" aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen((o) => !o)}><ChevronDown /></button>
      <MenuPanel anchor={group} open={open} onClose={close} items={items} align="start" />
    </div>
  );
}

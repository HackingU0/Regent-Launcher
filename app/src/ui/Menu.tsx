import { useCallback, useRef, useState, type ReactNode, type RefObject } from "react";
import { Popover } from "./Popover";
import { cx } from "./cx";
import s from "./Menu.module.css";

export interface MenuItem {
  label: string;
  icon?: ReactNode;
  onSelect?: () => void;
  danger?: boolean;
  disabled?: boolean;
  /** Draws a hairline above this item. */
  separator?: boolean;
  hint?: string;
}

export function MenuPanel({ anchor, open, onClose, items, align }: {
  anchor: RefObject<HTMLElement | null>; open: boolean; onClose: () => void; items: MenuItem[]; align?: "start" | "end";
}) {
  const list = useRef<HTMLDivElement>(null);
  const setList = useCallback((el: HTMLDivElement | null) => {
    list.current = el;
    if (el) requestAnimationFrame(() => el.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus({ preventScroll: true }));
  }, []);
  const move = (e: React.KeyboardEvent) => {
    const btns = [...(list.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? [])];
    const i = btns.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "ArrowDown") btns[(i + 1) % btns.length]?.focus();
    else if (e.key === "ArrowUp") btns[(i - 1 + btns.length) % btns.length]?.focus();
    else return;
    e.preventDefault();
  };
  return (
    <Popover anchor={anchor} open={open} onClose={onClose} align={align} width="anchor">
      <div ref={setList} role="menu" className={s.menu} onKeyDown={move}>
        {items.map((it, i) => (
          <button
            key={i} role="menuitem" type="button" disabled={it.disabled}
            className={cx(s.item, it.danger && s.danger, it.separator && s.sep)}
            onClick={() => { onClose(); it.onSelect?.(); }}
          >
            {it.icon}
            <span className={s.label}>{it.label}</span>
            {it.hint && <span className={s.hint}>{it.hint}</span>}
          </button>
        ))}
      </div>
    </Popover>
  );
}

/** `trigger` receives props to spread on any button-like element. */
export function Menu({ trigger, items, align = "end" }: {
  trigger: (p: { ref: RefObject<HTMLButtonElement | null>; onClick: () => void; "aria-haspopup": "menu"; "aria-expanded": boolean }) => ReactNode;
  items: MenuItem[]; align?: "start" | "end";
}) {
  const ref = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const close = useCallback(() => setOpen(false), []);
  return (
    <>
      {trigger({ ref, onClick: () => setOpen((o) => !o), "aria-haspopup": "menu", "aria-expanded": open })}
      <MenuPanel anchor={ref} open={open} onClose={close} items={items} align={align} />
    </>
  );
}

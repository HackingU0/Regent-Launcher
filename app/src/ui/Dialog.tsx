import { useEffect, useRef, type ReactNode } from "react";
import { X } from "lucide-react";
import { IconButton } from "./Button";
import s from "./Dialog.module.css";

/** Native <dialog> (focus trap, Esc, inert background for free). Controlled by `open`. */
export function Dialog({ open, onClose, title, children, actions, width = 440 }: {
  open: boolean; onClose: () => void; title: string; children: ReactNode; actions?: ReactNode; width?: number;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (open && !d.open) d.showModal();
    else if (!open && d.open) d.close();
  }, [open]);
  return (
    <dialog
      ref={ref} className={s.dialog} style={{ width }} aria-label={title}
      onCancel={(e) => (e.preventDefault(), onClose())}
      onClick={(e) => e.target === ref.current && onClose()}
    >
      {open && (
        <div className={s.card}>
          <header className={s.head}>
            <h2 className={s.title}>{title}</h2>
            <IconButton label="Close" icon={<X />} onClick={onClose} />
          </header>
          <div className={s.body}>{children}</div>
          {actions && <footer className={s.foot}>{actions}</footer>}
        </div>
      )}
    </dialog>
  );
}

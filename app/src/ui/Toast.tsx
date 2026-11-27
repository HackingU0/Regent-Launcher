import { AlertTriangle, CheckCircle2, Info, OctagonAlert, X } from "lucide-react";
import { actions, useStore, type ToastKind } from "../store";
import { cx } from "./cx";
import s from "./Toast.module.css";

const ICON: Record<ToastKind, typeof Info> = { info: Info, ok: CheckCircle2, warn: AlertTriangle, danger: OctagonAlert };

/** Mount once (the Shell does). Fire with `actions.toast(text, kind)`. */
export function ToastHost() {
  const toasts = useStore((st) => st.toasts);
  return (
    <div className={s.host} aria-live="polite">
      {toasts.map((t) => {
        const Icon = ICON[t.kind];
        return (
          <div key={t.id} role="status" className={cx(s.toast, s[t.kind])}>
            <Icon className={s.icon} />
            <span className={s.text}>{t.text}</span>
            <button type="button" className={s.x} aria-label="Dismiss" onClick={() => actions.dismissToast(t.id)}><X /></button>
          </div>
        );
      })}
    </div>
  );
}

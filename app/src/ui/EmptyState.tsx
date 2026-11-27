import type { ReactNode } from "react";
import { cx } from "./cx";
import s from "./EmptyState.module.css";

/** Line icon, one sentence, one action. */
export function EmptyState({ icon, title, children, action, className }: {
  icon?: ReactNode; title?: string; children?: ReactNode; action?: ReactNode; className?: string;
}) {
  return (
    <div className={cx(s.empty, className)}>
      {icon && <div className={s.icon}>{icon}</div>}
      {title && <h3 className={s.title}>{title}</h3>}
      {children && <p className={s.text}>{children}</p>}
      {action}
    </div>
  );
}

import type { ReactNode } from "react";
import s from "./PageHeader.module.css";

/** Every page's header. Draggable window region (the `deep` value lets the buttons inside stay clickable). */
export function PageHeader({ title, count, actions, sub }: { title: string; count?: number | string; actions?: ReactNode; sub?: ReactNode }) {
  return (
    <header className={s.header} data-tauri-drag-region="deep">
      <div className={s.titles}>
        <h1 className={s.title}>{title}{count != null && <span className={s.count}>{count}</span>}</h1>
        {sub && <p className={s.sub}>{sub}</p>}
      </div>
      {actions && <div className={s.actions}>{actions}</div>}
    </header>
  );
}

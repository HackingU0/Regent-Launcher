import type { ReactNode } from "react";
import { AlertTriangle, Info, OctagonAlert } from "lucide-react";
import { cx } from "./cx";
import { Button } from "./Button";
import s from "./Banner.module.css";

export type BannerTone = "info" | "warn" | "danger";
const ICON = { info: Info, warn: AlertTriangle, danger: OctagonAlert };

/** One message, at most one action (shape + words, so it never relies on colour alone). */
export function Banner({ tone = "info", children, action, onAction, className }: {
  tone?: BannerTone; children: ReactNode; action?: string; onAction?: () => void; className?: string;
}) {
  const Icon = ICON[tone];
  return (
    <div role={tone === "danger" ? "alert" : "status"} className={cx(s.banner, s[tone], className)}>
      <Icon className={s.icon} />
      <div className={s.text}>{children}</div>
      {action && <Button className={s.action} onClick={onAction}>{action}</Button>}
    </div>
  );
}

import type { ButtonHTMLAttributes, ReactNode, Ref } from "react";
import { cx } from "./cx";
import s from "./Button.module.css";

type Kind = "primary" | "quiet" | "ghost" | "danger";
interface Props extends ButtonHTMLAttributes<HTMLButtonElement> {
  kind?: Kind;
  size?: "md" | "lg";
  icon?: ReactNode;
  ref?: Ref<HTMLButtonElement>;
}

export function Button({ kind = "quiet", size = "md", icon, className, children, type = "button", ...rest }: Props) {
  return (
    <button type={type} className={cx(s.btn, s[kind], size === "lg" && s.lg, className)} {...rest}>
      {icon}
      {children != null && <span className={s.label}>{children}</span>}
    </button>
  );
}

/** Square icon-only button; `label` is the accessible name and tooltip text. */
export function IconButton({ label, icon, kind = "ghost", className, type = "button", ...rest }: Omit<Props, "children" | "size"> & { label: string; icon: ReactNode }) {
  return (
    <button type={type} aria-label={label} title={label} className={cx(s.btn, s[kind], s.square, className)} {...rest}>
      {icon}
    </button>
  );
}

import type { HTMLAttributes } from "react";
import { cx } from "./cx";
import s from "./Panel.module.css";

/** A surface with chart-style corner ticks. Use sparingly; one level only, never nest. */
export function Panel({ className, ...rest }: HTMLAttributes<HTMLDivElement>) {
  return <section className={cx(s.panel, className)} {...rest} />;
}

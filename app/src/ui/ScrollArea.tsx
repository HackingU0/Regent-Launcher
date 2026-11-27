import type { HTMLAttributes, Ref } from "react";
import { cx } from "./cx";
import s from "./ScrollArea.module.css";

/** A scroll container with the always-visible thin thumb. Give it a height (flex:1 / min-height:0 in a flex column). */
export function ScrollArea({ className, ref, ...rest }: HTMLAttributes<HTMLDivElement> & { ref?: Ref<HTMLDivElement> }) {
  return <div ref={ref} className={cx(s.area, className)} {...rest} />;
}

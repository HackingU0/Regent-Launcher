import { cx } from "./cx";
import s from "./Skeleton.module.css";

export function Skeleton({ w, h = 12, className }: { w?: number | string; h?: number | string; className?: string }) {
  return <span className={cx(s.bar, className)} style={{ width: w, height: h }} aria-hidden />;
}

/** Placeholder list rows (never a centred spinner). */
export function SkeletonRows({ count = 8, height = 36, thumb = false }: { count?: number; height?: number; thumb?: boolean }) {
  return (
    <div role="status" aria-label="Loading" className={s.rows}>
      {Array.from({ length: count }, (_, i) => (
        <div key={i} className={s.row} style={{ height }}>
          {thumb && <Skeleton w={40} h={40} />}
          <div className={s.lines}>
            <Skeleton w={`${48 + ((i * 17) % 35)}%`} h={12} />
            {thumb && <Skeleton w={`${28 + ((i * 11) % 25)}%`} h={10} />}
          </div>
        </div>
      ))}
    </div>
  );
}

import { useMemo } from "react";

function rng(seed: string) {
  let h = 1779033703;
  for (let i = 0; i < seed.length; i++) h = Math.imul(h ^ seed.charCodeAt(i), 3432918353) << 13 | h >>> 19;
  return () => {
    h = Math.imul(h ^ (h >>> 16), 2246822507);
    h = Math.imul(h ^ (h >>> 13), 3266489909);
    return ((h ^= h >>> 16) >>> 0) / 4294967296;
  };
}

/** Unlabelled star chart, the Home backdrop when the game has no key art. ~140 stars, nearest-neighbour lanes, seeded by game id. */
export function StarChart({ seed, className }: { seed: string; className?: string }) {
  const { stars, lanes } = useMemo(() => {
    const r = rng(seed);
    const stars = Array.from({ length: 140 }, () => ({ x: 120 + r() * 880, y: 20 + r() * 560, r: 0.8 + r() * r() * 2.6, o: 0.35 + r() * 0.65 }));
    const seen = new Set<string>();
    const lanes: [number, number, number, number][] = [];
    stars.forEach((a, i) => {
      const near = stars.map((b, j) => ({ j, d: (a.x - b.x) ** 2 + (a.y - b.y) ** 2 })).filter((n) => n.j !== i).sort((p, q) => p.d - q.d).slice(0, 2);
      for (const { j } of near) {
        const k = i < j ? `${i}-${j}` : `${j}-${i}`;
        if (seen.has(k)) continue;
        seen.add(k);
        lanes.push([a.x, a.y, stars[j].x, stars[j].y]);
      }
    });
    return { stars, lanes };
  }, [seed]);
  return (
    <svg className={className} viewBox="0 0 1000 600" preserveAspectRatio="xMaxYMid slice" aria-hidden>
      <g stroke="var(--lane)" strokeOpacity="0.12" strokeWidth="1">
        {lanes.map((l, i) => <line key={i} x1={l[0]} y1={l[1]} x2={l[2]} y2={l[3]} />)}
      </g>
      {stars.map((st, i) => <circle key={i} cx={st.x} cy={st.y} r={st.r} fill="var(--text)" fillOpacity={st.o * 0.7} />)}
    </svg>
  );
}

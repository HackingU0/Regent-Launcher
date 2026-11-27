import { useState } from "react";
import { cx } from "./cx";
import s from "./Thumb.module.css";

function hue(id: string) {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = Math.imul(h ^ id.charCodeAt(i), 2654435761) >>> 0;
  return (h >>> 7) % 360;
}
function initials(name: string) {
  const w = name.replace(/[^\p{L}\p{N}\s]/gu, " ").trim().split(/\s+/).filter(Boolean);
  if (!w.length) return "?";
  const cjk = /[㐀-鿿]/.test(w[0][0]);
  return cjk ? w[0].slice(0, 2) : (w[0][0] + (w[1]?.[0] ?? w[0][1] ?? "")).toUpperCase();
}

/** The image, or a monogram tile (initials on a hue hashed from the id). Never an empty box. */
export function Thumb({ src, name, id, w = 40, h = w, className }: { src?: string | null; name: string; id: string; w?: number; h?: number; className?: string }) {
  const [bad, setBad] = useState(false);
  const mono = !src || bad;
  const hh = hue(id);
  return (
    <span className={cx(s.thumb, mono && s.mono, className)} style={{ width: w, height: h, ...(mono ? { "--h": hh } as React.CSSProperties : null) }}>
      {mono ? <span className={s.init} style={{ fontSize: Math.max(10, Math.min(h, w) * 0.34) }}>{initials(name)}</span>
        : <img src={src} alt="" loading="lazy" decoding="async" onError={() => setBad(true)} />}
    </span>
  );
}

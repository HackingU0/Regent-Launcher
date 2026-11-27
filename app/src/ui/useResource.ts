import { useCallback, useEffect, useRef, useState } from "react";

export interface Resource<T> { data: T | null; error: string | null; loading: boolean; reload: () => void; set: (v: T) => void; }

/** Load async data on mount / when `deps` change. Shows `loading` only until the first result (keeps stale data on reload). */
export function useResource<T>(load: () => Promise<T>, deps: unknown[]): Resource<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [n, setN] = useState(0);
  const seq = useRef(0);
  const fn = useRef(load);
  fn.current = load;
  useEffect(() => {
    const id = ++seq.current;
    setError(null);
    fn.current().then(
      (d) => seq.current === id && (setData(d), setLoading(false)),
      (e) => seq.current === id && (setError(e instanceof Error ? e.message : String(e)), setLoading(false)),
    );
  }, [...deps, n]);
  const reload = useCallback(() => setN((x) => x + 1), []);
  return { data, error, loading, reload, set: setData };
}

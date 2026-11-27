import { useState } from "react";
import { FolderOpen, ScrollText, ShieldCheck } from "lucide-react";
import { api, type CrashReport } from "../../api";
import { actions, useStore } from "../../store";
import { Banner, Button, Chip, Disclosure, EmptyState, Meter, PageHeader, ScrollArea, SkeletonRows, cx, useResource } from "../../ui";
import s from "./crash.module.css";

const LIVE = "";
const n = (x: number) => x.toLocaleString("en-US");
/** crash dir names look like stellaris_20261125_140210. */
function crashLabel(name: string) {
  const m = /(\d{4})(\d{2})(\d{2})_(\d{2})(\d{2})(\d{2})/.exec(name);
  return m ? { title: `${m[1]}-${m[2]}-${m[3]}`, sub: `${m[4]}:${m[5]}:${m[6]}` } : { title: name, sub: "" };
}

function Report({ r }: { r: CrashReport }) {
  const top = r.ranked[0];
  const suspect = top && top.only > 0 ? top : null;
  const max = Math.max(1, ...r.ranked.map((x) => x.only + x.shared));
  const b = r.buckets;
  const bucketTotal = Math.max(1, b.only + b.shared + b.vanilla + b.none + b.inactive);
  const buckets: [string, number, "accent" | "warn" | "ok" | "muted", string][] = [
    ["Only one active mod ships the file", b.only, "accent", "Strongest evidence"],
    ["Several mods ship the file", b.shared, "warn", "Cannot tell which one"],
    ["Vanilla file", b.vanilla, "ok", r.missing.length ? "Or overridden by a missing mod" : ""],
    ["No installed mod ships it", b.none, "muted", ""],
    ...(b.inactive > 0 ? [["Only inactive mods ship it", b.inactive, "muted", "Not active at the crash"] as [string, number, "muted", string]] : []),
  ];
  const meta = [r.appVersion && `game ${r.appVersion}`, r.signal, r.checksum && `checksum ${r.checksum}`].filter(Boolean).join(" · ");
  return (
    <div className={s.report}>
      <section className={s.hero}>
        <div className={s.kicker}>{r.live ? "Live error.log" : "Crash report"}{meta && <span className={s.meta}> · {meta}</span>}</div>
        {suspect ? (
          <>
            <h2 className={s.suspect}>{suspect.name}</h2>
            <p className={s.why}>
              Most likely suspect: <b>{n(suspect.only)}</b> of {n(r.total)} log entries name files that only this mod ships, {n(suspect.onlyRecent)} of them in the last 10 seconds before the log ended.
            </p>
          </>
        ) : (
          <>
            <h2 className={s.suspect}>No single mod stands out</h2>
            <p className={s.why}>No log entry names a file that only one active mod ships, so the log cannot point at one mod. The ranking below shows who touched the most files.</p>
          </>
        )}
        <p className={s.stat}>{n(r.total)} entries in error.log, {n(r.named)} name a file path.</p>
      </section>

      <Banner tone="info">This is correlation, not proof. A mod named in the log may only be where the engine noticed a problem that started elsewhere.</Banner>

      {r.ranked.length > 0 && (
        <section className={s.sec}>
          <h3 className={s.h}>Suspects <span className={s.hint}>ranked by recent entries, then total</span></h3>
          <ol className={s.ranks}>
            {r.ranked.slice(0, 10).map((x, i) => (
              <li key={x.id} className={s.rank}>
                <span className={s.ord}>{String(i + 1).padStart(2, "0")}</span>
                <span className={s.rname} title={x.name}>{x.name}</span>
                <span className={s.bars}>
                  <Meter value={x.only / max} tone="accent" label={`${x.name}: ${x.only} entries only this mod ships`} />
                  <Meter value={x.shared / max} tone="warn" label={`${x.name}: ${x.shared} shared entries`} />
                </span>
                <span className={s.counts}>
                  <span>only it <b>{n(x.only)}</b> <i>({n(x.onlyRecent)} recent)</i></span>
                  <span>shared <b>{n(x.shared)}</b> <i>({n(x.sharedRecent)} recent)</i></span>
                </span>
              </li>
            ))}
          </ol>
        </section>
      )}

      <section className={s.sec}>
        <h3 className={s.h}>Entries by file <span className={s.hint}>one entry can appear in several rows</span></h3>
        <ul className={s.buckets}>
          {buckets.map(([label, v, tone, note]) => (
            <li key={label} className={s.bucket}>
              <span className={s.blabel}>{label}{note && <span className={s.hint}> · {note}</span>}</span>
              <Meter value={v / bucketTotal} tone={tone} label={label} className={s.bmeter} />
              <span className={s.bnum}>{n(v)}</span>
            </li>
          ))}
        </ul>
      </section>

      {r.missing.length > 0 && (
        <section className={s.sec}>
          <h3 className={s.h}>Missing mods <span className={s.hint}>in the load order but not installed</span></h3>
          <div className={s.chips}>{r.missing.map((m) => <Chip key={m} glyph="warn" tone="warn">{m}</Chip>)}</div>
        </section>
      )}

      <Disclosure title="Raw log lines" meta={n(r.lines.length)}>
        <pre className={cx(s.raw, "selectable")}>{r.lines.join("\n")}</pre>
      </Disclosure>
    </div>
  );
}

export default function CrashPage() {
  const game = useStore((x) => x.game);
  return game ? <CrashInner key={game} game={game} /> : null;
}

function CrashInner({ game }: { game: string }) {
  const list = useResource(() => api.crashList(game), [game]);
  // sel: LIVE ("") or a crash dir name; null = not chosen yet
  const [sel, setSel] = useState<string | null>(null);
  const crashes = list.data?.crashes ?? [];
  const hasLive = list.data?.hasLive ?? false;
  const cur = sel ?? (crashes[0] ?? (hasLive ? LIVE : null));
  const rep = useResource(
    () => (cur === null ? Promise.resolve(null) : api.crashDiagnose(game, cur === LIVE ? undefined : cur).then((report) => ({ cur, report }))),
    [game, cur],
  );

  const open = () => void actions.run(() => api.open({ kind: "crashDir", game, name: cur ?? LIVE }));

  return (
    <>
      <PageHeader
        title="Crash reports"
        count={list.data ? crashes.length : undefined}
        actions={<Button icon={<FolderOpen size={15} />} disabled={cur === null} onClick={open}>Open folder</Button>}
      />
      <div className={s.split}>
        <aside className={s.left} aria-label="Crash list">
          <ScrollArea className={s.leftScroll}>
            {list.loading ? <SkeletonRows count={6} height={52} /> : list.error ? (
              <Banner tone="danger" action="Retry" onAction={list.reload}>{list.error}</Banner>
            ) : (
              <ul className={s.items}>
                {hasLive && (
                  <li><button type="button" className={cx(s.item, cur === LIVE && s.on)} aria-current={cur === LIVE} onClick={() => setSel(LIVE)}>
                    <ScrollText className={s.ico} /><span className={s.it}>Live log<span className={s.is}>error.log of the last run</span></span>
                  </button></li>
                )}
                {crashes.map((c) => {
                  const l = crashLabel(c);
                  return (
                    <li key={c}><button type="button" className={cx(s.item, cur === c && s.on)} aria-current={cur === c} onClick={() => setSel(c)}>
                      <span className={s.dot} aria-hidden>◆</span><span className={s.it}><span className={s.mono}>{l.title} {l.sub}</span><span className={s.is}>{c}</span></span>
                    </button></li>
                  );
                })}
              </ul>
            )}
          </ScrollArea>
        </aside>
        <ScrollArea className={s.right}>
          {list.loading ? <SkeletonRows count={5} height={56} /> : cur === null ? (
            <EmptyState icon={<ShieldCheck />} title="No crashes found">Nothing to diagnose. If the game closes unexpectedly, its crash report appears here.</EmptyState>
          ) : rep.error ? (
            <Banner tone="danger" action="Retry" onAction={rep.reload}>{rep.error}</Banner>
          ) : rep.loading || rep.data?.cur !== cur ? (
            <SkeletonRows count={5} height={56} />
          ) : (
            <Report r={rep.data.report} />
          )}
        </ScrollArea>
      </div>
    </>
  );
}

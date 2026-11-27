import { useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ChevronRight, ShieldCheck } from "lucide-react";
import { api, type Summary } from "../../api";
import { actions, modsById, useStore } from "../../store";
import { Banner, Chip, EmptyState, PageHeader, ScrollArea, SkeletonRows, cx, useResource } from "../../ui";
import s from "./conflicts.module.css";

type ConflictPairSummary = Summary["pairs"][number];
const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;
const VIRTUAL_PAST = 200;
const PATH_H = 22;

function Paths({ game, pair }: { game: string; pair: ConflictPairSummary }) {
  const r = useResource(() => api.conflictFiles(game, pair.winner, pair.loser), [game, pair.winner, pair.loser]);
  const ref = useRef<HTMLDivElement>(null);
  const files = r.data ?? [];
  const virt = useVirtualizer({ count: files.length > VIRTUAL_PAST ? files.length : 0, getScrollElement: () => ref.current, estimateSize: () => PATH_H, overscan: 12 });
  if (r.loading) return <div className={s.paths}><SkeletonRows count={3} height={PATH_H} /></div>;
  if (r.error) return <div className={s.paths}><Banner tone="danger" action="Retry" onAction={r.reload}>{r.error}</Banner></div>;
  if (files.length <= VIRTUAL_PAST) {
    return <ul className={cx(s.paths, s.list, "selectable")}>{files.map((f) => <li key={f} className={s.path}>{f}</li>)}</ul>;
  }
  return (
    <div ref={ref} className={cx(s.paths, s.vlist, "selectable")}>
      <div style={{ height: virt.getTotalSize(), position: "relative" }}>
        {virt.getVirtualItems().map((v) => (
          <div key={v.key} className={s.path} style={{ position: "absolute", top: 0, left: 0, right: 0, height: PATH_H, transform: `translateY(${v.start}px)` }}>{files[v.index]}</div>
        ))}
      </div>
    </div>
  );
}

function PairRow({ game, pair, names }: { game: string; pair: ConflictPairSummary; names: Map<string, { name: string }> }) {
  const [open, setOpen] = useState(false);
  const nm = (id: string) => names.get(id)?.name ?? id;
  return (
    <li className={s.pair}>
      <button type="button" className={s.head} aria-expanded={open} onClick={() => setOpen(!open)}>
        <ChevronRight className={cx(s.chev, open && s.open)} />
        <span className={s.names}>
          <span className={s.win}>{nm(pair.winner)}</span>
          <span className={s.verb}>overrides</span>
          <span className={s.lose}>{nm(pair.loser)}</span>
        </span>
        <Chip glyph="conflict" tone="warn">{plural(pair.files, "file")}</Chip>
      </button>
      {open && <Paths game={game} pair={pair} />}
    </li>
  );
}

export default function ConflictsPage() {
  const game = useStore((x) => x.game);
  const view = useStore((x) => x.view);
  const summary = useStore((x) => x.summary);
  const readOnly = view?.readOnly ?? false;
  const names = useMemo(() => modsById(view), [view]);
  const [applying, setApplying] = useState(false);

  if (!game || !summary) {
    return (<><PageHeader title="Conflicts" /><div className={s.body}><SkeletonRows count={6} height={48} /></div></>);
  }
  const { pairs, suggest } = summary;
  const involved = new Set(pairs.flatMap((p) => [p.winner, p.loser])).size;
  const total = pairs.reduce((n, p) => n + p.files, 0);
  const apply = async () => {
    setApplying(true);
    const ok = await actions.applyOrder();
    setApplying(false);
    if (ok) actions.toast("Suggested order applied", "ok");
  };

  return (
    <>
      <PageHeader
        title="Conflicts"
        count={pairs.length || undefined}
        sub={pairs.length ? `${plural(pairs.length, "conflict")} across ${plural(involved, "mod")}, ${plural(total, "file")} overridden in total.` : undefined}
      />
      <ScrollArea className={s.body}>
        {suggest.changed && (
          <Banner tone="info" className={s.gap} action={readOnly ? undefined : applying ? "Applying…" : "Apply order"} onAction={applying ? undefined : apply}>
            <strong className={s.bt}>A better load order is available.</strong>
            <ul className={s.reasons}>{suggest.reasons.map((r) => <li key={r}>{r}</li>)}</ul>
            <span className={s.more}>Your current order is kept until you apply.</span>
          </Banner>
        )}
        {pairs.length === 0 ? (
          <EmptyState icon={<ShieldCheck />} title="No conflicts">No two enabled mods change the same file. Nothing to resolve.</EmptyState>
        ) : (
          <ul className={s.pairs} aria-label="Conflicting mod pairs">
            {pairs.map((p) => <PairRow key={`${p.winner}|${p.loser}`} game={game} pair={p} names={names} />)}
          </ul>
        )}
      </ScrollArea>
    </>
  );
}

import { useMemo, useState } from "react";
import { Link2, MoreHorizontal, Play, RefreshCw, Save } from "lucide-react";
import { api, formatDate, type At, type Binding, type SaveRow } from "../../api";
import { confirmExpect } from "../../queue";
import { actions, activeAt, activePlayset, useStore } from "../../store";
import { Banner, Button, Chip, Dialog, EmptyState, IconButton, Menu, PageHeader, ScrollArea, SkeletonRows, useResource } from "../../ui";
import s from "./Saves.module.css";

/** The dialog is bound to the save and playset it was raised for. */
interface Confirm { row: SaveRow; at: At; reason: string; missing: string[]; token: string }

function BindingChip({ b, active }: { b: Binding; active?: string }) {
  if (b.kind === "playset") return <Chip tone={b.name === active ? "accent" : "neutral"} glyph="●" title={`Saved with playset "${b.name}"`}>{b.name}</Chip>;
  if (b.kind === "custom") return <Chip glyph="◆" title="Saved with a mod list that matches no playset">custom ({b.count} {b.count === 1 ? "mod" : "mods"})</Chip>;
  return <Chip glyph="?" title="Regent has no record of the mods this save used">unknown</Chip>;
}

export default function SavesPage() {
  const game = useStore((st) => st.game);
  const view = useStore((st) => st.view);
  const at = activeAt(view);
  const active = activePlayset(view)?.name;
  const locked = (view?.readOnly ?? false) || (view?.running ?? 0) > 0;
  const res = useResource(() => api.savesList(game ?? ""), [game]);
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const launching = useStore((st) => st.launching);

  const rows = useMemo(() => [...(res.data?.rows ?? [])].sort((a, b) => b.mtime - a.mtime), [res.data]);

  /** Goes through the store's launch gate, so it cannot overlap the Launch bar or Home. A confirmed retry keeps the dialog's playset. */
  const play = async (row: SaveRow, force: boolean, on?: At, expect?: string) => {
    if (!game || !at || busy != null || launching) return;
    setBusy(row.rel);
    const r = await actions.guardedLaunch((g, cur) => {
      const target = on ?? cur;
      return api.savesPlay(g, target, row.rel, force, expect).then((x) => ({ x, target }));
    });
    setBusy(null);
    if (!r) {
      // Stale, failed or refused: the dialog (if any) no longer describes the save. Close it and re-read.
      setConfirm(null);
      res.reload();
      return;
    }
    if (r.x.kind === "needsConfirm") return setConfirm({ row, at: r.target, reason: r.x.reason, missing: r.x.missing, token: r.x.token });
    const x = r.x;
    setConfirm(null);
    setConfirm(null);
    if (x.warning) actions.toast(x.warning, "warn");
    actions.toast(`Launching with playset "${x.using}".`, "ok", 3000);
    if (x.manualLoad) actions.toast(`In the game, choose "${row.title}" in Load Game.`, "info", 9000);
    void actions.refresh(false);
    res.reload();
  };
  const bind = async (row: SaveRow) => {
    if (!game || !at) return;
    const r = await actions.run(() => api.savesBind(game, at, row.rel));
    if (!r) return;
    res.set(r);
    actions.toast(`"${row.title}" is now bound to "${at.name}".`, "ok");
  };

  const hasRows = rows.length > 0;
  return (
    <>
      <PageHeader
        title="Saves" count={rows.length || undefined}
        sub="Play a save with the playset it was made with."
        actions={<IconButton label="Refresh saves" icon={<RefreshCw size={16} />} onClick={res.reload} />}
      />
      {res.loading ? (
        <div className={s.pad}><SkeletonRows count={9} height={44} /></div>
      ) : res.error || !res.data ? (
        <div className={s.pad}><Banner tone="danger" action="Retry" onAction={res.reload}>{res.error ?? "Saves could not be loaded."}</Banner></div>
      ) : (
        <>
          {res.data.warning && <Banner tone="warn" className={s.banner}>{res.data.warning}</Banner>}
          {(view?.running ?? 0) > 0 && <Banner tone="info" className={s.banner}>The game is running. Play is off until it exits.</Banner>}
          {!hasRows ? (
            <EmptyState icon={<Save />}>No saves yet. Launch once and they appear here.</EmptyState>
          ) : (
            <ScrollArea className={s.scroll}>
              <div className={s.table} role="table" aria-label="Saves">
                <div className={s.head} role="row">
                  <span role="columnheader">Save</span>
                  <span role="columnheader">In-game date</span>
                  <span role="columnheader">Version</span>
                  <span role="columnheader">Modified</span>
                  <span role="columnheader">Playset</span>
                  <span role="columnheader" />
                </div>
                {rows.map((r) => (
                  <div key={r.rel} className={s.row} role="row">
                    <span className={s.title} role="cell" title={r.rel}>{r.title}</span>
                    <span className={s.mono} role="cell">{r.date || "-"}</span>
                    <span className={s.mono} role="cell">{r.version || "-"}</span>
                    <span className={s.mono} role="cell">{formatDate(r.mtime)}</span>
                    <span role="cell"><BindingChip b={r.binding} active={active} /></span>
                    <span className={s.actions} role="cell">
                      <Button icon={<Play size={13} />} disabled={locked || !at || busy != null || launching} onClick={() => play(r, false)}>
                        {busy === r.rel ? "Starting…" : "Play"}
                      </Button>
                      <Menu
                        trigger={(p) => <IconButton label={`More for ${r.title}`} icon={<MoreHorizontal size={16} />} {...p} />}
                        items={[{
                          label: at ? `Bind to "${at.name}"` : "Bind to active playset",
                          icon: <Link2 size={14} />, disabled: view?.readOnly || !at, onSelect: () => void bind(r),
                        }]}
                      />
                    </span>
                  </div>
                ))}
              </div>
            </ScrollArea>
          )}
        </>
      )}
      <Dialog
        open={confirm != null} onClose={() => setConfirm(null)} title="Play without all of its mods?" width={480}
        actions={<><Button onClick={() => setConfirm(null)}>Cancel</Button><Button kind="primary" disabled={busy != null || launching} onClick={() => confirm && void play(confirm.row, true, confirm.at, confirmExpect(confirm))}>{busy != null ? "Starting…" : "Play anyway"}</Button></>}
      >
        <p>{confirm?.reason}</p>
        {confirm && confirm.missing.length > 0 && (
          <>
            <p className={s.dim}>{confirm.missing.length === 1 ? "1 mod is" : `${confirm.missing.length} mods are`} not installed:</p>
            <ul className={s.missing}>{confirm.missing.map((m) => <li key={m}>{m}</li>)}</ul>
          </>
        )}
      </Dialog>
    </>
  );
}

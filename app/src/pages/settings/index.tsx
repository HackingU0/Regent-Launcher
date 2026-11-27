import { useEffect, useMemo, useRef, useState } from "react";
import { FolderOpen, RotateCcw } from "lucide-react";
import { AppError, api, formatDate, type SettingsField, type SettingsView } from "../../api";
import { actions, useStore } from "../../store";
import { Banner, Button, Dialog, Disclosure, EmptyState, PageHeader, Panel, ScrollArea, SkeletonRows, useResource } from "../../ui";
import { Control, fieldInvalid, isChanged } from "./Control";
import s from "./Settings.module.css";

const GROUPS: { title: string; ids: string[] }[] = [
  { title: "Display", ids: ["resolution", "display-mode", "vsync", "ui-scale"] },
  { title: "Audio", ids: ["master-volume", "music-volume", "sound-fx-volume", "ambient-volume", "voice-volume"] },
  { title: "Game", ids: ["language", "autosave"] },
];
const RAW_FILES = ["game", "pdx"] as const;
type RawKey = (typeof RAW_FILES)[number];
const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;
const when = (ms: number) => `${formatDate(ms)} ${new Date(ms).toTimeString().slice(0, 5)}`;

export default function SettingsPage() {
  const game = useStore((st) => st.game);
  const title = useStore((st) => st.games.find((g) => g.id === st.game)?.title ?? "the game");
  const readOnly = useStore((st) => st.view?.readOnly ?? false);
  const running = useStore((st) => st.view?.running ?? 0);
  const res = useResource(() => api.settingsGet(game ?? ""), [game]);
  const view = res.data;

  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [raw, setRaw] = useState<Partial<Record<RawKey, string>>>({});
  const [stale, setStale] = useState(false);
  const [busy, setBusy] = useState(false);
  const [restore, setRestore] = useState<number | null>(null);

  const fields = useMemo(() => new Map((view?.fields ?? []).map((f) => [f.id, f])), [view]);
  const changedIds = [...Object.keys(drafts)].filter((id) => fields.has(id) && isChanged(fields.get(id)!, drafts[id]));
  const changedRaw = RAW_FILES.filter((k) => raw[k] != null && raw[k] !== (view?.raw[k] ?? ""));
  const count = changedIds.length + changedRaw.length;
  const invalid = changedIds.some((id) => fieldInvalid(fields.get(id)!, drafts[id]));

  // A focus refresh that finds the files changed on disk: adopt silently when clean, else show the stale banner.
  // Every read/write bumps `ver`; a read that finishes under an older `ver` is dropped, so it never replaces a newer view.
  const ver = useRef(0);
  const live = useRef({ view, count });
  live.current = { view, count };
  const focusTick = useStore((st) => st.focusTick);
  useEffect(() => { ver.current++; }, [game]);
  useEffect(() => {
    if (focusTick === 0 || !game) return;
    const v0 = ++ver.current;
    api.settingsGet(game).then((v) => {
      if (v0 !== ver.current || v.rev === live.current.view?.rev) return;
      if (live.current.count === 0) res.set(v);
      else setStale(true);
    }, () => {});
  }, [focusTick]);

  const discard = () => { setDrafts({}); setRaw({}); };
  const reload = () => { ver.current++; discard(); setStale(false); res.reload(); };
  const adopt = (v: SettingsView) => { ver.current++; res.set(v); discard(); setStale(false); };

  const fail = (e: unknown) => {
    if (e instanceof AppError && e.kind === "stale") setStale(true);
    else actions.toast(e instanceof Error ? e.message : String(e), "danger");
  };
  const apply = async () => {
    if (!game || !view) return;
    setBusy(true);
    ver.current++;
    try {
      const r = await api.settingsApply(
        game, view.rev,
        changedIds.map((id) => ({ field: id, value: drafts[id] })),
        changedRaw.map((file) => ({ file, text: raw[file] ?? "" })),
      );
      adopt(r.view);
      actions.toast(r.backupDir ? "Settings applied. The previous files are backed up." : "Settings applied.", "ok");
    } catch (e) { fail(e); } finally { setBusy(false); }
  };
  const doRestore = async () => {
    if (!game || restore == null) return;
    const id = restore;
    setRestore(null);
    setBusy(true);
    ver.current++;
    try {
      adopt(await api.settingsRollback(game, id));
      actions.toast("Settings restored from backup.", "ok");
    } catch (e) { fail(e); } finally { setBusy(false); }
  };

  const showBackups = () => game && void actions.run(() => api.open({ kind: "backups", game }));
  const locked = readOnly || busy;

  return (
    <>
      <PageHeader title="Game settings" sub={view ? `Edits ${title}'s own settings files. One Apply makes one backup.` : undefined} />
      {res.loading ? (
        <div className={s.pad}><SkeletonRows count={9} height={48} /></div>
      ) : res.error || !view ? (
        <div className={s.pad}>
          <Banner tone="danger" action="Retry" onAction={res.reload}>{res.error ?? "Settings could not be loaded."}</Banner>
        </div>
      ) : (
        <>
          <ScrollArea className={s.scroll}>
            <div className={s.col}>
              {readOnly && <Banner tone="warn">Regent is read-only right now. Settings can be viewed but not changed.</Banner>}
              {running > 0 && (
                <Banner tone="warn">{title} is running. It can overwrite these files when it exits, so changes made now may be lost.</Banner>
              )}
              {stale && (
                <Banner tone="warn" action="Reload" onAction={reload}>
                  The settings files changed on disk since this page loaded. Reload to see them; unapplied edits will be discarded.
                </Banner>
              )}
              {view.fields.length === 0 && (
                <EmptyState title="No settings found">The game has not written its settings files yet. Launch it once, then come back.</EmptyState>
              )}
              {GROUPS.map((g) => {
                const list = g.ids.map((id) => fields.get(id)).filter((f): f is SettingsField => !!f);
                if (!list.length) return null;
                return (
                  <section key={g.title} className={s.group} aria-label={g.title}>
                    <h2 className={s.groupTitle}>{g.title}</h2>
                    <Panel className={s.rows}>
                      {list.map((f) => (
                        <Control
                          key={f.id} field={f} disabled={locked}
                          value={drafts[f.id] ?? f.value}
                          changed={changedIds.includes(f.id)}
                          onChange={(v) => setDrafts((d) => ({ ...d, [f.id]: v }))}
                        />
                      ))}
                    </Panel>
                  </section>
                );
              })}

              <section className={s.group} aria-label="Backups">
                <div className={s.groupHead}>
                  <h2 className={s.groupTitle}>Backups</h2>
                  <Button icon={<FolderOpen size={14} />} onClick={showBackups}>Show backups</Button>
                </div>
                <Panel className={s.rows}>
                  {view.backups.length === 0 ? (
                    <p className={s.none}>No backups yet. The first Apply saves the current files here.</p>
                  ) : (
                    view.backups.map((b) => (
                      <div key={b.id} className={s.backup}>
                        <span className={s.when}>{when(b.at)}</span>
                        <span className={s.files}>{b.files.join(", ")}</span>
                        <Button icon={<RotateCcw size={14} />} disabled={locked} onClick={() => setRestore(b.id)}>Restore</Button>
                      </div>
                    ))
                  )}
                </Panel>
              </section>

              <section className={s.group} aria-label="Advanced">
                <Disclosure title="Advanced" meta="raw files">
                  <p className={s.advNote}>Edit the settings files as text. Raw edits are applied first, then the form values above.</p>
                  {RAW_FILES.map((k) => {
                    const orig = view.raw[k];
                    if (orig == null) return null;
                    return (
                      <label key={k} className={s.rawBlock}>
                        <span className={s.rawHead}>
                          <span className={s.rawName}>{k === "game" ? "Game settings" : "Launcher settings (pdx)"}</span>
                          <span className={s.path}>{view.paths[k]}</span>
                        </span>
                        <textarea
                          className={s.raw} spellCheck={false} rows={14} disabled={locked}
                          value={raw[k] ?? orig}
                          onChange={(e) => setRaw((r) => ({ ...r, [k]: e.target.value }))}
                        />
                      </label>
                    );
                  })}
                </Disclosure>
              </section>
            </div>
          </ScrollArea>

          {count > 0 && (
            <div className={s.bar} role="region" aria-label="Unapplied changes">
              <span className={s.barText}>{plural(count, "unapplied change")}</span>
              {invalid && <span className={s.barWarn}>Fix the highlighted fields to apply.</span>}
              <span className={s.spacer} />
              <Button onClick={discard} disabled={busy}>Discard</Button>
              <Button kind="primary" onClick={apply} disabled={locked || invalid}>{busy ? "Applying…" : "Apply"}</Button>
            </div>
          )}
        </>
      )}
      <Dialog
        open={restore != null} onClose={() => setRestore(null)} title="Restore this backup?" width={440}
        actions={<><Button onClick={() => setRestore(null)}>Cancel</Button><Button kind="primary" onClick={doRestore}>Restore</Button></>}
      >
        <p>The settings files go back to how they were on {view?.backups.find((b) => b.id === restore) ? when(view.backups.find((b) => b.id === restore)!.at) : "that day"}. Your current files are backed up first.</p>
        {count > 0 && <p className={s.dim}>Your {plural(count, "unapplied change")} will be discarded.</p>}
      </Dialog>
    </>
  );
}

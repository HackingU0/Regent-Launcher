import { useEffect, useRef, useState } from "react";
import { Bug, FolderOpen, ImageIcon, Play, SlidersHorizontal, TriangleAlert } from "lucide-react";
import { artUrl } from "../../api";
import { actions, activeAt, checksumUnknownNote, mismatchCount, useStore, visibleEntries } from "../../store";
import { Button, Dialog, Select, SplitButton, TextField, Tooltip, cx } from "../../ui";
import { StarChart } from "./StarChart";
import s from "./Home.module.css";

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

export default function HomePage() {
  const view = useStore((st) => st.view);
  const info = useStore((st) => st.games.find((g) => g.id === st.game));
  const summary = useStore((st) => st.summary);
  const launching = useStore((st) => st.launching);
  const artTick = useStore((st) => st.artTick);
  const [artBad, setArtBad] = useState(false);
  const [logoBad, setLogoBad] = useState(false);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [wallOpen, setWallOpen] = useState(false);
  const game = view?.game;
  useEffect(() => { setArtBad(false); setLogoBad(false); }, [game, artTick]);
  if (!view || !game) return null;

  const playset = view.state.playsets[view.state.active];
  const entries = visibleEntries(view);
  const modsOn = entries.filter((e) => e.enabled).length;
  const off = new Set(playset?.disabledDlcs ?? []);
  const dlcOn = view.dlcs.filter((d) => !off.has(d.id)).length;
  const mism = mismatchCount(view);
  const running = view.running > 0;
  const ach = summary?.checksum.achievements;
  const title = info?.title ?? game;
  const hasArt = view.art.wallpaper && !artBad;
  const launchDisabled = running || launching || view.readOnly || !activeAt(view);
  const go = actions.navigate;

  const playsetOptions = view.state.playsets.map((p, i) => ({ value: String(i), label: p.name }));
  const achText = ach === true ? "Achievements on" : ach === false ? "Achievements off" : summary ? "Achievements unknown" : "Checking achievements";
  const achTitle = ach === false
    ? `${plural(summary?.checksum.changing.length ?? 0, "item")} ${summary?.checksum.changing.length === 1 ? "changes" : "change"} the checksum`
    : ach === true ? "Checksum unchanged" : summary ? checksumUnknownNote(summary.checksum.unknownReason) : undefined;

  return (
    <div className={s.home}>
      <div className={s.drag} data-tauri-drag-region="deep" />
      <div className={s.launch}>
        {view.art.logo && !logoBad
          ? <img className={s.logo} src={artUrl(game, "logo", artTick)} alt={title} onError={() => setLogoBad(true)} />
          : <h1 className={s.wordmark}>{title}</h1>}
        <div className={s.launchRow}>
          <SplitButton
            icon={running ? undefined : <Play />} disabled={launchDisabled} onClick={() => void actions.launch()}
            items={[
              { label: "Launch with -debug_mode", icon: <Bug />, onSelect: () => void actions.launch(["-debug_mode"]) },
              { label: "Edit launch arguments…", icon: <SlidersHorizontal />, separator: true, onSelect: () => { setDraft(view.state.launchArgs); setEditing(true); } },
            ]}
          >
            {running ? <><span className={s.runDot} aria-hidden />Running</> : launching ? "Starting…" : "Play"}
          </SplitButton>
          {mism > 0 && (
            <Tooltip text={`${plural(mism, "enabled mod")} made for another game version`}>
              <button type="button" className={s.issue} aria-label={`${plural(mism, "enabled mod")} made for another game version. Review in Mods.`} onClick={() => go("mods", { filter: "attention" })}>
                <TriangleAlert />
              </button>
            </Tooltip>
          )}
        </div>
        <div className={s.playsetLine}>
          <label htmlFor="home-playset">Playset</label>
          {playset
            ? <Select id="home-playset" className={s.playsetSelect} value={String(view.state.active)} disabled={view.readOnly} options={playsetOptions} onChange={(v) => void actions.playsetUse(Number(v))} />
            : <span>None yet</span>}
        </div>
        <div className={s.status}>
          <button type="button" onClick={() => go("mods")}>{modsOn} of {plural(entries.length, "mod")} on</button>
          <button type="button" onClick={() => go("mods", { tab: "dlc" })}>DLC {dlcOn} of {view.dlcs.length}</button>
          <span className={cx(ach === true && s.ok, ach === false && s.warn)} title={achTitle}>{achText}</span>
          {view.crashCount > 0 && <button type="button" onClick={() => go("crash")}>{plural(view.crashCount, "crash report")}</button>}
          {view.state.launchArgs && <span className={s.args} title="Launch arguments">{view.state.launchArgs}</span>}
        </div>
      </div>

      <div className={s.art}>
        {hasArt ? <img src={artUrl(game, "wallpaper", artTick)} alt="" onError={() => setArtBad(true)} /> : <StarChart className={s.chart} seed={game} />}
        <div className={s.artMeta}>
          <button type="button" className={s.wallBtn} aria-label="Change wallpaper" title="Change wallpaper" onClick={() => setWallOpen(true)}><ImageIcon /></button>
          {(info?.version || info?.channel) && <div className={s.build}>{[info?.version, info?.channel].filter(Boolean).join("  ·  ")}</div>}
        </div>
      </div>

      <Dialog
        open={editing} onClose={() => setEditing(false)} title="Launch arguments"
        actions={<><Button onClick={() => setEditing(false)}>Cancel</Button><Button kind="primary" disabled={view.readOnly} onClick={() => { void actions.setLaunchArgs(draft); setEditing(false); }}>Save</Button></>}
      >
        <TextField mono autoFocus value={draft} placeholder="e.g. -debug_mode" onChange={(e) => setDraft(e.target.value)} onKeyDown={(e) => e.key === "Enter" && (void actions.setLaunchArgs(draft), setEditing(false))} />
      </Dialog>
      <WallpaperDialog open={wallOpen} onClose={() => setWallOpen(false)} custom={view.art.custom} />
    </div>
  );
}

type WallJob = "url" | "file" | "reset";

function WallpaperDialog({ open, onClose, custom }: { open: boolean; onClose: () => void; custom: boolean }) {
  const running = useStore((st) => st.wallpaperBusy);
  const [url, setUrl] = useState("");
  const [job, setJob] = useState<WallJob | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Each opening is a session: a job started in an earlier one must not close or error this one.
  const session = useRef(0);
  useEffect(() => { if (open) { session.current++; setUrl(""); setError(null); } }, [open]);
  const busy = running ? job ?? "url" : null;
  const run = async (how: { url: string } | "file" | "reset", tag: WallJob) => {
    const mine = session.current;
    setJob(tag);
    setError(null);
    try {
      const done = await actions.setWallpaper(how);
      if (done && mine === session.current) onClose();
    } catch (e) {
      if (mine === session.current) setError(e instanceof Error ? e.message : String(e));
    }
  };
  const link = url.trim();
  const apply = () => { if (link && !busy) void run({ url: link }, "url"); };
  return (
    <Dialog
      open={open} onClose={onClose} title="Wallpaper" width={480}
      actions={<><Button onClick={onClose}>Cancel</Button><Button kind="primary" disabled={!link || busy !== null} onClick={apply}>{busy === "url" ? "Downloading…" : "Apply"}</Button></>}
    >
      <label className={s.field}>
        Image link
        <TextField autoFocus value={url} placeholder="https://wallhaven.cc/w/…" onChange={(e) => setUrl(e.target.value)} onKeyDown={(e) => e.key === "Enter" && apply()} />
      </label>
      <p className={s.hint}>A wallhaven page or any https image. PNG, JPEG or WebP up to 32 MB</p>
      {error && <p className={s.error} role="alert">{error}</p>}
      <div className={s.wallRow}>
        <Button icon={<FolderOpen />} disabled={busy !== null} onClick={() => void run("file", "file")}>{busy === "file" ? "Choosing…" : "Choose file…"}</Button>
        <Button kind="ghost" disabled={!custom || busy !== null} onClick={() => void run("reset", "reset")}>Reset to default</Button>
      </div>
    </Dialog>
  );
}

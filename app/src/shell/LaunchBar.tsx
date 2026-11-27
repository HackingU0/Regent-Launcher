import { useEffect, useRef, useState } from "react";
import { Play } from "lucide-react";
import { actions, activeAt, checksumUnknownNote, useStore } from "../store";
import { Button, Chip, Popover, TextField } from "../ui";
import s from "./LaunchBar.module.css";

/** Bottom bar on every page except Home: version, achievements pill, launch arguments, Launch. */
export function LaunchBar() {
  const view = useStore((st) => st.view);
  const launching = useStore((st) => st.launching);
  const info = useStore((st) => st.games.find((g) => g.id === st.game));
  const [draft, setDraft] = useState(view?.state.launchArgs ?? "");
  const saved = view?.state.launchArgs ?? "";
  useEffect(() => setDraft(saved), [saved]);
  if (!view) return null;
  /** Launch with what the field shows: waits for the real outcome of the args save (blur's or its own), aborts if it failed. */
  const launch = () => actions.launch([], draft);
  const running = view.running > 0;
  return (
    <footer className={s.bar}>
      <span className={s.version}>{info?.version ?? info?.title}</span>
      <AchievementsPill />
      <span className={s.grow} />
      <label className={s.args}>
        <span className={s.argsLabel}>Launch arguments</span>
        <TextField
          mono className={s.argsField} value={draft} placeholder="e.g. -debug_mode" disabled={view.readOnly}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={() => draft !== saved && void actions.setLaunchArgs(draft)}
          onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
        />
      </label>
      <Button kind="primary" size="lg" icon={<Play />} disabled={running || launching || view.readOnly || !activeAt(view)} onClick={() => void launch()}>
        {running ? "Running" : "Launch"}
      </Button>
    </footer>
  );
}

function AchievementsPill() {
  const mods = useStore((st) => st.view?.mods);
  const checksum = useStore((st) => st.summary?.checksum);
  const ref = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const names = new Map((mods ?? []).map((m) => [m.id, m.name]));
  const a = checksum?.achievements;
  const tone = a === true ? "ok" : a === false ? "warn" : "neutral";
  const text = a === true ? "Achievements OK" : a === false ? "Achievements off" : "Achievements unknown";
  const glyph = a === true ? "ok" : a === false ? "warn" : "○";
  return (
    <>
      <button ref={ref} type="button" className={s.pill} aria-haspopup="dialog" aria-expanded={open} onClick={() => setOpen(!open)}>
        <Chip tone={tone} glyph={glyph}>{text}</Chip>
      </button>
      <Popover anchor={ref} open={open} onClose={() => setOpen(false)} width={320} className={s.pop}>
        <div className={s.popBody}>
          {a === true && <p>No enabled mod changes the game checksum, so achievements stay on.</p>}
          {a == null && <p>Regent cannot tell: {checksumUnknownNote(checksum?.unknownReason)}.</p>}
          {a === false && <p>These change the game checksum and turn achievements off:</p>}
          {checksum && checksum.changing.length > 0 && (
            <ul className={s.list}>{checksum.changing.map((id) => <li key={id}>{names.get(id) ?? id}</li>)}</ul>
          )}
        </div>
      </Popover>
    </>
  );
}

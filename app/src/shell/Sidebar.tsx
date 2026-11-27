import { useCallback, useRef, useState } from "react";
import { ChevronsUpDown, Copy, MoreHorizontal, Pencil, Plus } from "lucide-react";
import { actions, activePlayset, useStore, type PageId } from "../store";
import { Button, Chip, Dialog, IconButton, Menu, MenuPanel, Select, TextField, Thumb, cx } from "../ui";
import s from "./Sidebar.module.css";
import ck3 from "./game-icons/ck3.png";
import eu4 from "./game-icons/eu4.png";
import hoi4 from "./game-icons/hoi4.png";
import stellaris from "./game-icons/stellaris.png";

/** Official Steam client icons (Stellaris tops out at 96px). Other ids fall back to the monogram. */
const ICONS: Record<string, string> = { ck3, eu4, hoi4, stellaris };

const NAV: { id: PageId; label: string }[] = [
  { id: "home", label: "Home" },
  { id: "mods", label: "Mods and DLC" },
  { id: "settings", label: "Game settings" },
  { id: "conflicts", label: "Conflicts" },
  { id: "saves", label: "Saves" },
  { id: "crash", label: "Crash reports" },
  { id: "share", label: "Share playset" },
];

export function Sidebar() {
  const page = useStore((st) => st.page);
  const fixture = useStore((st) => st.fixture);
  const conflicts = useStore((st) => st.summary?.pairs.length);
  const crashes = useStore((st) => st.view?.crashCount);
  const counts: Partial<Record<PageId, number | undefined>> = { conflicts, crash: crashes };
  return (
    <aside className={s.sidebar}>
      <div className={s.top} data-tauri-drag-region="deep">
        <span className={s.brand}>REGENT</span>
      </div>
      <GameSwitcher />
      <nav className={s.nav} aria-label="Pages">
        {NAV.map((n) => {
          const c = counts[n.id];
          return (
            <button key={n.id} type="button" className={cx(s.item, page === n.id && s.active)} aria-current={page === n.id ? "page" : undefined} onClick={() => actions.navigate(n.id)}>
              <span className={s.label}>{n.label}</span>
              {c != null && c > 0 && <span className={s.count}>{c}</span>}
            </button>
          );
        })}
      </nav>
      <div className={s.foot}>
        <PlaysetSwitcher />
        {fixture && <Chip className={s.fixture}>Fixture data</Chip>}
      </div>
    </aside>
  );
}

function GameSwitcher() {
  const games = useStore((st) => st.games);
  const game = useStore((st) => st.game);
  const view = useStore((st) => st.view);
  const ref = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const close = useCallback(() => setOpen(false), []);
  const cur = games.find((g) => g.id === game);
  if (!cur) return <div className={s.game} />;
  const body = (
    <>
      {ICONS[cur.id] ? <img className={s.gameIcon} src={ICONS[cur.id]} alt="" /> : <Thumb w={36} h={36} id={cur.id} name={cur.title} />}
      <span className={s.gameText}>
        <span className={s.gameTitle}>{cur.title}</span>
        <span className={s.gameMeta}>{[cur.channel, view?.running ? "running" : cur.version ? shortVersion(cur.version) : null].filter(Boolean).join(" · ")}</span>
      </span>
    </>
  );
  if (games.length < 2) return <div className={s.game}><div className={s.gameRow}>{body}</div></div>;
  return (
    <div className={s.game}>
      <button ref={ref} type="button" className={s.gameBtn} aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen(!open)}>
        {body}
        <ChevronsUpDown className={s.chev} />
      </button>
      <MenuPanel anchor={ref} open={open} onClose={close} align="start"
        items={games.map((g) => ({ label: g.title, hint: g.channel, onSelect: () => actions.selectGame(g.id), disabled: g.id === game }))} />
    </div>
  );
}

/** "Corona Borealis v4.6.0 (3d1a)" -> "v4.6.0" for the compact sidebar line. */
const shortVersion = (v: string) => v.match(/v?\d+(\.\d+)+/)?.[0] ?? v;

function PlaysetSwitcher() {
  const view = useStore((st) => st.view);
  const [dlg, setDlg] = useState<null | "new" | "rename">(null);
  const [name, setName] = useState("");
  if (!view) return <div className={s.playset} />;
  const ro = view.readOnly;
  const cur = activePlayset(view);
  const open = (m: "new" | "rename") => { setName(m === "rename" ? cur?.name ?? "" : ""); setDlg(m); };
  const taken = view.state.playsets.some((p, i) => p.name === name.trim() && (dlg === "new" || i !== view.state.active));
  const ok = name.trim().length > 0 && !taken;
  const submit = () => {
    if (!ok) return;
    void (dlg === "new" ? actions.playsetNew(name.trim()) : actions.playsetRename(name.trim()));
    setDlg(null);
  };
  return (
    <div className={s.playset}>
      <label className={s.psLabel} id="playset-label">Playset</label>
      <div className={s.psRow}>
        <Select
          aria-labelledby="playset-label" className={s.psSelect} value={String(view.state.active)} disabled={ro}
          options={view.state.playsets.map((p, i) => ({ value: String(i), label: p.name }))}
          onChange={(v) => void actions.playsetUse(Number(v))}
        />
        <Menu items={[
          { label: "New playset…", icon: <Plus />, onSelect: () => open("new") },
          { label: "Duplicate", icon: <Copy />, onSelect: () => void actions.playsetCopy() },
          { label: "Rename…", icon: <Pencil />, onSelect: () => open("rename") },
        ]} trigger={(p) => <IconButton label="Playset actions" icon={<MoreHorizontal />} disabled={ro} {...p} />} />
      </div>
      <Dialog open={dlg !== null} onClose={() => setDlg(null)} title={dlg === "new" ? "New playset" : "Rename playset"} width={380}
        actions={<><Button onClick={() => setDlg(null)}>Cancel</Button><Button kind="primary" disabled={!ok} onClick={submit}>{dlg === "new" ? "Create" : "Rename"}</Button></>}>
        <TextField autoFocus value={name} placeholder="Playset name" onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === "Enter" && submit()} />
        {taken && <p className={s.err}>A playset with that name already exists.</p>}
      </Dialog>
    </div>
  );
}

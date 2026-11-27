import { useMemo, useState, type ReactNode } from "react";
import { Archive, Copy, Download, Layers, Pencil, Play, Plus, RefreshCw, Trash2 } from "lucide-react";
import { actions } from "../store";
import { Shell } from "../shell/Shell";
import {
  Banner, Button, Chip, Dialog, Disclosure, EmptyState, IconButton, Menu, Meter, ModRow, PageHeader, Panel, ScrollArea, SearchField, Select,
  Skeleton, SkeletonRows, Slider, SortableVirtualList, SplitButton, Tabs, TextField, Thumb, Toggle, Tooltip, dragHandle,
} from "../ui";
import { mockMods, seedMockShell } from "./mock";
import s from "./Kit.module.css";

function Sec({ title, children, wide }: { title: string; children: ReactNode; wide?: boolean }) {
  return (
    <section className={wide ? s.wide : s.sec}>
      <h2 className={s.h}>{title}</h2>
      <div className={s.body}>{children}</div>
    </section>
  );
}
const Row = ({ children, label }: { children: ReactNode; label?: string }) => (
  <div className={s.row}>{label && <span className={s.lbl}>{label}</span>}{children}</div>
);

/** A drawn stand-in for a thumbnail, using token colours (the kit has no real mod art). */
function useArt() {
  return useMemo(() => {
    const c = document.createElement("canvas");
    c.width = c.height = 96;
    const g = c.getContext("2d")!;
    const css = getComputedStyle(document.documentElement);
    g.fillStyle = css.getPropertyValue("--raised").trim();
    g.fillRect(0, 0, 96, 96);
    g.strokeStyle = css.getPropertyValue("--lane").trim();
    g.fillStyle = css.getPropertyValue("--brass").trim();
    const pts = [[18, 70], [40, 40], [62, 54], [80, 22], [52, 82]];
    g.beginPath();
    pts.forEach(([x, y], i) => (i ? g.lineTo(x, y) : g.moveTo(x, y)));
    g.lineWidth = 2;
    g.stroke();
    pts.forEach(([x, y]) => { g.beginPath(); g.arc(x, y, 5, 0, 7); g.fill(); });
    return c.toDataURL();
  }, []);
}

export default function Kit({ mode }: { mode: string }) {
  if (mode === "shell" || mode === "home") {
    seedMockShell(mode === "home" ? "home" : "mods");
    return <Shell />;
  }
  return <KitBody />;
}

function KitBody() {
  const art = useArt();
  const [on, setOn] = useState([false, true, false, true]);
  const [sel, setSel] = useState("a");
  const [q, setQ] = useState("");
  const [vol, setVol] = useState(62);
  const [tab, setTab] = useState("mods");
  const [dlg, setDlg] = useState(false);
  const [lock, setLock] = useState(false);
  const [rows, setRows] = useState(() => mockMods(1000).map((m, i) => ({ ...m, enabled: i % 4 !== 3 })));
  const modsSmall = mockMods(5);

  const move = (key: string, before: string | null) =>
    setRows((r) => {
      const next = r.filter((x) => x.id !== key);
      const item = r.find((x) => x.id === key)!;
      const at = before ? next.findIndex((x) => x.id === before) : next.length;
      next.splice(at, 0, item);
      return next;
    });

  const modRow = (m: (typeof rows)[number], order: number, ctx?: Parameters<typeof dragHandle>[0]) => (
    <ModRow
      handle={ctx ? dragHandle(ctx) : undefined} order={order} off={!m.enabled}
      thumb={<Thumb src={order % 3 === 1 ? art : null} name={m.name} id={m.id} w={40} />}
      name={m.name}
      chips={<>
        {m.mismatch && <Chip tone="warn" glyph="warn">made for 4.4</Chip>}
        {order % 7 === 1 && <Chip tone="danger" glyph="conflict">{order} conflicts</Chip>}
      </>}
      meta={`${Math.round((m.size ?? 0) / 1e6)} MB · updated 2025-11-23 · in ${1 + (order % 3)} playset${order % 3 ? "s" : ""}`}
      toggle={<Toggle checked={m.enabled} label={`Enable ${m.name}`} onChange={(v) => setRows((r) => r.map((x) => (x.id === m.id ? { ...x, enabled: v } : x)))} />}
    />
  );

  return (
    <ScrollArea className={s.page}>
      <header className={s.top}>
        <h1 className={s.title}>Regent component kit</h1>
        <span className={s.sub}>Every component in every state. Open <code>?kit=shell</code> or <code>?kit=home</code> for the shell on mock data.</span>
      </header>
      <div className={s.grid}>
        <Sec title="Buttons">
          <Row label="primary"><Button kind="primary" icon={<Play />}>Launch</Button><Button kind="primary" disabled>Disabled</Button><Button kind="primary" size="lg" icon={<Play />}>Launch large</Button></Row>
          <Row label="quiet"><Button>Default</Button><Button icon={<Download />}>With icon</Button><Button disabled>Disabled</Button></Row>
          <Row label="danger"><Button kind="danger" icon={<Trash2 />}>Delete</Button><Button kind="danger" disabled>Disabled</Button></Row>
          <Row label="icon"><IconButton label="Refresh" icon={<RefreshCw />} /><IconButton label="New" icon={<Plus />} /><IconButton label="Copy" icon={<Copy />} /><IconButton label="Rename" icon={<Pencil />} disabled /></Row>
          <Row label="split"><SplitButton icon={<Play />} onClick={() => actions.toast("Launch", "ok")} items={[{ label: "Launch with debug_mode" }, { label: "Edit launch arguments…" }]}>Launch</SplitButton><SplitButton size="md" onClick={() => {}} items={[{ label: "One" }]} disabled>Disabled</SplitButton></Row>
        </Sec>

        <Sec title="Toggle">
          <Row label="off / on"><Toggle checked={on[0]} label="a" onChange={(v) => setOn([v, on[1], on[2], on[3]])} /><Toggle checked={on[1]} label="b" onChange={(v) => setOn([on[0], v, on[2], on[3]])} /></Row>
          <Row label="disabled"><Toggle checked={false} disabled label="c" onChange={() => {}} /><Toggle checked disabled label="d" onChange={() => {}} /></Row>
          <Row label="in a row">
            <div className={s.toggleRow}><span>Apocalypse</span><Toggle checked={on[2]} label="Apocalypse" onChange={(v) => setOn([on[0], on[1], v, on[3]])} /></div>
            <div className={s.toggleRow}><span>Federations</span><Toggle checked={on[3]} label="Federations" onChange={(v) => setOn([on[0], on[1], on[2], v])} /></div>
          </Row>
        </Sec>

        <Sec title="Inputs">
          <Row label="select"><Select value={sel} onChange={setSel} options={[{ value: "a", label: "Default" }, { value: "b", label: "Hardcore Run" }]} /><Select prefix="Sort" value="o" onChange={() => {}} options={[{ value: "o", label: "Load order" }]} /><Select disabled value="a" onChange={() => {}} options={[{ value: "a", label: "Disabled" }]} /></Row>
          <Row label="text"><TextField placeholder="Playset name" /><TextField mono defaultValue="-debug_mode" /><TextField disabled placeholder="Disabled" /></Row>
          <Row label="search"><SearchField value={q} onChange={setQ} /></Row>
          <Row label="slider"><Slider label="Volume" value={vol} onChange={setVol} /><Slider label="Off" value={0} onChange={() => {}} disabled /></Row>
        </Sec>

        <Sec title="Chips and status marks">
          <Row><Chip>neutral</Chip><Chip tone="ok" glyph="ok">4.6.*</Chip><Chip tone="warn" glyph="warn">made for 4.4</Chip><Chip tone="danger" glyph="conflict">23 conflicts</Chip><Chip tone="accent">Fixture</Chip><Chip tone="neutral" glyph="○">unknown</Chip><Chip tone="warn" onClick={() => {}}>clickable</Chip></Row>
        </Sec>

        <Sec title="Banner" wide>
          <Banner tone="info" action="Details">Regent re-read your playsets because the window was focused.</Banner>
          <Banner tone="warn" action="Review"><b>2 mods</b> were made for 4.4 and may break on 4.6.0.</Banner>
          <Banner tone="danger" action="Retry">Could not read playsets.json: permission denied.</Banner>
          <Banner tone="warn">A banner without an action.</Banner>
        </Sec>

        <Sec title="Menu, Popover, Dialog, Toast, Tooltip">
          <Row>
            <Menu items={[{ label: "New playset…", icon: <Plus /> }, { label: "Duplicate", icon: <Copy /> }, { label: "Rename…", icon: <Pencil />, hint: "F2" }, { label: "Delete", icon: <Trash2 />, danger: true, separator: true }]}
              trigger={(p) => <Button {...p}>Open menu</Button>} />
            <Button onClick={() => setDlg(true)}>Open dialog</Button>
            <Tooltip text="Reload mods from disk"><IconButton label="Refresh" icon={<RefreshCw />} /></Tooltip>
          </Row>
          <Row label="toasts"><Button onClick={() => actions.toast("Playset saved.", "ok")}>ok</Button><Button onClick={() => actions.toast("Mod list changed on disk.", "info")}>info</Button><Button onClick={() => actions.toast("2 mods were made for 4.4.", "warn")}>warn</Button><Button onClick={() => actions.toast("Could not write dlc_load.json.", "danger")}>danger</Button></Row>
          <Dialog open={dlg} onClose={() => setDlg(false)} title="Missing mods" actions={<><Button onClick={() => setDlg(false)}>Cancel</Button><Button kind="primary" onClick={() => setDlg(false)}>Play anyway</Button></>}>
            <p>This save was made with 2 mods that are not in the active playset.</p>
            <ul className={s.ul}><li>Planetary Diversity</li><li>Real Space</li></ul>
          </Dialog>
        </Sec>

        <Sec title="Thumb">
          <Row label="image / monogram / cjk / broken">
            <Thumb src={art} name="Image" id="x" w={40} /><Thumb name="Planetary Diversity" id="ugc_1" w={40} /><Thumb name="Real Space" id="ugc_2" w={40} /><Thumb name="更多科技树" id="ugc_3" w={40} /><Thumb src="thumb://broken" name="Broken Link" id="ugc_4" w={40} />
            <Thumb name="Federations" id="dlc_5" w={44} h={25} /><Thumb src={art} name="Wide" id="y" w={44} h={25} />
          </Row>
        </Sec>

        <Sec title="Tabs, Meter, Disclosure">
          <Row><Tabs label="demo" value={tab} onChange={setTab} tabs={[{ id: "mods", label: "Mods", count: 12 }, { id: "dlc", label: "DLC", count: 36 }]} /></Row>
          <Row label="meter"><div className={s.meters}><Meter value={0.82} label="a" /><Meter value={0.5} tone="warn" label="b" /><Meter value={0.2} tone="danger" label="c" /><Meter value={0.65} tone="ok" label="d" /><Meter value={0.4} tone="muted" label="e" /></div></Row>
          <Disclosure title="Advanced" meta="2 files"><p className={s.muted}>Raw editors live here.</p></Disclosure>
          <Disclosure title="Open by default" defaultOpen><p className={s.muted}>Body text.</p></Disclosure>
        </Sec>

        <Sec title="Skeleton, EmptyState, ScrollArea">
          <div className={s.box}><SkeletonRows count={3} height={56} thumb /></div>
          <Row><Skeleton w={120} /><Skeleton w={60} h={20} /></Row>
          <div className={s.box}><EmptyState icon={<Archive />} title="No saves yet" action={<Button>Open folder</Button>}>Launch the game once and your saves appear here.</EmptyState></div>
          <ScrollArea className={s.scrollDemo}>{Array.from({ length: 14 }, (_, i) => <div key={i} className={s.scrollRow}>Always-visible thin scrollbar, row {i + 1}</div>)}</ScrollArea>
        </Sec>

        <Sec title="Panel and PageHeader" wide>
          <div className={s.box}>
            <PageHeader title="Mods" count={3} actions={<><IconButton label="Refresh" icon={<RefreshCw />} /><Button icon={<Layers />}>Review</Button></>} sub="Drag the handle or press Alt+Up / Alt+Down to reorder." />
          </div>
          <Panel className={s.panelDemo}><span>A Panel with chart corner ticks.</span><span className={s.mono}>a.b.c</span></Panel>
        </Sec>

        <Sec title="ModRow states (56px)" wide>
          <div className={s.box}>
            {modsSmall.map((m, i) => <div key={m.id} className={s.rowFrame}>{modRow({ ...m, enabled: i !== 3 } as never, i + 1)}</div>)}
          </div>
        </Sec>

        <Sec title="SortableVirtualList, 1000 rows" wide>
          <Row><Toggle checked={!lock} label="Reorder enabled" onChange={(v) => setLock(!v)} /><span className={s.muted}>reorder {lock ? "disabled (handle hidden)" : "enabled"}: drag a handle, edge-scroll, Esc cancels, Alt+Arrow moves. {rows.length} rows.</span></Row>
          <div className={s.sortBox}>
            <SortableVirtualList
              items={rows} getKey={(m) => m.id} disabled={lock} label="Mods" rowLabel={(m) => m.name}
              onMove={(k, before) => move(k, before)}
              renderRow={(m, ctx) => modRow(m, ctx.index + 1, ctx)}
            />
          </div>
        </Sec>
      </div>
    </ScrollArea>
  );
}

import { useRef, useState } from "react";
import { Check, Copy, Download, ExternalLink, Search } from "lucide-react";
import { api, type ShareInspect } from "../../api";
import { actions, activeAt, useStore } from "../../store";
import { Banner, Button, Chip, EmptyState, PageHeader, ScrollArea, Skeleton, cx, useResource } from "../../ui";
import s from "./share.module.css";

/** Webview clipboard. In a plain browser (no secure context) it can fail; we then fall back to execCommand. */
async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.cssText = "position:fixed;opacity:0";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    ta.remove();
    return ok;
  }
}

const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;

function ExportCard({ game }: { game: string }) {
  const view = useStore((x) => x.view);
  const at = activeAt(view);
  const r = useResource(() => (at ? api.shareExport(game, at) : Promise.reject(new Error("No active playset"))), [game, at?.index, at?.name]);
  const [copied, setCopied] = useState<"code" | "json" | null>(null);
  const copy = async (kind: "code" | "json") => {
    if (!r.data) return;
    if (await copyText(kind === "code" ? r.data.code : r.data.json)) {
      setCopied(kind);
      setTimeout(() => setCopied(null), 1800);
      actions.toast(kind === "code" ? "Share code copied" : "JSON copied", "ok", 2500);
    } else actions.toast("The clipboard is not available here. Select the code and copy it by hand.", "warn");
  };
  return (
    <section className={s.card}>
      <header className={s.cardHead}>
        <h2 className={s.cardTitle}>Share this playset</h2>
        <p className={s.cardSub}>{at ? <>Send the code for <b>{at.name}</b> to a friend. It lists your mods, their order and which are on.</> : "No active playset."}</p>
      </header>
      {r.loading ? <Skeleton h={96} /> : r.error ? (
        <Banner tone="danger" action="Retry" onAction={r.reload}>{r.error}</Banner>
      ) : r.data && (
        <>
          <pre className={cx(s.code, "selectable")} tabIndex={0} aria-label="Share code">{r.data.code}</pre>
          <div className={s.row}>
            <Button kind="primary" icon={copied === "code" ? <Check size={15} /> : <Copy size={15} />} onClick={() => copy("code")}>{copied === "code" ? "Copied" : "Copy code"}</Button>
            <Button icon={copied === "json" ? <Check size={15} /> : <Copy size={15} />} onClick={() => copy("json")}>{copied === "json" ? "Copied" : "Copy JSON"}</Button>
          </div>
          <dl className={s.fp}>
            <dt>Regent fingerprint</dt>
            <dd className={cx(s.mono, "selectable")}>{r.data.fingerprint}</dd>
          </dl>
          <p className={s.note}>The fingerprint is Regent's own hash of the playset, not the game's checksum. Two playsets with the same fingerprint are identical.</p>
        </>
      )}
    </section>
  );
}

function InspectResult({ game, r, onImport, importing, readOnly }: { game: string; r: ShareInspect; onImport: () => void; importing: boolean; readOnly: boolean }) {
  const wrongGame = Boolean(r.game) && r.game !== game;
  const d = r.diff;
  return (
    <div className={s.result}>
      <div className={s.sumline}>
        <div>
          <div className={s.rname}>{r.name}</div>
          <div className={s.rmeta}>{[plural(r.modCount, "mod"), r.game].filter(Boolean).join(" · ")}{r.gameVersion ? ` ${r.gameVersion}` : ""}</div>
        </div>
        {r.equal ? <Chip glyph="ok" tone="ok">Same as yours</Chip> : <Chip glyph="warn" tone="warn">Differs from yours</Chip>}
      </div>
      {wrongGame && <Banner tone="danger">This playset is for {r.game}, not {game}. Switch game in the sidebar to import it.</Banner>}
      {r.missing.length > 0 && (
        <div className={s.block}>
          <h3 className={s.bh}>Not installed ({r.missing.length})</h3>
          <ul className={s.miss}>
            {r.missing.map((m) => (
              <li key={m.name} className={s.mrow}>
                <span className={s.mname}>{m.name}</span>
                {m.steam
                  ? <Button kind="ghost" icon={<ExternalLink size={14} />} onClick={() => void actions.run(() => api.open({ kind: "workshop", id: m.steam! }))}>Open in Workshop</Button>
                  : <span className={s.faint}>no Workshop id</span>}
              </li>
            ))}
          </ul>
          <p className={s.note}>Subscribe in Steam, then import. Missing mods stay disabled until they are installed.</p>
        </div>
      )}
      {r.lines.length > 0 && (
        <div className={s.block}>
          <h3 className={s.bh}>Differences</h3>
          <ul className={cx(s.diff, "selectable")}>{r.lines.map((l, i) => <li key={i}>{l}</li>)}</ul>
          {(d.extra.length > 0 || d.order) && <p className={s.note}>{[d.extra.length > 0 && `${plural(d.extra.length, "mod")} only you have`, d.order && "different load order"].filter(Boolean).join(" · ")}</p>}
        </div>
      )}
      <div className={s.row}>
        <Button kind="primary" icon={<Download size={15} />} disabled={readOnly || wrongGame || importing} onClick={onImport}>{importing ? "Importing…" : "Import"}</Button>
        {readOnly && <span className={s.faint}>Read-only: the game is running or files are locked.</span>}
      </div>
    </div>
  );
}

function ImportCard({ game }: { game: string }) {
  const readOnly = useStore((x) => x.view?.readOnly ?? false);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [importing, setImporting] = useState(false);
  // The inspected result keeps the exact text it was made from; Import only ever sends that text.
  const [res, setRes] = useState<{ text: string; r: ShareInspect } | null>(null);
  const latest = useRef("");
  latest.current = text.trim();
  const [err, setErr] = useState<string | null>(null);

  const inspect = async () => {
    const t = text.trim();
    setBusy(true); setErr(null); setRes(null);
    try {
      const r = await api.shareInspect(game, t);
      if (latest.current === t) setRes({ text: t, r });
    } catch (e) { if (latest.current === t) setErr(e instanceof Error ? e.message : String(e)); }
    setBusy(false);
  };
  const paste = async () => {
    try { setText(await navigator.clipboard.readText()); } catch { actions.toast("Cannot read the clipboard here. Paste into the box with Cmd/Ctrl+V.", "warn"); }
  };
  const doImport = async () => {
    if (!res) return;
    setImporting(true);
    const r = await actions.run(() => api.shareImport(game, res.text));
    setImporting(false);
    if (!r) return;
    actions.toast(r.missing.length ? `Imported ${r.name}. ${plural(r.missing.length, "missing mod")} stay disabled.` : `Imported ${r.name}`, r.missing.length ? "warn" : "ok");
    void actions.refresh(false);
    setRes(null); setText("");
  };

  return (
    <section className={s.card}>
      <header className={s.cardHead}>
        <h2 className={s.cardTitle}>Import or verify</h2>
        <p className={s.cardSub}>Paste a share code or JSON. Inspect compares it with your mods before anything changes.</p>
      </header>
      <textarea className={cx(s.paste, "selectable")} value={text} placeholder="Paste a share code or JSON here" spellCheck={false}
        aria-label="Share code or JSON" onChange={(e) => { setText(e.target.value); setRes(null); setErr(null); }} />
      <div className={s.row}>
        <Button kind={res ? "quiet" : "primary"} icon={<Search size={15} />} disabled={!text.trim() || busy} onClick={inspect}>{busy ? "Inspecting…" : "Inspect"}</Button>
        <Button kind="ghost" onClick={paste}>Paste from clipboard</Button>
      </div>
      {err && <Banner tone="danger">{err}</Banner>}
      {res && res.text === text.trim() && <InspectResult game={game} r={res.r} onImport={doImport} importing={importing} readOnly={readOnly} />}
      {!(res && res.text === text.trim()) && !err && !busy && <EmptyState className={s.hintEmpty}>Nothing is changed until you press Import.</EmptyState>}
    </section>
  );
}

export default function SharePage() {
  const game = useStore((x) => x.game);
  return (
    <>
      <PageHeader title="Share playset" />
      <ScrollArea className={s.body}>
        {game && <div className={s.grid}><ExportCard game={game} /><ImportCard game={game} /></div>}
      </ScrollArea>
    </>
  );
}

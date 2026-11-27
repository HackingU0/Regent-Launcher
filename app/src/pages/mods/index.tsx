import { useEffect, useMemo, useState } from "react";
import { MoreHorizontal, PackageOpen, RefreshCw, Rows3 } from "lucide-react";
import { formatDate, formatSize, thumbUrl, type ModInfo } from "../../api";
import { actions, inPlaysets, mismatchCount, useStore, visibleEntries } from "../../store";
import {
  Banner, Chip, EmptyState, IconButton, Menu, ModRow, PageHeader, SearchField, Select, SkeletonRows, SortableVirtualList, Tabs, Thumb, Toggle, dragHandle,
} from "../../ui";
import { DlcPanel } from "./DlcPanel";
import s from "./Mods.module.css";

type Sort = "load" | "name" | "size" | "updated";
type Filter = "all" | "enabled" | "attention";
type Tab = "mods" | "dlc";
interface Item { mod: ModInfo; enabled: boolean; order: number }

const SORTS = [
  { value: "load", label: "Load order" }, { value: "name", label: "Name" }, { value: "size", label: "Size" }, { value: "updated", label: "Updated" },
];

function useNarrow() {
  const q = "(max-width: 1099px)";
  const [narrow, setNarrow] = useState(() => matchMedia(q).matches);
  useEffect(() => {
    const m = matchMedia(q);
    const f = () => setNarrow(m.matches);
    m.addEventListener("change", f);
    return () => m.removeEventListener("change", f);
  }, []);
  return narrow;
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

export default function ModsPage() {
  const view = useStore((st) => st.view);
  const summary = useStore((st) => st.summary);
  const params = useStore((st) => st.pageParams);
  const loading = useStore((st) => st.loading);
  const narrow = useNarrow();
  const [q, setQ] = useState("");
  const [sort, setSort] = useState<Sort>("load");
  const [filter, setFilter] = useState<Filter>(params.filter === "attention" || params.filter === "enabled" ? params.filter : "all");
  const [tab, setTab] = useState<Tab>(params.tab === "dlc" ? "dlc" : "mods");
  const [compact, setCompact] = useState(false);

  const items = useMemo<Item[]>(() => {
    if (!view) return [];
    const byId = new Map(view.mods.map((m) => [m.id, m]));
    return visibleEntries(view).flatMap((e, i) => {
      const mod = byId.get(e.id);
      return mod ? [{ mod, enabled: e.enabled, order: i + 1 }] : [];
    });
  }, [view]);
  const sizes = summary?.sizes;
  const sizeOf = (m: ModInfo) => m.size ?? sizes?.[m.id] ?? null;

  const attention = (it: Item) => it.enabled && it.mod.mismatch;
  const counts = { all: items.length, enabled: items.filter((i) => i.enabled).length, attention: items.filter(attention).length };

  const shown = useMemo(() => {
    const needle = q.trim().toLowerCase();
    let out = items.filter((it) => (filter === "enabled" ? it.enabled : filter === "attention" ? it.enabled && it.mod.mismatch : true));
    if (needle) out = out.filter((it) => it.mod.name.toLowerCase().includes(needle));
    if (sort === "name") out = [...out].sort((a, b) => a.mod.name.localeCompare(b.mod.name));
    else if (sort === "size") out = [...out].sort((a, b) => (sizeOf(b.mod) ?? -1) - (sizeOf(a.mod) ?? -1));
    else if (sort === "updated") out = [...out].sort((a, b) => (toSec(b.mod.updated)) - toSec(a.mod.updated));
    return out;
  }, [items, q, sort, filter, sizes]);

  if (!view) return null;
  const readOnly = view.readOnly;
  const reorder = !q.trim() && sort === "load" && filter === "all" && !readOnly;
  const totalSize = items.reduce((n, it) => n + (sizeOf(it.mod) ?? 0), 0);
  const mism = mismatchCount(view);
  const playset = view.state.playsets[view.state.active]?.name;
  const dlcOn = view.dlcs.length - (view.state.playsets[view.state.active]?.disabledDlcs.filter((d) => view.dlcs.some((x) => x.id === d)).length ?? 0);
  const showMods = !narrow || tab === "mods";
  const showDlc = !narrow || tab === "dlc";
  const setAll = (on: boolean) => void actions.setModsEnabled(shown.map((i) => i.mod.id), on);

  return (
    <>
      <PageHeader
        title="Mods and DLC" count={items.length} sub={playset ? `Playset ${playset}` : undefined}
        actions={
          <>
            <IconButton label="Rescan mods" icon={<RefreshCw />} onClick={() => void actions.refresh(true)} />
            <IconButton label="Compact view" icon={<Rows3 />} aria-pressed={compact} className={s.press} onClick={() => setCompact(!compact)} />
            <Menu
              trigger={(p) => <IconButton label="More actions" icon={<MoreHorizontal />} {...p} />}
              items={[
                { label: `Turn on ${shown.length === items.length ? "all" : `${shown.length} shown`}`, disabled: readOnly || !shown.length, onSelect: () => setAll(true) },
                { label: `Turn off ${shown.length === items.length ? "all" : `${shown.length} shown`}`, disabled: readOnly || !shown.length, onSelect: () => setAll(false) },
              ]}
            />
          </>
        }
      />
      {narrow && (
        <div className={s.tabs}>
          <Tabs label="Mods or DLC" value={tab} onChange={setTab} tabs={[{ id: "mods", label: "Mods", count: items.length }, { id: "dlc", label: "DLC", count: view.dlcs.length }]} />
        </div>
      )}
      <div className={s.body}>
        {showMods && (
          <section className={s.main} aria-label="Mods">
            <div className={s.toolbar}>
              <SearchField className={s.search} value={q} onChange={setQ} placeholder="Search mods" aria-label="Search mods" />
              <Select prefix="Sort" value={sort} onChange={(v) => setSort(v as Sort)} options={SORTS} aria-label="Sort mods" />
            </div>
            <div className={s.chips} role="group" aria-label="Filter">
              {(["all", "enabled", "attention"] as const).map((f) => (
                <button key={f} type="button" className={s.fchip} aria-pressed={filter === f} onClick={() => setFilter(f)}>
                  {f === "all" ? "All" : f === "enabled" ? "Enabled" : "Needs attention"}
                  <span className={s.n}>{counts[f]}</span>
                </button>
              ))}
            </div>
            {mism > 0 && filter !== "attention" && (
              <Banner tone="warn" className={s.banner} action="Review" onAction={() => setFilter("attention")}>
                {plural(mism, "enabled mod was", "enabled mods were")} made for another version of the game and may misbehave.
              </Banner>
            )}
            {loading ? (
              <div className={s.pad}><SkeletonRows count={9} height={56} thumb /></div>
            ) : items.length === 0 ? (
              <EmptyState className={s.empty} icon={<PackageOpen />} title="No mods in this playset">
                Subscribe to mods in the Steam Workshop. They show up here after a rescan.
              </EmptyState>
            ) : shown.length === 0 ? (
              <EmptyState className={s.empty} icon={<PackageOpen />} title={q.trim() ? `Nothing matches "${q.trim()}"` : filter === "attention" ? "Nothing needs attention" : "No mods on"}
                action={<button type="button" className={s.link} onClick={() => { setQ(""); setFilter("all"); }}>Show all mods</button>}>
                {q.trim() ? "Check the spelling, or clear the search." : "Every enabled mod matches this game version."}
              </EmptyState>
            ) : (
              <SortableVirtualList
                className={s.list} rowHeight={compact ? 60 : 104} items={shown} label="Mods" scope={`${view.game}:${view.state.active}`} getKey={(it) => it.mod.id} disabled={!reorder}
                rowLabel={(it) => it.mod.name}
                onMove={(id, before) => void actions.moveMod(id, before)}
                renderRow={(it, ctx) => (
                  <ModRow
                    className={s.row} off={!it.enabled}
                    handle={dragHandle(ctx)} order={it.order}
                    thumb={<Thumb w={compact ? 40 : 136} h={compact ? 40 : 78} id={it.mod.id} name={it.mod.name} src={it.mod.hasThumb ? thumbUrl(view.game, "mod", it.mod.id) : null} />}
                    name={it.mod.name}
                    chips={<Chips mod={it.mod} conflicts={summary?.conflictsByMod[it.mod.id] ?? 0} />}
                    meta={[sizeOf(it.mod) != null && formatSize(sizeOf(it.mod)), it.mod.updated != null && `Updated ${formatDate(it.mod.updated)}`, `In ${plural(inPlaysets(view, it.mod.id), "playset")}`]
                      .filter(Boolean).map((t) => <span key={t as string}>{t}</span>)}
                    toggle={<Toggle checked={it.enabled} label={`${it.mod.name} enabled`} disabled={readOnly || ctx.ghost} onChange={(on) => void actions.setModsEnabled([it.mod.id], on)} />}
                  />
                )}
              />
            )}
            <footer className={s.foot}>
              <span>{plural(items.length, "mod")}</span><span>{counts.enabled} on</span>{totalSize > 0 && <span>{formatSize(totalSize)}</span>}
              {shown.length !== items.length && <span className={s.showing}>showing {shown.length}</span>}
            </footer>
          </section>
        )}
        {showDlc && <DlcPanel dlcs={view.dlcs} state={view.state} game={view.game} readOnly={readOnly} on={dlcOn} wide={narrow} />}
      </div>
    </>
  );
}

const toSec = (v: ModInfo["updated"]) => (typeof v === "number" ? (v < 1e12 ? v : v / 1000) : v ? Date.parse(v) / 1000 || 0 : 0);

function Chips({ mod, conflicts }: { mod: ModInfo; conflicts: number }) {
  return (
    <>
      {conflicts > 0 && <Chip tone="accent" glyph="conflict">{plural(conflicts, "conflict")}</Chip>}
      {mod.mismatch
        ? <Chip tone="warn" glyph="warn" title="Made for a different game version">made for {trimV(mod.supportedVersion) ?? "another version"}</Chip>
        : mod.supportedVersion && <Chip tone="ok" glyph="ok" className={s.ver}>{trimV(mod.supportedVersion)}</Chip>}
    </>
  );
}
const trimV = (v?: string | null) => v?.replace(/^v/i, "");

import { useMemo } from "react";
import { thumbUrl, type DlcInfo, type GameState } from "../../api";
import { actions } from "../../store";
import { Button, Thumb, Toggle, cx } from "../../ui";
import s from "./DlcPanel.module.css";

const GROUPS: { id: string; label: string; match: (c: string) => boolean }[] = [
  { id: "expansion", label: "Expansions", match: (c) => c === "expansion" },
  { id: "story_pack", label: "Story packs", match: (c) => c === "story_pack" },
  { id: "species_pack", label: "Species packs", match: (c) => c === "species_pack" },
  { id: "content_pack", label: "Content packs", match: (c) => c === "content_pack" },
];

export function DlcPanel({ dlcs, state, game, readOnly, on, wide }: { dlcs: DlcInfo[]; state: GameState; game: string; readOnly: boolean; on: number; wide: boolean }) {
  const off = useMemo(() => new Set(state.playsets[state.active]?.disabledDlcs ?? []), [state]);
  const groups = useMemo(() => {
    const known = GROUPS.map((g) => ({ ...g, items: dlcs.filter((d) => g.match(d.category ?? "")) }));
    const other = dlcs.filter((d) => !GROUPS.some((g) => g.match(d.category ?? "")));
    return [...known, { id: "other", label: "Other", match: () => true, items: other }].filter((g) => g.items.length);
  }, [dlcs]);
  const allOn = on === dlcs.length;
  return (
    <aside className={cx(s.panel, wide && s.wide)} aria-label="DLC">
      <header className={s.head}>
        <div>
          <h2 className={s.title}>DLC</h2>
          <p className={s.sub}>{on} of {dlcs.length} on</p>
        </div>
        <Button disabled={readOnly || !dlcs.length} onClick={() => void actions.setDlcsEnabled(dlcs.map((d) => d.id), !allOn)}>{allOn ? "Turn all off" : "Turn all on"}</Button>
      </header>
      <div className={s.scroll}>
        {dlcs.length === 0 && <p className={s.none}>No DLC installed.</p>}
        {groups.map((g) => {
          const gOn = g.items.filter((d) => !off.has(d.id)).length;
          return (
            <section key={g.id} aria-label={g.label}>
              <div className={s.ghead}>
                <span className={s.gname}>{g.label}</span>
                <span className={s.gcount}>{gOn}/{g.items.length}</span>
                <Toggle checked={gOn === g.items.length} label={`${g.label} all`} disabled={readOnly} onChange={(v) => void actions.setDlcsEnabled(g.items.map((d) => d.id), v)} />
              </div>
              {g.items.map((d) => {
                const enabled = !off.has(d.id);
                return (
                  <div key={d.id} className={cx(s.row, !enabled && s.off)}>
                    <Thumb w={52} h={30} id={d.id} name={d.name} src={d.hasThumb ? thumbUrl(game, "dlc", d.id) : null} />
                    <span className={s.name} title={d.name}>{d.name}</span>
                    <Toggle checked={enabled} label={`${d.name} enabled`} disabled={readOnly} onChange={(v) => void actions.setDlcsEnabled([d.id], v)} />
                  </div>
                );
              })}
            </section>
          );
        })}
      </div>
    </aside>
  );
}

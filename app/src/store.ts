// One tiny store (useSyncExternalStore). Pages read with `useStore(selector)` and mutate through `actions`.
// Selectors must return stable references (a field, not a fresh object/array).
import { useSyncExternalStore } from "react";
import {
  AppError, api, onRefresh,
  type At, type Entry, type GameId, type GameInfo, type GameState, type GameView, type ModInfo, type Playset, type Summary,
} from "./api";
import { atFor, mayLaunch, responseIsCurrent, showResultNow } from "./queue";

export type PageId = "home" | "mods" | "settings" | "conflicts" | "saves" | "crash" | "share";
export type ToastKind = "info" | "ok" | "warn" | "danger";
export interface Toast {
  id: number;
  kind: ToastKind;
  text: string;
}

export interface State {
  booted: boolean;
  bootError: string | null;
  games: GameInfo[];
  fixture: boolean;
  game: GameId | null;
  page: PageId;
  /** Optional hint for the target page, e.g. navigate("mods", { filter: "attention" }). */
  pageParams: Record<string, string>;
  view: GameView | null;
  /** A launch is queued or starting: Launch buttons stay off. */
  launching: boolean;
  loading: boolean;
  loadError: string | null;
  summary: Summary | null;
  /** Bumps on every window-focus refresh; pages with their own data re-read on it. */
  focusTick: number;
  /** Bumps whenever the game's art may have changed; part of every art URL. */
  artTick: number;
  /** A wallpaper set/reset is running; survives the dialog closing so a second one cannot race it. */
  wallpaperBusy: boolean;
  toasts: Toast[];
}

let state: State = {
  booted: false, bootError: null, games: [], fixture: false, game: null, page: "home", pageParams: {},
  view: null, launching: false, loading: false, loadError: null, summary: null, focusTick: 0, artTick: 0, wallpaperBusy: false, toasts: [],
};
const subs = new Set<() => void>();
function set(patch: Partial<State>) {
  state = { ...state, ...patch };
  subs.forEach((f) => f());
}
export const getState = () => state;
export function useStore<T>(selector: (s: State) => T): T {
  return useSyncExternalStore(
    (f) => (subs.add(f), () => subs.delete(f)),
    () => selector(state),
  );
}
/** Kit/mock only: replace state wholesale so the shell renders without the backend. */
export const seedState = (patch: Partial<State>) => set(patch);

// ---------------------------------------------------------------- derived helpers
export const activePlayset = (v: GameView | null) => v?.state.playsets[v.state.active];
export const activeAt = (v: GameView | null): At | null => atFor(v?.state);
/** Why the achievements state is unknown, as one sentence fragment. */
export const checksumUnknownNote = (r: "noManifest" | "incomplete" | null | undefined) =>
  r === "incomplete" ? "the checksum manifest could not be fully read" : "this install has no checksum manifest";
/** The `visible` rule: drop entries whose id is not an installed mod, before numbering rows. */
export function visibleEntries(v: GameView | null): Entry[] {
  const p = activePlayset(v);
  if (!v || !p) return [];
  const known = new Set(v.mods.map((m) => m.id));
  return p.mods.filter((e) => known.has(e.id));
}
export const modsById = (v: GameView | null): Map<string, ModInfo> => new Map((v?.mods ?? []).map((m) => [m.id, m]));
export const inPlaysets = (v: GameView | null, id: string) => v?.state.playsets.filter((p) => p.mods.some((e) => e.id === id)).length ?? 0;
export const mismatchCount = (v: GameView | null) => {
  const on = new Set(visibleEntries(v).filter((e) => e.enabled).map((e) => e.id));
  return v?.mods.filter((m) => m.mismatch && on.has(m.id)).length ?? 0;
};

// ---------------------------------------------------------------- toasts
let toastId = 0;
function toast(text: string, kind: ToastKind = "info", ms = 4500) {
  const id = ++toastId;
  set({ toasts: [...state.toasts, { id, kind, text }] });
  if (ms > 0) setTimeout(() => dismissToast(id), ms);
}
const dismissToast = (id: number) => set({ toasts: state.toasts.filter((t) => t.id !== id) });

// ---------------------------------------------------------------- mutation queue
// Mutating commands run one at a time. `pending` counts queued + running ones; `mutEpoch` bumps on every enqueue.
// A load/summary response is only applied if no mutation could have overtaken it.
let pending = 0;
let mutEpoch = 0;
let tail: Promise<unknown> = Promise.resolve();
/** Last server-confirmed state of the current game: what a failed optimistic edit rolls back to. */
let confirmed: GameState | null = null;
function enqueue<T>(f: () => Promise<T>): Promise<T> {
  pending++;
  mutEpoch++;
  const p = tail.then(f).finally(() => { pending--; });
  tail = p.catch(() => {});
  return p;
}

// ---------------------------------------------------------------- loading
let loadSeq = 0;
let loadsInFlight = 0;
async function load(rescan: boolean): Promise<void> {
  const game = state.game;
  if (!game) return;
  const seq = ++loadSeq;
  const epoch = mutEpoch;
  loadsInFlight++;
  set({ loading: state.view?.game !== game, loadError: null });
  try {
    const view = await api.load(game, rescan);
    if (seq !== loadSeq || state.game !== game) return;
    // A mutation was queued before or during this load: its answer is newer than ours, so read again once it is done.
    if (pending > 0 || epoch !== mutEpoch) {
      await tail;
      if (seq === loadSeq) return load(false);
      return;
    }
    const prev = state.view;
    const changed = !prev || prev.game !== game || JSON.stringify(prev) !== JSON.stringify(view);
    confirmed = view.state;
    set(changed ? { view, loading: false } : { loading: false });
    if (view.warning) toast(view.warning, "warn");
    // Nothing changed on a plain refresh: the summary is still right. An explicit rescan always recomputes it.
    void ensureSummary(rescan || changed || !state.summary);
  } catch (e) {
    if (seq !== loadSeq) return;
    set({ loading: false, loadError: msg(e) });
  } finally {
    loadsInFlight--;
    if (loadsInFlight === 0 && refreshQueued) {
      refreshQueued = false;
      void load(false);
    }
  }
}

let refreshQueued = false;
/** Re-read the view once no load is running: a background art/thumbs event must neither be lost to nor supersede a running load. */
function refreshWhenIdle() {
  if (loadsInFlight > 0) refreshQueued = true;
  else void load(false);
}

let summaryFlight: Promise<Summary | null> | null = null;
let summarySeq = 0;
/** Shared in-flight `summary` call. `fresh` forces a new one (after mutations); only the newest answer is applied. */
function ensureSummary(fresh = false): Promise<Summary | null> {
  const game = state.game;
  if (!game) return Promise.resolve(null);
  if (!fresh && state.summary && state.view?.game === game) return Promise.resolve(state.summary);
  if (summaryFlight && !fresh) return summaryFlight;
  const seq = ++summarySeq;
  const epoch = mutEpoch;
  const p: Promise<Summary | null> = api.summary(game).then(
    (summary) => {
      if (summaryFlight === p) summaryFlight = null;
      // Older than a mutation enqueued since: that mutation asks for its own summary.
      if (state.game !== game || seq !== summarySeq || !responseIsCurrent(epoch, mutEpoch)) return state.summary;
      set({ summary });
      return summary;
    },
    (e) => {
      if (summaryFlight === p) summaryFlight = null;
      if (seq === summarySeq) toast(msg(e), "danger");
      return null;
    },
  );
  summaryFlight = p;
  return p;
}

const msg = (e: unknown) => (e instanceof Error ? e.message : String(e));
const isStale = (e: unknown) => e instanceof AppError && e.kind === "stale";

/** Run a backend call. `stale` reloads and toasts; other errors toast. Resolves undefined on failure. */
async function run<T>(f: () => Promise<T>): Promise<T | undefined> {
  try {
    return await f();
  } catch (e) {
    if (isStale(e)) {
      toast("Data changed outside Regent. Reloaded.", "warn");
      void load(false);
    } else toast(msg(e), "danger");
    return undefined;
  }
}

/** Clone the active playset (entries included) and let `f` edit the clone; never mutates shared objects. */
function patchState(f: (s: GameState, p: Playset | undefined) => void) {
  const v = state.view;
  if (!v) return;
  const s0 = v.state;
  const playsets = s0.playsets.map((p, i) => (i === s0.active ? { ...p, mods: p.mods.map((e) => ({ ...e })), disabledDlcs: [...p.disabledDlcs] } : p));
  const next = { ...s0, playsets };
  f(next, playsets[s0.active]);
  set({ view: { ...v, state: next } });
}

/**
 * Run a mutating command that returns the new GameState, one at a time, after any earlier ones.
 * `optimistic` edits the shown state right away; on failure the last confirmed state comes back.
 */
function mutate(f: (game: GameId, at: At) => Promise<GameState>, optimistic?: Parameters<typeof patchState>[0]): Promise<boolean> {
  const game = state.game;
  if (!game || !state.view) return Promise.resolve(false);
  if (optimistic) patchState(optimistic);
  return enqueue(async (): Promise<boolean | "stale"> => {
    // At is read now, from the latest applied state, so an action queued behind a playset switch targets the new playset.
    const at = activeAt(state.view);
    let next: GameState | null = null;
    let err: unknown;
    if (state.game === game && at) {
      try { next = await f(game, at); } catch (e) { err = e; }
    }
    if (state.game !== game || !state.view) return false;
    if (next) {
      confirmed = next;
      if (showResultNow(!!optimistic, pending)) set({ view: { ...state.view, state: next } });
      void ensureSummary(true);
      return true;
    }
    // Whatever is queued next must read confirmed state, not this failed edit.
    if (confirmed) set({ view: { ...state.view, state: confirmed } });
    void ensureSummary(true);
    if (err !== undefined && !isStale(err)) toast(msg(err), "danger");
    return isStale(err) ? "stale" : false;
  }).then((r) => {
    if (r !== "stale") return r;
    toast("Data changed outside Regent. Reloaded.", "warn");
    void load(false);
    return false;
  });
}

// ---------------------------------------------------------------- actions
let unlisten: (() => void) | null = null;
const stopListening = () => { unlisten?.(); unlisten = null; };
// Vite hot reload re-evaluates this module: drop the old subscription so there is exactly one.
import.meta.hot?.dispose(stopListening);

let argsFlight: { args: string; done: Promise<boolean> } | null = null;

export const actions = {
  toast, dismissToast, ensureSummary,

  async boot() {
    try {
      const r = await api.games();
      const first = r.games[0]?.id ?? null;
      confirmed = null;
      set({ booted: true, games: r.games, fixture: r.fixture, game: first, bootError: null });
      if (r.warning) toast(r.warning, "warn");
      document.documentElement.dataset.game = first ?? "";
      unlisten ??= onRefresh(
        (p) => {
          if (p.warning) toast(p.warning, "warn");
          // A default wallpaper or workshop previews arrived: new art URLs now, fresh flags once loads settle.
          if (p.why === "art" || p.why === "thumbs") {
            if (p.game !== state.game) return;
            if (p.why === "art") set({ artTick: state.artTick + 1 });
            refreshWhenIdle();
            return;
          }
          if (p.why === "focus") set({ focusTick: state.focusTick + 1 });
          const rescan = p.why === "exit" && p.game === state.game;
          if (!rescan && loadsInFlight > 0) return; // a focus refresh while a load is running adds nothing
          void load(rescan);
        },
        (e) => { unlisten = null; toast(`Background refresh is off: ${msg(e)}`, "warn"); },
      );
      await load(true);
    } catch (e) {
      set({ booted: true, bootError: msg(e) });
    }
  },
  /** Drop the `regent:refresh` subscription (app teardown). */
  dispose: stopListening,
  navigate(page: PageId, pageParams: Record<string, string> = {}) {
    set({ page, pageParams });
  },
  selectGame(game: GameId) {
    if (game === state.game) return;
    document.documentElement.dataset.game = game;
    confirmed = null;
    set({ game, view: null, summary: null, page: "home", pageParams: {} });
    void load(true);
  },
  /** Re-read everything; rescan=true also rescans installed mods. */
  refresh: (rescan = true) => load(rescan),

  setModsEnabled: (ids: string[], on: boolean) =>
    mutate((g, at) => api.setModsEnabled(g, at, ids, on), (_, p) => p?.mods.forEach((e) => ids.includes(e.id) && (e.enabled = on))),
  setDlcsEnabled: (ids: string[], on: boolean) =>
    mutate((g, at) => api.setDlcsEnabled(g, at, ids, on), (_, p) => {
      if (!p) return;
      const off = new Set(p.disabledDlcs);
      ids.forEach((id) => (on ? off.delete(id) : off.add(id)));
      p.disabledDlcs = [...off];
    }),
  /** Optimistic: reorders locally, then asks the backend. `beforeId` null/undefined = append. */
  moveMod: (id: string, beforeId?: string | null) =>
    mutate((g, at) => api.moveMod(g, at, id, beforeId), (_, p) => {
      if (!p) return;
      const from = p.mods.findIndex((e) => e.id === id);
      if (from < 0) return;
      const [e] = p.mods.splice(from, 1);
      const to = beforeId ? p.mods.findIndex((x) => x.id === beforeId) : -1;
      p.mods.splice(to < 0 ? p.mods.length : to, 0, e);
    }),
  applyOrder: () => mutate((g, at) => api.applyOrder(g, at)),
  playsetNew: (name: string) => mutate((g) => api.playsetNew(g, name)),
  playsetCopy: (name?: string) => mutate((g, at) => api.playsetCopy(g, at, name)),
  playsetRename: (name: string) => mutate((g, at) => api.playsetRename(g, at, name)),
  playsetUse: (index: number) => {
    const name = state.view?.state.playsets[index]?.name;
    return name == null ? Promise.resolve(false) : mutate((g) => api.playsetUse(g, index, name));
  },
  /** Shown at once. A repeat call with the same text returns the SAME in-flight save, so a caller always learns the real outcome. */
  /** Change this game's wallpaper. False when the file picker was cancelled; throws AppError on failure. */
  async setWallpaper(how: { url: string } | "file" | "reset"): Promise<boolean> {
    const game = state.game;
    if (!game || state.wallpaperBusy) return false;
    set({ wallpaperBusy: true });
    try {
      const art = how === "reset" ? await api.wallpaperReset(game)
        : how === "file" ? await api.wallpaperPickFile(game)
        : await api.wallpaperSetUrl(game, how.url);
      if (!art) return false;
      mutEpoch++; // a load already in flight computed the old art: make it read again
      if (state.game === game && state.view) set({ view: { ...state.view, art }, artTick: state.artTick + 1 });
      return true;
    } finally {
      set({ wallpaperBusy: false });
    }
  },
  setLaunchArgs(args: string): Promise<boolean> {
    if (argsFlight?.args === args) return argsFlight.done;
    if (!argsFlight && confirmed?.launchArgs === args) return Promise.resolve(true);
    const done: Promise<boolean> = mutate((g) => api.setLaunchArgs(g, args), (s) => { s.launchArgs = args; }).finally(() => {
      if (argsFlight?.done === done) argsFlight = null;
    });
    argsFlight = { args, done };
    return done;
  },

  /**
   * The one launch gate (Launch bar, Home, Saves Play): one launch at a time, queued after pending edits.
   * `args` (launch bar only) is saved first; if that save fails nothing launches. `f` gets the At current when it runs.
   */
  async guardedLaunch<T>(f: (game: GameId, at: At) => Promise<T>, args?: string): Promise<T | undefined> {
    const game = state.game;
    if (!game || state.launching) return undefined;
    set({ launching: true });
    try {
      if (args !== undefined && !mayLaunch(await actions.setLaunchArgs(args))) {
        toast("Launch cancelled: the launch arguments were not saved.", "danger");
        return undefined;
      }
      const r = await enqueue(async () => {
        const at = activeAt(state.view);
        if (!at || state.game !== game) return undefined;
        return run(() => f(game, at));
      });
      // A failed launch must not leave an edit that also failed drawn on screen.
      if (r === undefined && confirmed && pending === 0 && state.view) set({ view: { ...state.view, state: confirmed } });
      return r;
    } finally {
      set({ launching: false });
    }
  },

  /** Launch the shown playset. */
  async launch(extra: string[] = [], args?: string): Promise<void> {
    const r = await actions.guardedLaunch((g, at) => api.launch(g, at, extra), args);
    if (!r) return;
    if (r.warning) toast(r.warning, "warn");
    toast("Launching...", "ok", 2500);
    await load(false);
  },

  /** Run any other api call with the store's error handling (toast on error). */
  run,
};

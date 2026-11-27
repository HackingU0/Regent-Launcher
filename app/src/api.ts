// Typed mirror of the Rust commands in src/app.rs (plan section 3 + deltas A). Hand-written: keep in step with the Rust DTOs.
// Pages never call `invoke` directly; they use `api.*` (or the store actions that wrap the mutating ones).
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// ---------------------------------------------------------------- errors
export type AppErrorKind = "stale" | "readOnly" | "failed";

export class AppError extends Error {
  kind: AppErrorKind;
  constructor(kind: AppErrorKind, message: string) {
    super(message);
    this.name = "AppError";
    this.kind = kind;
  }
}

function toAppError(e: unknown): AppError {
  if (e instanceof AppError) return e;
  if (e && typeof e === "object" && "message" in e) {
    const o = e as { kind?: string; message: unknown };
    const kind = o.kind === "stale" || o.kind === "readOnly" ? o.kind : "failed";
    return new AppError(kind, String(o.message));
  }
  return new AppError("failed", typeof e === "string" ? e : String(e));
}

export const inTauri = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!inTauri()) throw new AppError("failed", "Not running inside the Regent app");
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw toAppError(e);
  }
}

// ---------------------------------------------------------------- DTOs
export type GameId = string;
export type Channel = string;

export interface GameInfo {
  id: GameId;
  title: string;
  channel: Channel;
  /** Display string, e.g. "Corona Borealis v4.6.0 (3d1a)". */
  version?: string | null;
  rawVersion?: string | null;
}
export interface GamesResponse {
  games: GameInfo[];
  fixture: boolean;
  warning?: string | null;
}

export interface Entry {
  id: string;
  enabled: boolean;
}
export interface Playset {
  name: string;
  mods: Entry[];
  disabledDlcs: string[];
}
export interface GameState {
  active: number;
  playsets: Playset[];
  launchArgs: string;
}
/** The playset the UI believes is active; the backend answers `stale` if it no longer is. */
export interface At {
  index: number;
  name: string;
}

export interface ModInfo {
  id: string;
  name: string;
  version?: string | null;
  supportedVersion?: string | null;
  /** Made for another game version than the installed one. */
  mismatch: boolean;
  remoteId?: string | null;
  hasThumb: boolean;
  /** ISO date string, or unix seconds/millis; use `formatDate`. */
  updated?: string | number | null;
  /** Bytes. */
  size?: number | null;
  dependencies: string[];
}
export interface DlcInfo {
  id: string;
  name: string;
  /** expansion | story_pack | species_pack | content_pack | other... */
  category?: string | null;
  affectsChecksum: boolean;
  hasThumb: boolean;
}
/** What `thumb://<game>/art/*` can serve. `wallpaper`: custom, else the downloaded default, else the install background. */
export interface GameArt {
  logo: boolean;
  background: boolean;
  wallpaper: boolean;
  /** The user picked their own wallpaper for this game. */
  custom: boolean;
}
export interface GameView {
  game: GameId;
  art: GameArt;
  mods: ModInfo[];
  dlcs: DlcInfo[];
  state: GameState;
  readOnly: boolean;
  warning?: string | null;
  running: number;
  crashCount: number;
}

export interface Checksum {
  changing: string[];
  clean: string[];
  /** null = unknown. */
  achievements: boolean | null;
  /** Why `achievements` is null; null when it is known. */
  unknownReason: "noManifest" | "incomplete" | null;
}
export interface ConflictPairSummary {
  winner: string;
  loser: string;
  files: number;
}
export interface Summary {
  hasManifest: boolean;
  pairs: ConflictPairSummary[];
  conflictsByMod: Record<string, number>;
  suggest: { order: Entry[]; reasons: string[]; changed: boolean };
  checksum: Checksum;
  /** Bytes per mod id. */
  sizes: Record<string, number>;
}

export type FieldValue = string | number | boolean | null;
export interface SettingsField {
  id: string;
  label: string;
  /** toggle | slider | int | resolution | mode | language */
  kind: string;
  min?: number;
  max?: number;
  step?: number;
  options?: { value: string; label: string }[];
  value: string;
  note?: string | null;
}
export interface BackupInfo {
  id: number;
  /** Unix milliseconds. */
  at: number;
  files: string[];
}
export interface SettingsView {
  rev: string;
  fields: SettingsField[];
  raw: { game?: string | null; pdx?: string | null };
  paths: { game: string; pdx: string };
  backups: BackupInfo[];
}
export interface SettingsEdit {
  field: string;
  value: FieldValue;
}
export interface RawEdit {
  file: "game" | "pdx";
  text: string;
}

export type Binding =
  | { kind: "unknown" }
  | { kind: "playset"; name: string }
  | { kind: "custom"; count: number };
export interface SaveRow {
  rel: string;
  title: string;
  date?: string | null;
  version?: string | null;
  mtime: number;
  binding: Binding;
  plan: { kind: string; [k: string]: unknown };
  canContinue: boolean;
}
export interface SavesList {
  rows: SaveRow[];
  warning?: string | null;
}
export type SavesPlay =
  | { kind: "launched"; state: GameState; using: string; manualLoad: boolean; pid: number; warning?: string | null }
  | { kind: "needsConfirm"; reason: string; missing: string[]; token: string };

export interface CrashList {
  crashes: string[];
  hasLive: boolean;
}
export interface CrashRank {
  id: string;
  name: string;
  only: number;
  onlyRecent: number;
  shared: number;
  sharedRecent: number;
}
export interface CrashReport {
  live: boolean;
  appVersion?: string | null;
  signal?: string | null;
  checksum?: string | null;
  total: number;
  named: number;
  buckets: { only: number; shared: number; vanilla: number; none: number; inactive: number };
  missing: string[];
  crashMods: number;
  ranked: CrashRank[];
  lines: string[];
}

export interface ShareExport {
  code: string;
  json: string;
  fingerprint: string;
}
export interface ShareMissing {
  name: string;
  steam?: string | null;
  url?: string | null;
}
export interface ShareDiff {
  missing: string[];
  extra: string[];
  order: boolean;
  /** [mod, theirs, ours] */
  version: [string, string, string][];
  content: string[];
  /** [theirs, ours] */
  gameVersion?: [string, string] | null;
}
export interface ShareInspect {
  name: string;
  game: string;
  gameVersion?: string | null;
  modCount: number;
  missing: ShareMissing[];
  diff: ShareDiff;
  lines: string[];
  equal: boolean;
  theirs: unknown;
  ours: unknown;
}
export interface ShareImport {
  state: GameState;
  name: string;
  missing: ShareMissing[];
}

export interface LaunchInfo {
  pid: number;
  warning?: string | null;
}
export type OpenTarget =
  | { kind: "workshop"; id: string }
  | { kind: "crashDir"; game: GameId; name?: string }
  | { kind: "backups"; game: GameId };

export interface RefreshPayload {
  /** art: a default wallpaper finished downloading. thumbs: workshop previews finished downloading. */
  why: "focus" | "exit" | "art" | "thumbs";
  game?: GameId;
  warning?: string | null;
}

// ---------------------------------------------------------------- commands
export const api = {
  games: () => call<GamesResponse>("games"),
  load: (game: GameId, rescan: boolean) => call<GameView>("load", { game, rescan }),
  summary: (game: GameId) => call<Summary>("summary", { game }),
  conflictFiles: (game: GameId, winner: string, loser: string) =>
    call<string[]>("conflict_files", { game, winner, loser }),

  setModsEnabled: (game: GameId, at: At, ids: string[], on: boolean) =>
    call<GameState>("set_mods_enabled", { game, at, ids, on }),
  moveMod: (game: GameId, at: At, id: string, beforeId?: string | null) =>
    call<GameState>("move_mod", { game, at, id, beforeId: beforeId ?? null }),
  applyOrder: (game: GameId, at: At) => call<GameState>("apply_order", { game, at }),
  setDlcsEnabled: (game: GameId, at: At, ids: string[], on: boolean) =>
    call<GameState>("set_dlcs_enabled", { game, at, ids, on }),

  playsetNew: (game: GameId, name: string) => call<GameState>("playset_new", { game, name }),
  playsetCopy: (game: GameId, at: At, name?: string) => call<GameState>("playset_copy", { game, at, name: name ?? null }),
  playsetRename: (game: GameId, at: At, name: string) => call<GameState>("playset_rename", { game, at, name }),
  playsetUse: (game: GameId, index: number, name: string) => call<GameState>("playset_use", { game, index, name }),
  setLaunchArgs: (game: GameId, args: string) => call<GameState>("set_launch_args", { game, args }),

  launch: (game: GameId, at: At, extra: string[] = []) => call<LaunchInfo>("launch", { game, at, extra }),

  settingsGet: (game: GameId) => call<SettingsView>("settings_get", { game }),
  settingsApply: (game: GameId, rev: string, edits: SettingsEdit[], raw: RawEdit[]) =>
    call<{ view: SettingsView; backupDir?: string | null }>("settings_apply", { game, rev, edits, raw }),
  settingsRollback: (game: GameId, id: number) => call<SettingsView>("settings_rollback", { game, id }),

  savesList: (game: GameId) => call<SavesList>("saves_list", { game }),
  savesBind: (game: GameId, at: At, rel: string) => call<SavesList>("saves_bind", { game, at, rel }),
  /** `expect` = the opaque `token` of the needsConfirm the user accepted; the backend re-asks (new token) if the plan changed, or answers `stale` if it became safe. */
  savesPlay: (game: GameId, at: At, rel: string, force: boolean, expect?: string) =>
    call<SavesPlay>("saves_play", { game, at, rel, force, expect: expect ?? null }),

  crashList: (game: GameId) => call<CrashList>("crash_list", { game }),
  /** `name` undefined = the live log. */
  crashDiagnose: (game: GameId, name?: string) => call<CrashReport>("crash_diagnose", { game, name: name ?? null }),

  shareExport: (game: GameId, at: At) => call<ShareExport>("share_export", { game, at }),
  shareInspect: (game: GameId, text: string) => call<ShareInspect>("share_inspect", { game, text }),
  shareImport: (game: GameId, text: string) => call<ShareImport>("share_import", { game, text }),

  open: (target: OpenTarget) => call<void>("open", { target }),

  /** https image link, or a wallhaven page link / id. */
  wallpaperSetUrl: (game: GameId, url: string) => call<GameArt>("wallpaper_set_url", { game, url }),
  /** Opens the native file picker; null when cancelled. */
  wallpaperPickFile: (game: GameId) => call<GameArt | null>("wallpaper_pick_file", { game }),
  wallpaperReset: (game: GameId) => call<GameArt>("wallpaper_reset", { game }),
};

/** Subscribe to `regent:refresh`. Returns an unsubscribe function (no-op outside Tauri). */
export function onRefresh(cb: (p: RefreshPayload) => void, onError?: (e: unknown) => void): () => void {
  if (!inTauri()) return () => {};
  const un = listen<RefreshPayload>("regent:refresh", (e) => cb(e.payload));
  un.catch((e) => onError?.(e));
  return () => void un.then((f) => f(), () => {});
}

// ---------------------------------------------------------------- thumbs
export type ThumbKind = "mod" | "dlc" | "art";
/** `thumb://` URL for a mod/dlc thumbnail or the game's art. Only this helper builds them. */
export const thumbUrl = (game: GameId, kind: Exclude<ThumbKind, "art">, id: string) =>
  convertFileSrc(`${game}/${kind}/${id}`, "thumb");
/** Art is served `no-cache`; `tick` (store `artTick`) changes the URL so a new wallpaper replaces the old one on screen. */
export const artUrl = (game: GameId, which: "logo" | "background" | "wallpaper", tick = 0) =>
  `${convertFileSrc(`${game}/art/${which}`, "thumb")}?v=${tick}`;

// ---------------------------------------------------------------- small formatters
export function formatSize(bytes?: number | null): string {
  if (bytes == null) return "";
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(2)} GB`;
  if (bytes >= 1e6) return `${(bytes / 1e6).toFixed(bytes >= 1e8 ? 0 : 1)} MB`;
  if (bytes >= 1e3) return `${Math.round(bytes / 1e3)} KB`;
  return `${bytes} B`;
}

export function formatDate(v?: string | number | null): string {
  if (v == null || v === "") return "";
  let d: Date;
  if (typeof v === "number") d = new Date(v < 1e12 ? v * 1000 : v);
  else d = new Date(v);
  if (Number.isNaN(d.getTime())) return String(v);
  return d.toISOString().slice(0, 10);
}

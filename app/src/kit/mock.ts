// Mock data so the shell and kit render without the Tauri backend.
import type { GameView, ModInfo, DlcInfo, Summary } from "../api";
import { seedState } from "../store";

export const NAMES = [
  "UI Overhaul Dynamic", "UI Overhaul Dynamic - Extended Topbar for DLCs", "UI Overhaul Dynamic - Tiny Options",
  "Planetary Diversity", "Guilli's Planet Modifiers and Features", "Gigastructural Engineering & More", "Real Space",
  "Ancient Cache of Technologies", "更多科技树 More Tech Trees", "Extra Ship Components", "New Ambient Objects", "Dark Space",
];

export function mockMods(n = 12): ModInfo[] {
  return Array.from({ length: n }, (_, i) => ({
    id: `ugc_${1000 + i}.mod`,
    name: i < NAMES.length ? NAMES[i] : `Generated Mod ${String(i + 1).padStart(4, "0")}`,
    version: "1.0", supportedVersion: i % 5 === 1 ? "v4.4.*" : "v4.6.*", mismatch: i % 5 === 1, remoteId: String(1000 + i),
    hasThumb: false, updated: 1763900000 + i * 86400, size: 600000 + ((i * 7919) % 230) * 1000000 / 8, dependencies: [],
  }));
}
export const mockDlcs: DlcInfo[] = ["Apocalypse", "Federations", "Galactic Paragons", "BioGenesis", "Aquatics Species Pack", "Humanoids Species Pack"].map((name, i) => ({
  id: `dlc/${i}`, name, category: i < 4 ? "expansion" : "species_pack", affectsChecksum: false, hasThumb: false,
}));

export function seedMockShell(page: "home" | "mods" = "mods") {
  const mods = mockMods(12);
  const view: GameView = {
    game: "stellaris", art: { background: false, logo: false, wallpaper: false, custom: false }, mods, dlcs: mockDlcs, readOnly: false, warning: null, running: 0, crashCount: 2,
    state: { active: 0, launchArgs: "", playsets: [
      { name: "Default", mods: mods.map((m, i) => ({ id: m.id, enabled: i % 3 !== 2 })), disabledDlcs: [] },
      { name: "Hardcore Run", mods: mods.slice(0, 4).map((m) => ({ id: m.id, enabled: true })), disabledDlcs: [] },
    ] },
  };
  const summary: Summary = {
    hasManifest: true, pairs: Array.from({ length: 23 }, (_, i) => ({ winner: mods[0].id, loser: mods[1 + (i % 5)].id, files: i + 1 })),
    conflictsByMod: {}, suggest: { order: [], reasons: [], changed: false },
    checksum: { changing: [mods[3].id], clean: [], achievements: false, unknownReason: null }, sizes: {},
  };
  seedState({
    booted: true, fixture: true, game: "stellaris", page, view, summary,
    games: [
      { id: "stellaris", title: "Stellaris", channel: "Steam", version: "Corona Borealis v4.6.0 (3d1a)" },
      { id: "ck3", title: "Crusader Kings III", channel: "Steam", version: "1.21.0.3 (Florin)" },
    ],
  });
}

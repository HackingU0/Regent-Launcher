import { useEffect, type ComponentType } from "react";
import { Compass } from "lucide-react";
import { actions, useStore, type PageId } from "../store";
import { Banner, Button, EmptyState, SkeletonRows, ToastHost } from "../ui";
import HomePage from "../pages/home";
import ModsPage from "../pages/mods";
import SettingsPage from "../pages/settings";
import ConflictsPage from "../pages/conflicts";
import SavesPage from "../pages/saves";
import CrashPage from "../pages/crash";
import SharePage from "../pages/share";
import { Sidebar } from "./Sidebar";
import { LaunchBar } from "./LaunchBar";
import s from "./Shell.module.css";

export const PAGES: Record<PageId, ComponentType> = {
  home: HomePage, mods: ModsPage, settings: SettingsPage, conflicts: ConflictsPage, saves: SavesPage, crash: CrashPage, share: SharePage,
};

/**
 * App frame: sidebar + page + launch bar (hidden on Home). Pages render `<PageHeader/>` plus their body;
 * they fill a flex column (use `flex:1; min-height:0` on the scrolling part). Pages mount only once `view` is loaded.
 */
export function Shell() {
  const booted = useStore((st) => st.booted);
  const bootError = useStore((st) => st.bootError);
  const noGames = useStore((st) => st.booted && !st.bootError && st.games.length === 0);
  const page = useStore((st) => st.page);
  const hasView = useStore((st) => !!st.view);
  const readOnly = useStore((st) => !!st.view?.readOnly);
  const loading = useStore((st) => st.loading);
  const loadError = useStore((st) => st.loadError);
  useEffect(() => {
    const f = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.altKey) return;
      const i = Number(e.key) - 1;
      const ids = Object.keys(PAGES) as PageId[];
      if (i >= 0 && i < ids.length) { e.preventDefault(); actions.navigate(ids[i]); }
    };
    addEventListener("keydown", f);
    return () => removeEventListener("keydown", f);
  }, []);

  if (bootError || noGames) {
    return (
      <div className={s.full} data-tauri-drag-region="deep">
        <EmptyState
          icon={<Compass />} title={bootError ? "Regent could not start" : "No game found"}
          action={bootError ? <Button kind="primary" onClick={() => void actions.boot()}>Retry</Button> : undefined}
        >
          {bootError ?? "Regent looks for Stellaris, Europa Universalis IV and Hearts of Iron IV in your Steam, GOG and Xbox libraries. Install one, then reopen Regent."}
        </EmptyState>
        <ToastHost />
      </div>
    );
  }
  const Page = PAGES[page];
  return (
    <div className={s.app}>
      <Sidebar />
      <main className={s.main}>
        {readOnly && (
          <Banner tone="warn" className={s.banner}>
            Playsets could not be read from disk, so Regent will not change them. Editing and launching are disabled until the file is fixed.
          </Banner>
        )}
        {loadError && (
          <Banner tone="danger" className={s.banner} action="Retry" onAction={() => void actions.refresh(true)}>{loadError}</Banner>
        )}
        <div className={s.page}>
          {hasView ? <Page /> : !booted || loading ? <div className={s.skeleton}><SkeletonRows count={10} height={56} thumb /></div> : null}
        </div>
        {hasView && page !== "home" && <LaunchBar />}
      </main>
      <ToastHost />
    </div>
  );
}

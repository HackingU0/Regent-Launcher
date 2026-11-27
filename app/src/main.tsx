import { StrictMode, Suspense, lazy } from "react";
import { createRoot } from "react-dom/client";
import "./tokens.css";
import "./base.css";
import { actions } from "./store";
import { Shell } from "./shell/Shell";

document.documentElement.dataset.platform = /Mac/.test(navigator.platform) ? "mac" : "other";

// Dev only: `?kit` renders the component kit (no Tauri needed); `?kit=shell` renders the shell on mock data.
const kit = import.meta.env.DEV ? new URLSearchParams(location.search).get("kit") : null;
const Kit = import.meta.env.DEV ? lazy(() => import("./kit/Kit")) : null;

if (kit === null) void actions.boot();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {kit === null || !Kit ? <Shell /> : <Suspense fallback={null}><Kit mode={kit} /></Suspense>}
  </StrictMode>,
);

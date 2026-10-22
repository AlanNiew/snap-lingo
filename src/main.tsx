/** Settings window entry. */
import { createRoot } from "react-dom/client";

import SettingsApp from "./settings/App";
import { applyStoredTheme } from "./lib/theme";
import { api } from "./lib/api";
import "./styles.css";

// Paint the cached palette before React mounts so a dark theme never flashes.
applyStoredTheme();

const host = document.getElementById("root");
if (host) createRoot(host).render(<SettingsApp />);

// The window is created hidden: a cold WebView2 shows a blank white sheet for a
// second or two, and an undecorated white window looks like a hung app. Reveal
// it once the first frame is actually on screen (two frames: one to lay out,
// one to paint), with the Rust side keeping a watchdog in case this never runs.
requestAnimationFrame(() =>
  requestAnimationFrame(() => {
    void api.uiReady().catch(() => undefined);
  }),
);

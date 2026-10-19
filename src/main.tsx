/** Settings window entry. */
import { createRoot } from "react-dom/client";

import SettingsApp from "./settings/App";
import { applyStoredTheme } from "./lib/theme";
import "./styles.css";

// Paint the cached palette before React mounts so a dark theme never flashes.
applyStoredTheme();

const host = document.getElementById("root");
if (host) createRoot(host).render(<SettingsApp />);

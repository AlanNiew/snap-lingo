/** Settings window entry. */
import { createRoot } from "react-dom/client";

import SettingsApp from "./settings/App";
import "./styles.css";

const host = document.getElementById("root");
if (host) createRoot(host).render(<SettingsApp />);

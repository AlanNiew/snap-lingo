/**
 * The branded notice window (`toast.html`).
 *
 * Failures used to be announced with a Windows toast, which only carries the
 * SnapLingo identity in an installed build — a dev or portable run showed up as
 * “Windows PowerShell”. Drawing the notice ourselves keeps the name, the icon
 * and the current theme right, works in every build, and can point the user
 * straight at the log page.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";

import { api, onToast, type Notice } from "./lib/api";
import { t as translate, type StringKey } from "./lib/i18n";
import { applyStoredTheme, applyTheme } from "./lib/theme";
import type { AppConfig } from "./lib/types";
import "./styles.css";

/// How long a notice stays up. `pipeline::notify` also arms a watchdog, so a
/// page that never gets to run this timer cannot leave the card on screen.
const LIFETIME = 6000;

export function NoticeWindow() {
  const [notice, setNotice] = useState<Notice | null>(null);
  const [config, setConfig] = useState<AppConfig | null>(null);
  const timer = useRef<number | null>(null);

  const lang = config?.ui.language === "en" ? "en" : "zh";
  const tt = useCallback(
    (key: StringKey, vars?: Record<string, string | number>) => translate(lang, key, vars),
    [lang],
  );

  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        const loaded = await api.getConfig();
        if (alive) setConfig(loaded);
      } catch {
        /* defaults are fine */
      }
    })();

    const pending = [onToast(setNotice)];
    return () => {
      alive = false;
      for (const promise of pending) {
        void promise.then((off) => off()).catch(() => undefined);
      }
    };
  }, []);

  // Paint with the last known theme immediately, then follow the config.
  useEffect(() => {
    applyStoredTheme();
  }, []);
  useEffect(() => applyTheme(config?.ui.theme), [config?.ui.theme]);

  // Hide on its own after a few seconds; a new notice restarts the clock.
  useEffect(() => {
    if (!notice) return;
    if (timer.current !== null) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      void api.hideToast().catch(() => undefined);
    }, LIFETIME);
    return () => {
      if (timer.current !== null) window.clearTimeout(timer.current);
    };
  }, [notice]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      void api.hideToast().catch(() => undefined);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  if (!notice) return <div className="notice notice-idle" />;

  const failed = notice.kind === "error";

  return (
    <button
      type="button"
      className={`notice${failed ? " notice-error" : ""}`}
      onClick={() => void api.openLogs().catch(() => undefined)}
    >
      <span className="notice-mark" aria-hidden="true">
        {failed ? "!" : "S"}
      </span>
      <span className="notice-body">
        <span className="notice-title">
          SnapLingo · {failed ? tt("notifyError") : tt("notifyNotice")}
        </span>
        <span className="notice-text">{notice.message}</span>
        <span className="notice-hint">{tt("notifyHint")}</span>
      </span>
    </button>
  );
}

createRoot(document.getElementById("root")!).render(<NoticeWindow />);

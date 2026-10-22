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
import type { AppConfig } from "./lib/types";
import "./styles.css";

/**
 * The theme helpers live in `lib/theme` (theme branch). The notice keeps its own
 * tiny copy so every branch builds; the stored value is passed straight through
 * to `data-theme`, so palettes added later (midnight, paper, …) still apply.
 */
const THEME_STORE_KEY = "snaplingo.theme";

function resolveTheme(theme: string | null | undefined): string {
  const wanted = theme && theme.trim() !== "" ? theme.trim() : "system";
  if (wanted !== "system") return wanted;
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function applyTheme(theme: string | null | undefined): () => void {
  const wanted = theme && theme.trim() !== "" ? theme.trim() : "system";
  const paint = () => {
    document.documentElement.dataset.theme = resolveTheme(wanted);
  };
  paint();
  try {
    localStorage.setItem(THEME_STORE_KEY, wanted);
  } catch {
    /* storage can be unavailable; the palette still applies for this session */
  }
  if (wanted !== "system") return () => undefined;
  const query = window.matchMedia("(prefers-color-scheme: dark)");
  query.addEventListener("change", paint);
  return () => query.removeEventListener("change", paint);
}

function applyStoredTheme(): void {
  try {
    const stored = localStorage.getItem(THEME_STORE_KEY);
    if (stored) document.documentElement.dataset.theme = resolveTheme(stored);
  } catch {
    /* nothing cached yet */
  }
}

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
      // The window is often created *by* the notification, so the event above
      // was emitted before this page existed. Pull whatever should be showing.
      try {
        const pending = await api.currentToast();
        if (alive && pending) setNotice((current) => current ?? pending);
      } catch {
        /* nothing pending */
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
  // Update notices linger longer: they are only useful if there is time to click.
  useEffect(() => {
    if (!notice) return;
    if (timer.current !== null) window.clearTimeout(timer.current);
    const lifetime = notice.kind === "update" ? 14000 : LIFETIME;
    timer.current = window.setTimeout(() => {
      void api.hideToast().catch(() => undefined);
    }, lifetime);
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
  const update = notice.kind === "update";
  const hint = update ? tt("notifyUpdateHint") : tt("notifyHint");

  return (
    <button
      type="button"
      className={`notice${failed ? " notice-error" : ""}${update ? " notice-update" : ""}`}
      onClick={() =>
        void (update ? api.openUpdate() : api.openLogs()).catch(() => undefined)
      }
    >
      <span className="notice-mark" aria-hidden="true">
        {failed ? "!" : update ? "↑" : "S"}
      </span>
      <span className="notice-body">
        <span className="notice-title">
          SnapLingo · {failed ? tt("notifyError") : update ? tt("notifyUpdate") : tt("notifyNotice")}
        </span>
        <span className="notice-text">{notice.message}</span>
        <span className="notice-hint">{hint}</span>
      </span>
    </button>
  );
}

createRoot(document.getElementById("root")!).render(<NoticeWindow />);

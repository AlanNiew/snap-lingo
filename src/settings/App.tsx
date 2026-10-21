/** Settings shell: sidebar, the active page, toasts, and all the wiring. */
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";

import {
  api,
  onConfigChanged,
  onHotkeyError,
  onNavigate,
  type MonitorInfo,
  type OcrLanguage,
} from "../lib/api";
import { setLang, t as translate, tr, type Lang, type StringKey } from "../lib/i18n";
import type { AppConfig, HistoryItem } from "../lib/types";
import { cx, debounce } from "../lib/util";
import { applyTheme } from "../lib/theme";
import { LangCtx, type TFn } from "./context";
import {
  AboutSection,
  AppearanceSection,
  CaptureSection,
  GeneralSection,
  HistorySection,
  LogsSection,
  OcrSection,
  UpdateSection,
  TranslateSection,
  type PatchFn,
} from "./sections";

type TabId =
  | "general"
  | "ocr"
  | "translate"
  | "capture"
  | "appearance"
  | "history"
  | "logs"
  | "update"
  | "about";

const NAV: Array<{ id: TabId; key: StringKey }> = [
  { id: "general", key: "tabGeneral" },
  { id: "ocr", key: "tabOcr" },
  { id: "translate", key: "tabTranslate" },
  { id: "capture", key: "tabCapture" },
  { id: "appearance", key: "tabAppearance" },
  { id: "history", key: "tabHistory" },
  { id: "logs", key: "tabLogs" },
  { id: "update", key: "tabUpdate" },
  { id: "about", key: "tabAbout" },
];

/* Line icons for the sidebar. Stroke-only, 24x24 grid, so they inherit the
   current colour and stay legible on every palette. */
const NAV_PATHS: Record<TabId, string> = {
  general: "M5 7h14M5 12h14M5 17h14",
  ocr: "M4 8V5.5A1.5 1.5 0 0 1 5.5 4H8M16 4h2.5A1.5 1.5 0 0 1 20 5.5V8M20 16v2.5A1.5 1.5 0 0 1 18.5 20H16M8 20H5.5A1.5 1.5 0 0 1 4 18.5V16M8 12h8",
  translate:
    "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18ZM3 12h18M12 3c2.4 2.4 2.4 15.6 0 18M12 3C9.6 5.4 9.6 18.6 12 21",
  capture:
    "M4 8.5A1.5 1.5 0 0 1 5.5 7h1.2l1-1.6A1 1 0 0 1 8.5 5h7a1 1 0 0 1 .8.4L17.3 7h1.2A1.5 1.5 0 0 1 20 8.5v9A1.5 1.5 0 0 1 18.5 19h-13A1.5 1.5 0 0 1 4 17.5ZM12 16.5a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7Z",
  appearance:
    "M12 3a9 9 0 0 0 0 18h1.5a1.8 1.8 0 0 0 1.3-3l-.6-.7A1.5 1.5 0 0 1 15.3 15H17a4 4 0 0 0 4-4 8 8 0 0 0-9-8ZM8 9.5h.01M11 7.5h.01M14.5 8.5h.01M8.5 13h.01",
  history: "M12 7v5l3 2M4 12a8 8 0 1 0 2.3-5.6M4 4v4h4",
  logs: "M7 3h7l4 4v14a1 1 0 0 1-1 1H7a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1ZM14 3v5h4M9 13h6M9 17h4",
  update: "M12 16V4M7.5 8.5 12 4l4.5 4.5M4 16v2.5A1.5 1.5 0 0 0 5.5 20h13a1.5 1.5 0 0 0 1.5-1.5V16",
  about: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18ZM12 8h.01M11 12h1v5h1",
};

function NavIcon({ id }: { id: TabId }) {
  return (
    <svg
      className="nav-icon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d={NAV_PATHS[id]} />
    </svg>
  );
}

interface Toast {
  id: number;
  text: string;
  bad: boolean;
}

export default function SettingsApp() {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [tab, setTab] = useState<TabId>("general");
  const [monitors, setMonitors] = useState<MonitorInfo[]>([]);
  const [languages, setLanguages] = useState<OcrLanguage[]>([]);
  const [history, setHistory] = useState<HistoryItem[]>([]);
  const [hotkeyError, setHotkeyError] = useState<string | null>(null);
  const [toasts, setToasts] = useState<Toast[]>([]);

  // The newest config lives in a ref so `patch` never reads stale state and the
  // debounced save always writes the last edit, not the first.
  const latest = useRef<AppConfig | null>(null);
  const toastSeq = useRef(0);

  const lang: Lang = config?.ui.language === "en" ? "en" : "zh";

  useEffect(() => setLang(lang), [lang]);

  const tFn = useCallback<TFn>((key, vars) => translate(lang, key, vars), [lang]);
  const ctx = useMemo(() => ({ lang, t: tFn }), [lang, tFn]);

  const notify = useCallback((text: string, bad = false) => {
    const id = ++toastSeq.current;
    setToasts((list) => [...list, { id, text, bad }]);
    window.setTimeout(
      () => setToasts((list) => list.filter((toast) => toast.id !== id)),
      bad ? 4200 : 1900,
    );
  }, []);

  const persist = useCallback(
    async (next: AppConfig) => {
      try {
        await api.saveConfig(next);
      } catch (error) {
        notify(tr("saveFailed", { message: String(error) }), true);
      }
    },
    [notify],
  );

  const saveSoon = useMemo(() => debounce((next: AppConfig) => void persist(next), 240), [persist]);

  const patch = useCallback<PatchFn>(
    (recipe, immediate = false) => {
      const prev = latest.current;
      if (!prev) return;
      const next = recipe(prev);
      latest.current = next;
      setConfig(next);
      if (immediate) void persist(next);
      else saveSoon(next);
    },
    [persist, saveSoon],
  );

  const reloadHistory = useCallback(() => {
    void api
      .history()
      .then(setHistory)
      .catch(() => undefined);
  }, []);

  /* --------------------------------------------------------------- load + wire */

  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        const loaded = await api.getConfig();
        if (!alive) return;
        latest.current = loaded;
        setConfig(loaded);
      } catch (error) {
        if (alive) notify(String(error), true);
      }
    })();
    void api.monitors().then(setMonitors).catch(() => undefined);
    void api.ocrLanguages().then(setLanguages).catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [notify]);

  useEffect(() => {
    const pending = [
      onConfigChanged((next) => {
        latest.current = next;
        setConfig(next);
      }),
      onHotkeyError(setHotkeyError),
      onNavigate((target) => {
        if (NAV.some((item) => item.id === target)) setTab(target as TabId);
      }),
    ];
    return () => {
      for (const promise of pending) {
        void promise.then((off) => off()).catch(() => undefined);
      }
    };
  }, []);

  useEffect(() => {
    if (tab === "history") reloadHistory();
  }, [tab, reloadHistory]);

  useEffect(() => {
    setHotkeyError(null);
  }, [config?.shortcut]);

  /* ------------------------------------------------------------------- theme */

  useEffect(() => applyTheme(config?.ui.theme), [config?.ui.theme]);

  const reset = async () => {
    if (!window.confirm(tFn("resetConfirm"))) return;
    try {
      const next = await api.resetConfig();
      latest.current = next;
      setConfig(next);
      setHotkeyError(null);
      notify(tFn("resetDone"));
    } catch (error) {
      notify(String(error), true);
    }
  };

  /* ------------------------------------------------------------------- pages */

  if (!config) {
    return (
      <div className="settings">
        <main className="content">
          <div className="empty">{translate(lang, "loading")}</div>
        </main>
      </div>
    );
  }

  const base = { config, patch, notify };
  let page: ReactNode;
  switch (tab) {
    case "ocr":
      page = <OcrSection {...base} languages={languages} />;
      break;
    case "translate":
      page = <TranslateSection {...base} />;
      break;
    case "capture":
      page = <CaptureSection {...base} />;
      break;
    case "appearance":
      page = <AppearanceSection {...base} />;
      break;
    case "history":
      page = <HistorySection {...base} history={history} reloadHistory={reloadHistory} />;
      break;
    case "logs":
      page = <LogsSection {...base} />;
      break;
    case "update":
      page = <UpdateSection {...base} />;
      break;
    case "about":
      page = <AboutSection config={config} monitors={monitors} />;
      break;
    default:
      page = <GeneralSection {...base} hotkeyError={hotkeyError} monitors={monitors} />;
  }

  return (
    <LangCtx.Provider value={ctx}>
      <div className="settings">
        <aside className="sidebar">
          <div className="brand">
            <span className="brand-mark" />
            <span className="brand-name">{tFn("appName")}</span>
          </div>
          {NAV.map((item) => (
            <button
              key={item.id}
              type="button"
              className="nav-item"
              aria-current={tab === item.id ? "true" : undefined}
              onClick={() => setTab(item.id)}
            >
              <NavIcon id={item.id} />
              <span className="nav-label">{tFn(item.key)}</span>
              {item.id === "history" && history.length > 0 ? (
                <span className="nav-count">{history.length}</span>
              ) : null}
            </button>
          ))}
          <div className="nav-footer">
            <button type="button" className="btn btn-sm btn-block" onClick={reset}>
              {tFn("resetDefaults")}
            </button>
            <span>
              {tFn("aboutVersion")} {__APP_VERSION__}
            </span>
          </div>
        </aside>

        <main className="content">{page}</main>
      </div>

      <div className="toasts">
        {toasts.map((toast) => (
          <div key={toast.id} className={cx("toast", toast.bad && "toast-bad")}>
            {toast.text}
          </div>
        ))}
      </div>
    </LangCtx.Provider>
  );
}

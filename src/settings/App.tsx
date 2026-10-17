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
import { LangCtx, type TFn } from "./context";
import {
  AboutSection,
  AppearanceSection,
  CaptureSection,
  GeneralSection,
  HistorySection,
  OcrSection,
  TranslateSection,
  type PatchFn,
} from "./sections";

type TabId = "general" | "ocr" | "translate" | "capture" | "appearance" | "history" | "about";

const NAV: Array<{ id: TabId; key: StringKey }> = [
  { id: "general", key: "tabGeneral" },
  { id: "ocr", key: "tabOcr" },
  { id: "translate", key: "tabTranslate" },
  { id: "capture", key: "tabCapture" },
  { id: "appearance", key: "tabAppearance" },
  { id: "history", key: "tabHistory" },
  { id: "about", key: "tabAbout" },
];

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

  useEffect(() => {
    const theme = config?.ui.theme ?? "system";
    const query = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      const dark = theme === "dark" || (theme === "system" && query.matches);
      document.documentElement.dataset.theme = dark ? "dark" : "light";
    };
    apply();
    if (theme !== "system") return;
    query.addEventListener("change", apply);
    return () => query.removeEventListener("change", apply);
  }, [config?.ui.theme]);

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
              <span>{tFn(item.key)}</span>
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

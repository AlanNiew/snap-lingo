/**
 * The result window: opens the moment a capture is confirmed and follows the
 * pipeline (`pipeline:stage`) until the translation lands.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";

import { api, onConfigChanged, onResult, onStage } from "./lib/api";
import { t as translate, type StringKey } from "./lib/i18n";
import {
  PROVIDER_KEYS,
  TARGET_LANGS,
  targetOptionLabel,
  withTarget,
} from "./lib/labels";
import type { AppConfig, ResultPayload, Stage, StageEvent, TranslateProvider } from "./lib/types";
import { copyText, formatDuration } from "./lib/util";
import { applyStoredTheme, applyTheme } from "./lib/theme";
import { Select } from "./settings/fields";
import "./styles.css";

const BUSY: Stage[] = ["capturing", "ocr", "translate"];
type ResultView = "both" | "source" | "translation";

function statusKey(stage: Stage): StringKey {
  switch (stage) {
    case "capturing":
      return "overlayBusy";
    case "ocr":
      return "resultOcr";
    case "translate":
      return "resultTranslate";
    case "error":
      return "resultError";
    case "done":
      return "resultDone";
    default:
      return "appName";
  }
}

export function ResultWindow() {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [payload, setPayload] = useState<ResultPayload | null>(null);
  const [stage, setStage] = useState<StageEvent | null>(null);
  const [copied, setCopied] = useState(false);
  /** Temporary override of the configured view, per result window. */
  const [localView, setLocalView] = useState<ResultView | null>(null);
  /** Temporary override of the configured "show the picture" switch. */
  const [localShot, setLocalShot] = useState<boolean | null>(null);
  /** Click the picture to inspect it full size. */
  const [zoom, setZoom] = useState(false);

  const root = useRef<HTMLDivElement>(null);
  const autoCopied = useRef<string | null>(null);

  const lang = config?.ui.language === "en" ? "en" : "zh";
  const tt = useCallback(
    (key: StringKey, vars?: Record<string, string | number>) => translate(lang, key, vars),
    [lang],
  );

  /* -- derived state (declared before the hooks that depend on it) ---------- */

  const view: ResultView = localView ?? config?.ui.resultView ?? "both";
  const sourceText = payload?.sourceText ?? "";
  const targetText = payload?.error ? "" : payload?.translatedText?.trim() ?? "";
  /** Copy what is on screen: the source in source-only mode, otherwise the translation. */
  const copyBody = view === "source" ? sourceText : targetText || sourceText;

  /* -------------------------------------------------------------------- boot */

  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        const loaded = await api.getConfig();
        if (alive) setConfig(loaded);
      } catch {
        /* defaults are fine */
      }
      try {
        const last = await api.getLastResult();
        if (alive && last) setPayload(last);
      } catch {
        /* nothing to restore */
      }
    })();

    const pending = [onResult(setPayload), onStage(setStage), onConfigChanged(setConfig)];
    return () => {
      alive = false;
      for (const promise of pending) {
        void promise.then((off) => off()).catch(() => undefined);
      }
    };
  }, []);

  /* ------------------------------------------------------------------- theme */

  useEffect(() => applyTheme(config?.ui.theme), [config?.ui.theme]);

  /* ------------------------------------------------ optional fit-to-content */

  // Off by default: the window is centred and sized by Rust. When the user asks
  // for it, grow/shrink the height to the content — but only on real content
  // changes, only when a reading repeats, and never more than a handful of times
  // per result, so the window can't flap or fight a manual drag.
  useEffect(() => {
    const working = stage !== null && BUSY.includes(stage.stage);
    if (!config?.ui.autoFit || !payload || working) return;
    const node = root.current;
    if (!node) return;

    const MAX_FITS = 6;
    let lastSent = -1;
    let stable: number | null = null;
    let sent = 0;
    let raf = 0;
    let stopped = false;

    function schedule() {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(measure);
    }

    function measure() {
      raf = 0;
      if (stopped) return;
      const el = root.current;
      if (!el) return;
      const head = el.querySelector<HTMLElement>(".result-head");
      const body = el.querySelector<HTMLElement>(".result-body");
      const content = el.querySelector<HTMLElement>(".result-content");
      const foot = el.querySelector<HTMLElement>(".result-foot");
      // Measure the content box, never the scroll container (its scrollHeight is
      // clamped to the current window height, which breaks shrinking).
      const style = body ? getComputedStyle(body) : null;
      const padY = style
        ? parseFloat(style.paddingTop) + parseFloat(style.paddingBottom)
        : 0;
      const total =
        (head?.offsetHeight ?? 0) +
        (content?.offsetHeight ?? 0) +
        padY +
        (foot?.offsetHeight ?? 0) +
        4;
      if (total < 120) return;

      if (stable !== total) {
        stable = total;
        schedule();
        return;
      }
      if (sent >= MAX_FITS || Math.abs(total - lastSent) < 4) return;
      lastSent = total;
      sent += 1;
      // CSS pixels; Rust clamps and re-centres the window.
      void api.fitResultWindow(window.innerWidth, total).catch(() => undefined);
    }

    schedule();
    return () => {
      stopped = true;
      cancelAnimationFrame(raf);
    };
  }, [payload, stage, view, config?.ui.autoFit, config?.ui.resultFontSize]);

  /* ------------------------------------------------------------- interactions */

  const copy = async () => {
    const text = copyBody || payload?.sourceText || "";
    const ok = await copyText(text);
    if (ok) {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    }
  };

  /**
   * Quick target-language switch. The choice is written back to the config (so
   * the next capture uses it too) and the current result is translated again
   * from the cached picture — no trip to the settings page.
   */
  const switchTarget = async (targetLang: string) => {
    if (!config || targetLang === config.translate.targetLang) return;
    const next: AppConfig = {
      ...config,
      translate: { ...config.translate, targetLang },
    };
    setConfig(next);
    try {
      await api.saveConfig(next);
      await api.retryTranslation();
    } catch {
      /* the pipeline reports the failure in the result card */
    }
  };

  useEffect(() => {
    if (!config?.ui.autoCopy || !payload || payload.error) return;
    if (autoCopied.current === payload.id) return;
    const text = payload.translatedText?.trim() || payload.sourceText;
    if (!text) return;
    autoCopied.current = payload.id;
    void copyText(text).then((ok) => {
      if (!ok) return;
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    });
  }, [config?.ui.autoCopy, payload]);

  useEffect(() => {
    if (!config?.ui.closeOnBlur) return;
    const hide = () => void api.hideResult().catch(() => undefined);
    window.addEventListener("blur", hide);
    return () => window.removeEventListener("blur", hide);
  }, [config?.ui.closeOnBlur]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      // Esc first leaves the enlarged picture, and only then the window: it is
      // the same "back out one step" the user expects from the overlay.
      setZoom((wasZoomed) => {
        if (wasZoomed) return false;
        void api.hideResult().catch(() => undefined);
        return false;
      });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // A new result starts unzoomed.
  useEffect(() => setZoom(false), [payload?.id]);

  /* ------------------------------------------------------------------ render */

  const busy = stage !== null && BUSY.includes(stage.stage);
  const stageText = config
    ? stage
      ? tt(statusKey(stage.stage))
      : tt("loading")
    : tt("loading");
  const fontSize = config?.ui.resultFontSize ?? 14;
  // Retry stays available for every finished run — a failed OCR, a failed
  // translation, or simply wanting another attempt with a different model.
  const canRetry = Boolean(payload) && !busy;
  const done = Boolean(payload && !payload.error && !busy);
  const ocrLabel =
    payload?.ocrProvider === "winocr"
      ? tt("ocrWin")
      : payload?.ocrProvider === "vision"
        ? tt("ocrVision")
        : payload?.ocrProvider === "image"
          ? tt("badgeImageTranslate")
          : (payload?.ocrProvider ?? "");
  const providerLabel =
    payload && payload.translateProvider !== "none"
      ? tt(PROVIDER_KEYS[payload.translateProvider as TranslateProvider] ?? "translateOpenai")
      : "";

  const viewOptions: Array<{ id: ResultView; key: StringKey }> = [
    { id: "source", key: "viewSource" },
    { id: "translation", key: "viewTranslation" },
    { id: "both", key: "viewBoth" },
  ];
  // The picture is there to check the translation against the original, so it
  // is pointless in source-only mode (the original text *is* the content then).
  const shotOpen = localShot ?? config?.ui.resultShowImage ?? true;
  const showShot = Boolean(payload?.preview) && view !== "source" && shotOpen;

  return (
    <div className="result" ref={root}>
      <header className="result-head" data-tauri-drag-region="deep">
        <span className="result-title">
          {tt("translation")} ·{" "}
          {payload?.targetLang
            ? targetOptionLabel(payload.targetLang, tt("targetAuto"))
            : tt("empty")}
        </span>
        <span className="result-status">
          {busy ? <span className="spinner" /> : null}
          <span>{busy ? stageText : payload?.error ? tt("resultError") : tt("resultDone")}</span>
          {done ? <span className="nav-count">{formatDuration(payload?.elapsedMs ?? 0)}</span> : null}
        </span>
        <button
          type="button"
          className="result-close"
          title={tt("close")}
          aria-label={tt("close")}
          onClick={() => void api.hideResult().catch(() => undefined)}
        >
          <svg viewBox="0 0 16 16" width="11" height="11" aria-hidden="true">
            <path
              d="M3.2 3.2l9.6 9.6M12.8 3.2l-9.6 9.6"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.7"
              strokeLinecap="round"
            />
          </svg>
        </button>
      </header>

      <div className="result-body">
        <div className="result-content">
          {payload?.error && !busy ? <div className="result-error">{payload.error}</div> : null}

        {busy ? (
          <div className="result-skeleton">
            <span style={{ width: "62%" }} />
            <span style={{ width: "88%" }} />
            <span style={{ width: "74%" }} />
          </div>
        ) : null}

        {done ? (
          <>
            {showShot ? (
              <figure className="result-shot" data-zoom={zoom} onClick={() => setZoom((was) => !was)}>
                <img src={payload?.preview ?? undefined} alt="" />
              </figure>
            ) : null}

            {view !== "source" ? (
              <section className="result-pane">
                {view === "both" ? <div className="result-pane-label">{tt("viewTranslation")}</div> : null}
                <div className="result-text" style={{ fontSize }}>
                  {targetText ? targetText : <span className="result-placeholder">{tt("noTranslation")}</span>}
                </div>
              </section>
            ) : null}

            {view === "both" && sourceText ? <div className="result-divider" /> : null}

            {view !== "translation" ? (
              <section className="result-pane">
                {view === "both" ? <div className="result-pane-label">{tt("viewSource")}</div> : null}
                <div className="result-src-text" style={view === "source" ? { fontSize } : undefined}>
                  {sourceText ? sourceText : <span className="result-placeholder">{tt("empty")}</span>}
                </div>
              </section>
            ) : null}

            <div className="result-meta">
              {ocrLabel ? <span className="badge">{ocrLabel}</span> : null}
              {providerLabel ? <span className="badge badge-accent">{providerLabel}</span> : null}
              {payload?.model ? <span className="badge">{payload.model}</span> : null}
            </div>
          </>
        ) : null}

        </div>
      </div>

      <footer className="result-foot">
          {config ? (
            <label className="result-lang" title={tt("targetLang")}>
              <span className="result-lang-label">{tt("targetLang")}</span>
              <Select
                value={config.translate.targetLang}
                onChange={(value) => void switchTarget(value)}
                options={withTarget(TARGET_LANGS, config.translate.targetLang).map((option) => ({
                  value: option.value,
                  label: targetOptionLabel(option.value, tt("targetAuto")),
                }))}
              />
            </label>
          ) : null}

          {payload && !payload.error ? (
            <div className="result-view" role="group" aria-label={tt("resultView")}>
              {viewOptions.map((option) => (
                <button
                  key={option.id}
                  type="button"
                  disabled={busy}
                  data-active={view === option.id}
                  onClick={() => setLocalView(option.id)}
                >
                  {tt(option.key)}
                </button>
              ))}
            </div>
          ) : null}

          <span className="spacer" />

          {payload?.preview ? (
            <button
              type="button"
              className="btn btn-sm btn-ghost"
              data-active={shotOpen && view !== "source"}
              title={tt("resultShotToggle")}
              onClick={() => setLocalShot(!shotOpen)}
            >
              {tt("resultShot")}
            </button>
          ) : null}

          {canRetry ? (
            <button
              className="btn btn-sm"
              disabled={busy}
              onClick={() => void api.retryTranslation().catch(() => undefined)}
            >
              {tt("retryTranslate")}
            </button>
          ) : null}
          {done ? (
            <button className="btn btn-primary btn-sm" onClick={copy}>
              {copied ? tt("copied") : tt("copy")}
            </button>
          ) : null}
          <button className="btn btn-sm btn-ghost" onClick={() => void api.hideResult()}>
            {tt("close")}
          </button>
      </footer>
    </div>
  );
}

// Paint the cached palette before React mounts: the config arrives over IPC a
// moment later, and a dark theme should not flash light first.
applyStoredTheme();

const host = document.getElementById("root");
if (host) createRoot(host).render(<ResultWindow />);
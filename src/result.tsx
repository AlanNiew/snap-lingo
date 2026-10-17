/**
 * The result window: opens the moment a capture is confirmed and follows the
 * pipeline (`pipeline:stage`) until the translation lands.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";

import { api, onConfigChanged, onResult, onStage } from "./lib/api";
import { t as translate, type StringKey } from "./lib/i18n";
import type { AppConfig, ResultPayload, Stage, StageEvent } from "./lib/types";
import { copyText, formatDuration } from "./lib/util";
import "./styles.css";

const BUSY: Stage[] = ["capturing", "ocr", "translate"];

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

  const root = useRef<HTMLDivElement>(null);
  const autoCopied = useRef<string | null>(null);

  const lang = config?.ui.language === "en" ? "en" : "zh";
  const tt = useCallback(
    (key: StringKey, vars?: Record<string, string | number>) => translate(lang, key, vars),
    [lang],
  );

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

  /* ------------------------------------------------------ size to the content */

  useEffect(() => {
    const node = root.current;
    if (!node) return;
    let last = 0;
    let raf = 0;

    const measure = () => {
      const head = node.querySelector<HTMLElement>(".result-head");
      const body = node.querySelector<HTMLElement>(".result-body");
      const foot = node.querySelector<HTMLElement>(".result-foot");
      const total =
        (head?.offsetHeight ?? 0) + (body?.scrollHeight ?? 0) + (foot?.offsetHeight ?? 0) + 2;
      if (total < 80 || Math.abs(total - last) < 3) return;
      last = total;
      // Logical pixels: Rust applies `LogicalSize`, so this is CSS pixels.
      void api.fitResultWindow(window.innerWidth, total).catch(() => undefined);
    };

    const schedule = () => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(measure);
    };

    schedule();
    const observer = new ResizeObserver(schedule);
    observer.observe(node);
    window.addEventListener("resize", schedule);
    return () => {
      cancelAnimationFrame(raf);
      observer.disconnect();
      window.removeEventListener("resize", schedule);
    };
  }, [payload, stage, config?.ui.resultFontSize, config?.ui.showOriginal]);

  /* ------------------------------------------------------------- interactions */

  const body = payload?.error
    ? ""
    : payload
      ? payload.translatedText?.trim() || (payload.translateProvider === "none" ? payload.sourceText : "")
      : "";

  const copy = async () => {
    const text = body || payload?.sourceText || "";
    const ok = await copyText(text);
    if (ok) {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
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
      if (event.key === "Escape") {
        event.preventDefault();
        void api.hideResult().catch(() => undefined);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  /* ------------------------------------------------------------------ render */

  const busy = stage !== null && BUSY.includes(stage.stage);
  const stageText = config
    ? stage
      ? tt(statusKey(stage.stage))
      : tt("loading")
    : tt("loading");
  const showOriginal = Boolean(config?.ui.showOriginal && payload?.sourceText);
  const fontSize = config?.ui.resultFontSize ?? 14;
  const canRetry = Boolean(payload && payload.translateProvider !== "none");

  return (
    <div className="result" ref={root}>
      <header className="result-head" data-tauri-drag-region>
        <span className="result-title">
          {tt("translation")} · {payload?.targetLang || tt("empty")}
        </span>
        <span className="result-status">
          {busy ? <span className="spinner" /> : null}
          <span>{busy ? stageText : payload?.error ? tt("resultError") : tt("resultDone")}</span>
          {payload && !busy && !payload.error ? (
            <span className="nav-count">{formatDuration(payload.elapsedMs)}</span>
          ) : null}
        </span>
      </header>

      <div className="result-body">
        {payload?.error ? <div className="result-error">{payload.error}</div> : null}

        {!payload && busy ? (
          <div className="result-skeleton">
            <span style={{ width: "62%" }} />
            <span style={{ width: "88%" }} />
            <span style={{ width: "74%" }} />
          </div>
        ) : null}

        {payload && !payload.error ? (
          <>
            {payload.preview ? <img className="result-preview" src={payload.preview} alt="" /> : null}
            <div className="result-text" style={{ fontSize }}>
              {body ? body : <span className="result-placeholder">{tt("noTranslation")}</span>}
            </div>
            {showOriginal ? (
              <details className="result-src">
                <summary>{tt("original")}</summary>
                <div className="result-src-text">{payload.sourceText}</div>
              </details>
            ) : null}
            <div className="result-meta">
              <span className="badge">{payload.ocrProvider}</span>
              {payload.translateProvider !== "none" ? (
                <span className="badge">{payload.translateProvider}</span>
              ) : null}
              {payload.model ? <span className="badge">{payload.model}</span> : null}
            </div>
          </>
        ) : null}
      </div>

      <footer className="result-foot">
        {payload && !payload.error ? (
          <button className="btn btn-primary btn-sm" onClick={copy}>
            {copied ? tt("copied") : tt("copy")}
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
        <span className="spacer" />
        <span className="result-status">
          <button className="btn btn-sm btn-ghost" onClick={() => void api.hideResult()}>
            {tt("close")}
          </button>
        </span>
      </footer>
    </div>
  );
}

const host = document.getElementById("root");
if (host) createRoot(host).render(<ResultWindow />);

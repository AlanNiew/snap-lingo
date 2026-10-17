/**
 * The per-monitor selection layer.
 *
 * One instance lives per monitor. Each one shows the frozen frame that Rust
 * captured before the layer appeared, so the desktop can never move underneath
 * the user while they drag — and the layer itself can never end up in the shot.
 */
import { useCallback, useEffect, useRef, useState, type MouseEvent as ReactMouseEvent } from "react";
import { createRoot } from "react-dom/client";

import { api, currentWindow, monitorIdFromLabel, onSession } from "./lib/api";
import { t as translate, type StringKey } from "./lib/i18n";
import type { SessionImage } from "./lib/types";
import "./styles.css";

/** Below this many physical pixels a selection is treated as a stray click. */
const MIN_PHYSICAL = 4;

interface Point {
  x: number;
  y: number;
}

interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
}

function boxBetween(a: Point, b: Point): Box {
  return {
    x: Math.min(a.x, b.x),
    y: Math.min(a.y, b.y),
    width: Math.abs(a.x - b.x),
    height: Math.abs(a.y - b.y),
  };
}

export function Overlay() {
  const monitorId = monitorIdFromLabel(currentWindow.label);
  const [lang, setLang] = useState<"zh" | "en">("zh");
  const [frame, setFrame] = useState<SessionImage | null>(null);
  const [anchor, setAnchor] = useState<Point | null>(null);
  const [pointer, setPointer] = useState<Point | null>(null);
  const [dragging, setDragging] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const shot = useRef<SessionImage | null>(null);

  const tt = useCallback(
    (key: StringKey, vars?: Record<string, string | number>) => translate(lang, key, vars),
    [lang],
  );

  const cancel = useCallback(() => {
    setDragging(false);
    void api.cancelCapture().catch(() => undefined);
  }, []);

  /* ----------------------------------------------------------- boot the layer */

  const reset = useCallback(() => {
    shot.current = null;
    setFrame(null);
    setAnchor(null);
    setPointer(null);
    setDragging(false);
    setBusy(false);
    setFailure(null);
  }, []);

  const load = useCallback(async () => {
    if (monitorId === null) return;
    const image = await api.getSessionImage(monitorId);
    if (!image) {
      setFailure("no frame");
      cancel();
      return;
    }
    shot.current = image;
    setFrame(image);
    // Rust keeps the window hidden until the frozen frame is painted, so the
    // user never sees a black flash before the desktop appears.
    await api.overlayReady(monitorId);
  }, [cancel, monitorId]);

  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        const config = await api.getConfig();
        if (alive) setLang(config.ui.language === "en" ? "en" : "zh");
      } catch {
        /* language is cosmetic here */
      }

      if (monitorId === null) {
        cancel();
        return;
      }

      try {
        await load();
      } catch (error) {
        if (alive) {
          setFailure(String(error));
          cancel();
        }
      }
    })();
    return () => {
      alive = false;
    };
  }, [cancel, load, monitorId]);

  // Escape hatch of last resort: if the frozen frame never arrives — a wedged
  // IPC call, a lost event — close the layer instead of leaving a black sheet
  // over the desktop with nothing the user can do.
  useEffect(() => {
    const timer = window.setTimeout(() => {
      if (!shot.current) {
        setFailure("timeout");
        cancel();
      }
    }, 10000);
    return () => window.clearTimeout(timer);
  }, [cancel]);

  // The window is reused for every capture on this monitor, so a new frozen
  // frame arrives as an event instead of a page load.
  useEffect(() => {
    if (monitorId === null) return;
    let alive = true;
    let off: (() => void) | null = null;
    void onSession((id) => {
      if (id !== monitorId) return;
      reset();
      void load().catch((error) => {
        if (!alive) return;
        setFailure(String(error));
        cancel();
      });
    }).then((unlisten) => {
      if (!alive) {
        unlisten();
        return;
      }
      off = unlisten;
    });
    return () => {
      alive = false;
      if (off) off();
    };
  }, [cancel, load, monitorId, reset]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        cancel();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [cancel]);

  /* ---------------------------------------------------------------- selection */

  const box = anchor && pointer ? boxBetween(anchor, pointer) : null;

  const onMouseDown = (event: ReactMouseEvent) => {
    if (busy || event.button !== 0 || !frame) return;
    setDragging(true);
    setAnchor({ x: event.clientX, y: event.clientY });
    setPointer({ x: event.clientX, y: event.clientY });
  };

  const onMouseMove = (event: ReactMouseEvent) => {
    if (!dragging) return;
    setPointer({ x: event.clientX, y: event.clientY });
  };

  const onMouseUp = () => {
    if (!dragging || !box) return;
    setDragging(false);

    const image = shot.current;
    if (!image || monitorId === null) {
      cancel();
      return;
    }

    // The frame is stretched to fill the viewport, so one CSS pixel is
    // `frame.width / innerWidth` physical pixels. Deriving it instead of
    // trusting the scale factor keeps the crop glued to what the user saw.
    const scale = image.width / Math.max(1, window.innerWidth);
    const rect = {
      monitorId,
      x: Math.round(box.x * scale),
      y: Math.round(box.y * scale),
      width: Math.round(box.width * scale),
      height: Math.round(box.height * scale),
    };

    if (rect.width < MIN_PHYSICAL || rect.height < MIN_PHYSICAL) {
      setAnchor(null);
      setPointer(null);
      cancel();
      return;
    }

    setBusy(true);
    void api.finishSelection(rect).catch((error) => {
      setBusy(false);
      setFailure(String(error));
      cancel();
    });
  };

  const tooSmall = box !== null && (box.width < 4 || box.height < 4);

  return (
    <div
      className="overlay"
      onMouseDown={onMouseDown}
      onMouseMove={onMouseMove}
      onMouseUp={onMouseUp}
      onContextMenu={(event) => {
        event.preventDefault();
        cancel();
      }}
    >
      {frame ? (
        <img className="overlay-shot" src={frame.dataUrl} alt="" draggable={false} />
      ) : null}

      {box ? (
        <div
          className="overlay-hole"
          style={{ left: box.x, top: box.y, width: box.width, height: box.height }}
        >
          <span className="overlay-size">
            {Math.round(box.width)} × {Math.round(box.height)}
          </span>
        </div>
      ) : busy || failure ? null : (
        <div className="overlay-scrim" />
      )}

      {dragging && pointer ? (
        <div className="overlay-guides">
          <span style={{ left: pointer.x, top: 0, width: 1, height: "100%" }} />
          <span style={{ left: 0, top: pointer.y, width: "100%", height: 1 }} />
        </div>
      ) : null}

      {dragging && pointer ? (
        <div className="overlay-cursor-hint" style={{ left: pointer.x, top: pointer.y }}>
          {tooSmall ? tt("overlayTooSmall") : tt("overlaySelecting")}
        </div>
      ) : null}

      {busy || failure ? (
        <div className="overlay-busy">
          <div className="overlay-busy-inner">
            {failure ? null : <span className="spinner" />}
            <span>{failure ? tt("resultError") : tt("overlayBusy")}</span>
          </div>
        </div>
      ) : !dragging ? (
        <div className="overlay-bar">
          <span>{tt("overlayHint")}</span>
          <kbd>Esc</kbd>
        </div>
      ) : null}
    </div>
  );
}

const host = document.getElementById("root");
if (host) createRoot(host).render(<Overlay />);

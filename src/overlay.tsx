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
  const shotRef = useRef<HTMLImageElement | null>(null);
  // Reveal happens once per frame: the first paint that can show the frozen
  // desktop is what ends the black sheet, so a slow image must not un-hide the
  // window early and a repeated event must not ask twice.
  const revealed = useRef(false);
  // Pointer moves are coalesced to one state update per animation frame. Each
  // move repaints a full-screen, colour-sampled layer, and the events arrive
  // faster than the compositor consumes them.
  const pending = useRef<Point | null>(null);
  const raf = useRef<number | null>(null);

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
    revealed.current = false;
    if (raf.current !== null) {
      cancelAnimationFrame(raf.current);
      raf.current = null;
    }
    pending.current = null;
    setFrame(null);
    setAnchor(null);
    setPointer(null);
    setDragging(false);
    setBusy(false);
    setFailure(null);
  }, []);

  const reveal = useCallback(() => {
    if (monitorId === null || revealed.current) return;
    revealed.current = true;
    // One paint later: `overlayReady` is what un-hides the window, and asking
    // for it in the same commit as the <img> update let a frame of the empty
    // black layer reach the screen before the frozen desktop did.
    requestAnimationFrame(() => {
      void api.overlayReady(monitorId).catch(() => undefined);
    });
  }, [monitorId]);

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
    // `reveal` runs from the image's load hook (or the effect below when the
    // frame is already decoded), not here: Rust keeps the window hidden until
    // the frozen frame can actually be painted.
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

  // A warm window can be handed a frame that is already decoded (cached data
  // URL, or the browser got there before the effect ran), in which case
  // `onLoad` never fires and the layer would stay hidden until the watchdog.
  useEffect(() => {
    if (!frame) return;
    const img = shotRef.current;
    if (!img || revealed.current) return;
    if (img.complete && img.naturalWidth > 0) {
      const decoded = img.decode ? img.decode() : Promise.resolve();
      void decoded.then(reveal, reveal);
    }
  }, [frame, reveal]);

  // Drop a queued pointer frame if the layer unmounts mid-drag.
  useEffect(
    () => () => {
      if (raf.current !== null) cancelAnimationFrame(raf.current);
    },
    [],
  );

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
    pending.current = { x: event.clientX, y: event.clientY };
    if (raf.current !== null) return;
    raf.current = requestAnimationFrame(() => {
      raf.current = null;
      if (pending.current) setPointer(pending.current);
    });
  };

  const onMouseUp = (event: ReactMouseEvent) => {
    if (!dragging) return;
    if (raf.current !== null) {
      cancelAnimationFrame(raf.current);
      raf.current = null;
    }
    // The coalesced pointer state can lag the physical cursor by a frame, so
    // commit the release position itself instead of reading `box` from state.
    const end = { x: event.clientX, y: event.clientY };
    pending.current = null;
    setPointer(end);
    setDragging(false);

    const finalBox = anchor ? boxBetween(anchor, end) : null;
    if (!finalBox) {
      cancel();
      return;
    }

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
      x: Math.round(finalBox.x * scale),
      y: Math.round(finalBox.y * scale),
      width: Math.round(finalBox.width * scale),
      height: Math.round(finalBox.height * scale),
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

  // Same rule as the commit check in onMouseUp, but measured in physical pixels
  // so the hint agrees with what will actually happen when the mouse is released.
  const previewScale = frame ? frame.width / Math.max(1, window.innerWidth) : 1;
  const tooSmall =
    box !== null &&
    (box.width * previewScale < MIN_PHYSICAL || box.height * previewScale < MIN_PHYSICAL);

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
        <img
          ref={shotRef}
          className="overlay-shot"
          src={frame.dataUrl}
          alt=""
          draggable={false}
          decoding="sync"
          onLoad={reveal}
        />
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

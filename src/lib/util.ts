/** Small shared helpers. No dependencies. */
import { api } from "./api";

export function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max);
}

export function debounce<A extends unknown[]>(fn: (...args: A) => void, ms: number) {
  let timer: number | undefined;
  return (...args: A) => {
    if (timer !== undefined) window.clearTimeout(timer);
    timer = window.setTimeout(() => fn(...args), ms);
  };
}

export function cx(...parts: Array<string | false | null | undefined>): string {
  return parts.filter(Boolean).join(" ");
}

export function formatClock(ms: number): string {
  const date = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  const today = new Date();
  const sameDay = date.toDateString() === today.toDateString();
  const time = `${pad(date.getHours())}:${pad(date.getMinutes())}`;
  if (sameDay) return time;
  return `${date.getMonth() + 1}/${date.getDate()} ${time}`;
}

/** "1.2 s" / "340 ms" */
export function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)} ms`;
  return `${(ms / 1000).toFixed(1)} s`;
}

export function truncate(text: string, max: number): string {
  const clean = text.replace(/\s+/g, " ").trim();
  return clean.length <= max ? clean : `${clean.slice(0, max - 1)}…`;
}

/** Rust clipboard command first (no permission prompt), DOM as a fallback. */
export async function copyText(text: string): Promise<boolean> {
  try {
    await api.copyText(text);
    return true;
  } catch {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      return false;
    }
  }
}

/* ------------------------------------------------------------------ hotkeys */

const MODIFIER_CODES = new Set([
  "ControlLeft",
  "ControlRight",
  "AltLeft",
  "AltRight",
  "ShiftLeft",
  "ShiftRight",
  "MetaLeft",
  "MetaRight",
  "OSLeft",
  "OSRight",
]);

const FUNCTION_KEY = /^F([1-9]|1\d|2[0-4])$/;

/**
 * Turn a keydown into a Tauri accelerator ("Ctrl+Alt+KeyZ"). Returns null while
 * only modifiers are held, and for combinations that would steal ordinary
 * typing (a bare letter), because a global hotkey outranks every app.
 */
export function acceleratorFromEvent(event: KeyboardEvent): string | null {
  if (MODIFIER_CODES.has(event.code)) return null;
  if (event.code === "Escape" && !event.ctrlKey && !event.altKey && !event.metaKey) return null;
  if (event.code === "Tab") return null;

  const parts: string[] = [];
  if (event.ctrlKey) parts.push("Ctrl");
  if (event.altKey) parts.push("Alt");
  if (event.shiftKey) parts.push("Shift");
  if (event.metaKey) parts.push("Super");

  const isFunction = FUNCTION_KEY.test(event.code);
  if (parts.length === 0 && !isFunction) return null;

  parts.push(event.code);
  return parts.join("+");
}

/** "Ctrl+KeyZ" → "Ctrl + Z" (what a human would write). */
export function prettyAccelerator(accelerator: string): string {
  return accelerator
    .split("+")
    .map((part) => prettyKey(part.trim()))
    .filter(Boolean)
    .join(" + ");
}

function prettyKey(part: string): string {
  const table: Record<string, string> = {
    Control: "Ctrl",
    CmdOrCtrl: "Ctrl",
    CommandOrControl: "Ctrl",
    Super: "Win",
    Meta: "Win",
    ArrowUp: "↑",
    ArrowDown: "↓",
    ArrowLeft: "←",
    ArrowRight: "→",
    Space: "Space",
    Escape: "Esc",
    Backquote: "`",
    Minus: "-",
    Equal: "=",
    BracketLeft: "[",
    BracketRight: "]",
    Backslash: "\\",
    Semicolon: ";",
    Quote: "'",
    Comma: ",",
    Period: ".",
    Slash: "/",
  };
  if (table[part]) return table[part];
  return part.replace(/^Key/, "").replace(/^Digit/, "");
}

/** Sort helper for language tags: 中文 first, then alphabetical. */
export function byName(a: { name: string }, b: { name: string }): number {
  return a.name.localeCompare(b.name, "zh-Hans-CN");
}

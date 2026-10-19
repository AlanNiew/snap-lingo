/**
 * Theme plumbing shared by every window.
 *
 * The palette is applied as `data-theme` on <html>; all the colours live in
 * `styles.css` as custom properties, so switching themes never re-renders React
 * and never touches the Rust side.
 *
 * `system` is a choice, not a palette: it resolves to `light`/`dark` before it
 * is applied, and it keeps listening so the window follows the OS live.
 */

import type { StringKey } from "./i18n";

export type ThemeId =
  | "system"
  | "light"
  | "dark"
  | "paper"
  | "midnight"
  | "forest"
  | "contrast";

/** Order shown in the picker. */
export const THEMES: ThemeId[] = [
  "system",
  "light",
  "dark",
  "paper",
  "midnight",
  "forest",
  "contrast",
];

/** Palettes that are dark on screen — used to resolve `system`. */
export const DARK_PALETTES = new Set<string>(["dark", "midnight", "forest", "contrast"]);

/** i18n key per theme, so the picker needs no per-theme branch. */
export const THEME_LABEL_KEYS: Record<ThemeId, StringKey> = {
  system: "themeSystem",
  light: "themeLight",
  dark: "themeDark",
  paper: "themePaper",
  midnight: "themeMidnight",
  forest: "themeForest",
  contrast: "themeContrast",
};

/** Preview colours per palette — kept in sync with the CSS custom properties. */
export const THEME_SWATCHES: Record<
  Exclude<ThemeId, "system">,
  { bg: string; surface: string; accent: string; text: string }
> = {
  light: { bg: "#f3f5f9", surface: "#ffffff", accent: "#4c6ef5", text: "#101622" },
  dark: { bg: "#131519", surface: "#1b1e23", accent: "#7c96ff", text: "#e8ecf2" },
  paper: { bg: "#f7f1e6", surface: "#fffdf8", accent: "#b45309", text: "#3a3126" },
  midnight: { bg: "#0b1120", surface: "#121a2c", accent: "#63a4ff", text: "#e6ecf8" },
  forest: { bg: "#0f1a15", surface: "#16241d", accent: "#43c17a", text: "#e5f0e9" },
  contrast: { bg: "#000000", surface: "#0b0b0b", accent: "#ffd400", text: "#ffffff" },
};

export function isThemeId(value: string | null | undefined): value is ThemeId {
  return typeof value === "string" && (THEMES as string[]).includes(value);
}

export function prefersDark(): boolean {
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

/** Turn a stored value (possibly unknown or missing) into a palette id. */
export function resolveTheme(theme: string | null | undefined): string {
  const wanted = isThemeId(theme) ? theme : "system";
  if (wanted === "system") return prefersDark() ? "dark" : "light";
  return wanted;
}

/**
 * Apply a theme to this window's document.
 * Returns a cleanup function that detaches the OS listener (only set up for
 * `system`); call it from the effect's teardown.
 */
export function applyTheme(theme: string | null | undefined): () => void {
  const wanted = isThemeId(theme) ? theme : "system";
  const paint = () => {
    document.documentElement.dataset.theme = resolveTheme(wanted);
  };
  paint();
  try {
    localStorage.setItem(THEME_STORE_KEY, wanted);
  } catch {
    /* storage can be unavailable; the theme still applies for this session */
  }
  if (wanted !== "system") return () => undefined;
  const query = window.matchMedia("(prefers-color-scheme: dark)");
  query.addEventListener("change", paint);
  return () => query.removeEventListener("change", paint);
}

const THEME_STORE_KEY = "snaplingo.theme";

/**
 * Paint the last known theme before the config is fetched.
 *
 * Every window is told its palette from the config, which arrives over IPC a
 * moment after the page loads; without this the window would flash the default
 * light palette first. The choice is cached in localStorage by `applyTheme`.
 */
export function applyStoredTheme(): void {
  try {
    const stored = localStorage.getItem(THEME_STORE_KEY);
    if (stored) document.documentElement.dataset.theme = resolveTheme(stored);
  } catch {
    /* ignore */
  }
}

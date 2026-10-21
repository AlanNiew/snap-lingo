/**
 * The only place that talks to Rust.
 *
 * Every command name here matches a `#[tauri::command]` in
 * `src-tauri/src/commands.rs`; argument keys are camelCase because that is what
 * Tauri v2 expects for snake_case Rust parameters.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow, type Window } from "@tauri-apps/api/window";

import type {
  AppConfig,
  ConnectionCheck,
  DownloadedUpdate,
  Endpoints,
  HistoryItem,
  LogFile,
  OcrConfig,
  ResultPayload,
  SelectionRect,
  SessionImage,
  StageEvent,
  TranslateConfig,
  UpdateInfo,
} from "./types";

export interface MonitorInfo {
  id: number;
  name: string;
  x: number;
  y: number;
  width: number;
  height: number;
  scaleFactor: number;
}

export interface OcrLanguage {
  tag: string;
  name: string;
}

/** Payload of `toast:changed` — `kind` is `"error"` or `"notice"`. */
export interface Notice {
  kind: string;
  message: string;
}

/* ------------------------------------------------------------------- events */

export const EVENTS = {
  stage: "pipeline:stage",
  result: "result:update",
  config: "config:changed",
  hotkeyError: "hotkey:error",
  navigate: "ui:navigate",
  session: "session:changed",
  toast: "toast:changed",
} as const;

export const api = {
  /* config */
  getConfig: () => invoke<AppConfig>("get_config"),
  saveConfig: (config: AppConfig) => invoke<AppConfig>("save_config", { config }),
  resetConfig: () => invoke<AppConfig>("reset_config"),

  /* services */
  listModels: (baseUrl: string, apiKey: string) =>
    invoke<string[]>("list_models", { baseUrl, apiKey }),
  testOcrConnection: (baseUrl: string, apiKey: string, model: string) =>
    invoke<ConnectionCheck>("test_ocr_connection", { baseUrl, apiKey, model }),
  testTranslateConnection: (config: TranslateConfig) =>
    invoke<ConnectionCheck>("test_translate_connection", { config }),
  ocrLanguages: () => invoke<OcrLanguage[]>("ocr_languages"),
  ocrProbe: (language: string) => invoke<ConnectionCheck>("ocr_probe", { language }),
  testLocalOcrConnection: (config: OcrConfig) =>
    invoke<ConnectionCheck>("test_local_ocr_connection", { config }),

  /* capture */
  startCapture: () => invoke<void>("start_capture"),
  cancelCapture: () => invoke<void>("cancel_capture"),
  finishSelection: (rect: SelectionRect) => invoke<void>("finish_selection", { rect }),
  overlayReady: (monitorId: number) => invoke<void>("overlay_ready", { monitorId }),
  getSessionImage: (monitorId: number) => invoke<SessionImage | null>("get_session_image", { monitorId }),
  monitors: () => invoke<MonitorInfo[]>("monitors_info"),

  /* result */
  getLastResult: () => invoke<ResultPayload | null>("get_last_result"),
  retryTranslation: () => invoke<void>("retry_translation"),
  hideResult: () => invoke<void>("hide_result"),
  fitResultWindow: (width: number, height: number) =>
    invoke<void>("fit_result_window", { width, height }),
  copyText: (text: string) => invoke<void>("copy_text", { text }),

  /* history */
  history: () => invoke<HistoryItem[]>("get_history"),
  clearHistory: () => invoke<void>("clear_history"),
  deleteHistoryItem: (id: string) => invoke<HistoryItem[]>("delete_history_item", { id }),

  /* shell */
  openSettings: () => invoke<void>("open_settings"),
  hideSettings: () => invoke<void>("hide_settings"),
  hideToast: () => invoke<void>("hide_toast"),
  openLogs: () => invoke<void>("open_logs"),
  openUpdate: () => invoke<void>("open_update"),
  checkUpdate: () => invoke<UpdateInfo>("check_update"),
  downloadUpdate: () => invoke<DownloadedUpdate>("download_update"),
  installUpdate: (path: string) => invoke<void>("install_update", { path }),
  openExternal: (url: string) => invoke<void>("open_external", { url }),
  revealConfigDir: () => invoke<void>("reveal_config_dir"),
  openGuide: () => invoke<void>("open_guide"),
  describeEndpoints: (config: AppConfig) =>
    invoke<Endpoints>("describe_endpoints", { config }),
  readLog: () => invoke<LogFile>("read_log"),
  clearLog: () => invoke<void>("clear_log"),
  revealLog: () => invoke<void>("reveal_log"),
};

/* ------------------------------------------------------------------ listens */

export function onStage(handler: (event: StageEvent) => void): Promise<UnlistenFn> {
  return listen<StageEvent>(EVENTS.stage, (event) => handler(event.payload));
}

export function onResult(handler: (payload: ResultPayload) => void): Promise<UnlistenFn> {
  return listen<ResultPayload>(EVENTS.result, (event) => handler(event.payload));
}

export function onConfigChanged(handler: (config: AppConfig) => void): Promise<UnlistenFn> {
  return listen<AppConfig>(EVENTS.config, (event) => handler(event.payload));
}

export function onNavigate(handler: (target: string) => void): Promise<UnlistenFn> {
  return listen<string>(EVENTS.navigate, (event) => handler(event.payload));
}

export function onHotkeyError(handler: (message: string) => void): Promise<UnlistenFn> {
  return listen<string>(EVENTS.hotkeyError, (event) => handler(event.payload));
}

/** Fires in a warm overlay when a new capture has been frozen for its monitor. */
export function onSession(handler: (monitorId: number) => void): Promise<UnlistenFn> {
  return listen<number>(EVENTS.session, (event) => handler(event.payload));
}

/** The branded notice window: what to say, and whether it is a failure. */
export function onToast(handler: (notice: Notice) => void): Promise<UnlistenFn> {
  return listen<Notice>(EVENTS.toast, (event) => handler(event.payload));
}

/* ------------------------------------------------------------------ helpers */

export const currentWindow: Window = getCurrentWindow();

/** `overlay-3` → 3. Returns null when the label is not a per-monitor overlay. */
export function monitorIdFromLabel(label: string): number | null {
  const match = /^overlay-(\d+)$/.exec(label);
  return match ? Number(match[1]) : null;
}

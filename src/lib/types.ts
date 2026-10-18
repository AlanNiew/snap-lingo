/** Shared types — must mirror the Rust structs in `src-tauri/src/config.rs` and `model.rs`. */

export type OcrProvider = "vision" | "winocr";
export type TranslateProvider =
  | "openai"
  | "deepl"
  | "microsoft"
  | "google"
  | "youdao"
  | "baidu"
  | "tencent"
  | "none";
export type DeepLPlan = "free" | "pro";

export interface OcrConfig {
  provider: OcrProvider;
  baseUrl: string;
  apiKey: string;
  model: string;
  /** Windows OCR language tag, e.g. "zh-Hans"; empty = from user profile. */
  language: string;
  /** Vision models: ask the model to return text + translation in one call. */
  oneShot: boolean;
  /** Optional custom OCR instruction; empty = built-in prompt. */
  prompt: string;
}

/** Credentials and endpoint for one engine. Each engine keeps its own copy so a
 *  key typed for one service never shows up in another service's form. */
export interface TranslateProfile {
  baseUrl: string;
  apiKey: string;
  /** Second credential: Youdao app secret, Baidu key, Tencent secretKey. */
  apiSecret: string;
  /** Azure Translator region / Tencent region. */
  region: string;
  model: string;
  /** Youdao / Baidu / Tencent can read the picture themselves (one-step OCR + translate). */
  imageTranslate: boolean;
  /** Engine tier: "standard" | "llm" (Baidu, Youdao) or "llm" | "lite" (Tencent). */
  modelTier: string;
}

/** A blank profile, used while the user is still filling one in. */
export const EMPTY_PROFILE: TranslateProfile = {
  baseUrl: "",
  apiKey: "",
  apiSecret: "",
  region: "",
  model: "",
  imageTranslate: false,
  modelTier: "",
};

export interface TranslateConfig {
  provider: TranslateProvider;
  /** One entry per engine, keyed by provider id. */
  profiles: Record<string, TranslateProfile>;
  /** BCP-47-ish target language tag, e.g. "zh-CN", "en", "ja". */
  targetLang: string;
  deeplPlan: DeepLPlan;
  /** Optional custom translation instruction; empty = built-in prompt. */
  prompt: string;
}

export interface CaptureConfig {
  /** Capture every monitor and let the user pick on any of them. */
  allMonitors: boolean;
  /** Upscale tiny crops before OCR (helps small UI text a lot). */
  upscaleSmall: boolean;
  /** Long-edge cap applied before sending the image to a vision model. */
  maxImageSide: number;
  jpegQuality: number;
}

export interface UiConfig {
  autoCopy: boolean;
  /** What the result window shows: both / source only / translation only. */
  resultView: "both" | "source" | "translation";
  resultShowImage: boolean;
  closeOnBlur: boolean;
  resultFontSize: number;
  theme: "system" | "dark" | "light";
  language: "zh" | "en";
}

export interface HistoryConfig {
  enabled: boolean;
  limit: number;
}

export interface AppConfig {
  shortcut: string;
  ocr: OcrConfig;
  translate: TranslateConfig;
  capture: CaptureConfig;
  ui: UiConfig;
  startup: boolean;
  history: HistoryConfig;
}

export type Stage = "idle" | "capturing" | "ocr" | "translate" | "done" | "error";

export interface StageEvent {
  stage: Stage;
  message?: string | null;
  elapsedMs: number;
}

export interface SessionImage {
  monitorId: number;
  /** data:image/png;base64,... */
  dataUrl: string;
  /** Physical pixels. */
  width: number;
  height: number;
  /** Monitor scale factor (physical / logical). */
  scaleFactor: number;
  name: string;
}

export interface SelectionRect {
  monitorId: number;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface ResultPayload {
  id: string;
  sourceText: string;
  translatedText: string | null;
  targetLang: string;
  ocrProvider: string;
  translateProvider: string;
  model: string;
  elapsedMs: number;
  createdAt: number;
  /** Small base64 thumbnail of the captured region (may be empty). */
  preview: string | null;
  error: string | null;
}

export interface HistoryItem {
  id: string;
  sourceText: string;
  translatedText: string | null;
  targetLang: string;
  createdAt: number;
}

export interface ConnectionCheck {
  ok: boolean;
  message: string;
  /** Round-trip time in ms. */
  elapsedMs: number;
  /** Model reply text, when the check performed a real completion. */
  sample: string | null;
}

export interface ProviderPreset {
  id: string;
  label: string;
  hint: string;
  kind: "ocr" | "translate" | "both";
  baseUrl: string;
  visionModel: string;
  textModel: string;
  /** Official page where an API key for this service is issued. */
  applyUrl: string;
}

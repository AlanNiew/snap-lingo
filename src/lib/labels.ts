import type { StringKey } from "./i18n";
import type { OcrProvider, TranslateProvider } from "./types";

/** Display names for every translation service, shared by settings and the result window. */
export const PROVIDER_KEYS: Record<TranslateProvider, StringKey> = {
  openai: "translateOpenai",
  deepl: "translateDeepl",
  microsoft: "translateMicrosoft",
  google: "translateGoogle",
  youdao: "translateYoudao",
  baidu: "translateBaidu",
  tencent: "translateTencent",
  none: "translateNone",
};

/** Which credential boxes an engine asks for, in the order they are shown. */
export type ProfileField =
  | "baseUrl"
  | "apiKey"
  | "apiSecret"
  | "region"
  | "model"
  | "auth"
  | "bearer";

export interface TranslateEngine {
  id: TranslateProvider;
  titleKey: StringKey;
  descKey: StringKey;
  /** Exactly the fields this engine needs; nothing else is rendered. */
  fields: ProfileField[];
  /** Official page that issues the credentials. */
  applyUrl: string;
  keyPlaceholder: string;
  /** The service can read the picture itself: OCR + translation in one call. */
  picture?: boolean;
  /** Service tiers the user may pick between (e.g. Baidu's standard vs LLM). */
  tiers?: Array<{ value: string; labelKey: StringKey }>;
}

/**
 * One row per translation service. The settings page renders the engine list
 * from here, so adding a service is a single entry in this table plus its
 * backend implementation.
 */
export const TRANSLATE_ENGINES: TranslateEngine[] = [
  {
    id: "openai",
    titleKey: "translateOpenai",
    descKey: "translateOpenaiDesc",
    fields: ["baseUrl", "apiKey", "model"],
    applyUrl: "https://platform.openai.com/api-keys",
    keyPlaceholder: "sk-…",
  },
  {
    id: "deepl",
    titleKey: "translateDeepl",
    descKey: "translateDeeplDesc",
    fields: ["apiKey"],
    applyUrl: "https://www.deepl.com/your-account/keys",
    keyPlaceholder: "xxxxxxxx-xxxx-…:fx",
  },
  {
    id: "microsoft",
    titleKey: "translateMicrosoft",
    descKey: "translateMicrosoftDesc",
    fields: ["apiKey", "region"],
    applyUrl: "https://portal.azure.com/#create/Microsoft.CognitiveServicesTextTranslation",
    keyPlaceholder: "Azure 密钥",
  },
  {
    id: "google",
    titleKey: "translateGoogle",
    descKey: "translateGoogleDesc",
    fields: ["apiKey"],
    applyUrl: "https://console.cloud.google.com/apis/credentials",
    keyPlaceholder: "AIza…",
  },
  {
    id: "youdao",
    titleKey: "translateYoudao",
    descKey: "translateYoudaoDesc",
    fields: ["apiKey", "apiSecret"],
    applyUrl: "https://ai.youdao.com/console/#/app-overview",
    keyPlaceholder: "应用 ID",
    picture: true,
    tiers: [
      { value: "standard", labelKey: "tierStandard" },
      { value: "llm", labelKey: "tierLlmPro" },
      { value: "lite", labelKey: "tierLlmLite" },
    ],
  },
  {
    id: "baidu",
    titleKey: "translateBaidu",
    descKey: "translateBaiduDesc",
    fields: ["apiKey", "apiSecret"],
    applyUrl: "https://fanyi-api.baidu.com/",
    keyPlaceholder: "AppID",
    picture: true,
    tiers: [
      { value: "standard", labelKey: "tierStandard" },
      { value: "llm", labelKey: "tierLlm" },
    ],
  },
  {
    id: "tencent",
    titleKey: "translateTencent",
    descKey: "translateTencentDesc",
    fields: ["apiKey", "apiSecret", "region"],
    applyUrl: "https://console.cloud.tencent.com/cam/capi",
    keyPlaceholder: "SecretId",
    picture: true,
    tiers: [
      { value: "llm", labelKey: "tierLlm" },
      { value: "lite", labelKey: "tierLite" },
    ],
  },
  {
    id: "none",
    titleKey: "translateNone",
    descKey: "translateNoneDesc",
    fields: [],
    applyUrl: "",
    keyPlaceholder: "",
  },
];

export function engineMeta(id: TranslateProvider): TranslateEngine | undefined {
  return TRANSLATE_ENGINES.find((engine) => engine.id === id);
}

/** Which inputs an OCR engine needs, in the order they are shown. */
export type OcrField = "baseUrl" | "apiKey" | "model" | "language";

export interface OcrEngine {
  id: OcrProvider;
  titleKey: StringKey;
  descKey: StringKey;
  fields: OcrField[];
  /** Page that explains / provides the service (opened from a button). */
  applyUrl?: string;
}

/** Every OCR path, listed in the engine dropdown. */
export const OCR_ENGINES: OcrEngine[] = [
  {
    id: "winocr",
    titleKey: "ocrWin",
    descKey: "ocrWinDesc",
    fields: ["language"],
  },
  {
    id: "vision",
    titleKey: "ocrVision",
    descKey: "ocrVisionDesc",
    fields: ["baseUrl", "apiKey", "model"],
  },
  {
    id: "local",
    titleKey: "ocrLocal",
    descKey: "ocrLocalDesc",
    fields: ["baseUrl"],
    applyUrl: "https://github.com/hiroi-sora/Umi-OCR/releases",
  },
  {
    id: "picture",
    titleKey: "ocrPicture",
    descKey: "ocrPictureDesc",
    fields: [],
  },
];

/* ================================================================== target */

/**
 * Target-language choices shared by the settings page and the result window.
 *
 * `auto` is the smart option: the pipeline looks at the recognised text and
 * decides the target itself (Chinese in → English out, anything else → Chinese).
 * Its label is empty on purpose — callers render `targetAuto` in the UI locale.
 */
export interface TargetLangOption {
  value: string;
  /** Native name of the language; empty for `auto`. */
  label: string;
}

export const AUTO_TARGET = "auto";

export const TARGET_LANGS: TargetLangOption[] = [
  { value: AUTO_TARGET, label: "" },
  { value: "zh-CN", label: "简体中文" },
  { value: "zh-TW", label: "繁體中文" },
  { value: "en", label: "English" },
  { value: "ja", label: "日本語" },
  { value: "ko", label: "한국어" },
  { value: "fr", label: "Français" },
  { value: "de", label: "Deutsch" },
  { value: "es", label: "Español" },
  { value: "ru", label: "Русский" },
  { value: "pt", label: "Português" },
  { value: "it", label: "Italiano" },
  { value: "ar", label: "العربية" },
  { value: "hi", label: "हिन्दी" },
  { value: "th", label: "ไทย" },
  { value: "vi", label: "Tiếng Việt" },
  { value: "id", label: "Bahasa Indonesia" },
  { value: "tr", label: "Türkçe" },
];

/** Native label for a target tag; `auto` and unknown tags come back as-is. */
export function targetLabel(value: string): string {
  const match = TARGET_LANGS.find((option) => option.value === value);
  return match ? match.label || value : value;
}

/** Keep a stored value selectable even when it is not in the list. */
export function withTarget(
  options: TargetLangOption[],
  current: string,
): TargetLangOption[] {
  if (!current || options.some((option) => option.value === current)) return options;
  return [...options, { value: current, label: current }];
}

/**
 * Label for a target tag in the UI language. `auto` has no native name, so the
 * caller passes `t("targetAuto")` and this walks `TARGET_LANGS` as-is.
 */
export function targetOptionLabel(value: string, auto: string): string {
  if (value === AUTO_TARGET) return auto;
  return targetLabel(value);
}

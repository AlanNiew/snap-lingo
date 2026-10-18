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
export type ProfileField = "baseUrl" | "apiKey" | "apiSecret" | "region" | "model";

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
      { value: "llm", labelKey: "tierLlm" },
      { value: "standard", labelKey: "tierStandard" },
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

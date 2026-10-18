import type { StringKey } from "./i18n";
import type { TranslateProvider } from "./types";

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

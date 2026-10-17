import { createContext, useContext } from "react";

import type { Lang, StringKey } from "../lib/i18n";

export type TFn = (key: StringKey, vars?: Record<string, string | number>) => string;

export const LangCtx = createContext<{ lang: Lang; t: TFn }>({
  lang: "zh",
  t: (key) => String(key),
});

export function useT(): { lang: Lang; t: TFn } {
  return useContext(LangCtx);
}

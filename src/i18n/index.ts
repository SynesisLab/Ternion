import { en, type I18nKey } from "./en";

const dicts = { en } as const;

/**
 * Translate a key into the active language. M0 has exactly one dictionary;
 * M4 introduces a locale parameter and plural forms behind the same call site.
 */
export function t(key: I18nKey): string {
  return dicts.en[key];
}

export type { I18nKey };
import { useEffect } from "react";
import { create } from "zustand";

import { en, type I18nKey } from "./en";
import { zhTW } from "./zhTW";
import { getSetting } from "../lib/ipc";
import { settingsKeys } from "../lib/settingsKeys";

const dicts = { en, "zh-TW": zhTW } as const;

/** Locales shipped with the app (§9.5): English default, zh-TW v1. */
export type Locale = keyof typeof dicts;

interface I18nState {
  locale: Locale;
  setLocale: (l: Locale) => void;
}

/**
 * Locale lives in a tiny store so a language switch can re-key the app tree
 * and force every `t()` call site to re-render with the new dictionary.
 * `t()` itself stays a plain synchronous accessor — no per-component wiring.
 */
export const useI18n = create<I18nState>()((set) => ({
  locale: "en",
  setLocale: (locale) => set({ locale }),
}));

/** §9.5: apply the persisted language once at window boot. */
export function useApplyStoredLocale() {
  const setLocale = useI18n((s) => s.setLocale);
  useEffect(() => {
    void getSetting(settingsKeys.uiLocale)
      .then((l) => {
        if (l === "zh-TW" || l === "en") setLocale(l);
      })
      .catch(() => {});
  }, [setLocale]);
}

/** Translate a key into the active language. */
export function t(key: I18nKey): string {
  return dicts[useI18n.getState().locale][key];
}

export type { I18nKey };
import { useEffect } from "react";
import { create } from "zustand";

import { getSetting } from "./ipc";
import { settingsKeys } from "./settingsKeys";

/** Appearance modes (§10 settings): `dark` is the historical default,
 * `light` flips the token block in index.css, `system` follows the OS. */
export type ThemeMode = "dark" | "light" | "system";

interface ThemeState {
  mode: ThemeMode;
  setMode: (mode: ThemeMode) => void;
}

export const useTheme = create<ThemeState>()((set) => ({
  mode: "dark",
  setMode: (mode) => set({ mode }),
}));

function resolveDark(mode: ThemeMode, systemDark: boolean): boolean {
  return mode === "dark" || (mode === "system" && systemDark);
}

/** Boot + live application: loads the persisted `ui.theme` once, then keeps
 * the `light`/`dark` classes on documentElement in sync (system mode follows
 * the OS via matchMedia). Runs once per window from App(). */
export function useApplyStoredTheme() {
  const setMode = useTheme((s) => s.setMode);
  const mode = useTheme((s) => s.mode);

  useEffect(() => {
    void getSetting(settingsKeys.uiTheme)
      .then((v) => {
        if (v === "dark" || v === "light" || v === "system") setMode(v);
      })
      .catch(() => {});
  }, [setMode]);

  useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      const dark = resolveDark(mode, mq.matches);
      document.documentElement.classList.toggle("dark", dark);
      document.documentElement.classList.toggle("light", !dark);
    };
    apply();
    if (mode !== "system") return;
    mq.addEventListener("change", apply);
    return () => mq.removeEventListener("change", apply);
  }, [mode]);
}
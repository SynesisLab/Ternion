import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";
import "highlight.js/styles/github-dark.css";

import { getSetting } from "./lib/ipc";
import { settingsKeys } from "./lib/settingsKeys";
import { useTheme, type ThemeMode } from "./lib/theme";
import { useI18n } from "./i18n";

declare global {
  interface Window {
    __ternionBoot?: (message: string) => void;
  }
}

window.__ternionBoot?.("boot: modules loaded");

// §9.5: hydrate the persisted appearance (locale + theme) into their stores
// BEFORE the first paint — the App-level hooks re-apply and subscribe for
// runtime changes; this read only kills the boot flash for non-default
// users. Render waits one local IPC round-trip, then happens regardless.
void Promise.all([
  getSetting(settingsKeys.uiLocale).catch(() => null),
  getSetting(settingsKeys.uiTheme).catch(() => null),
])
  .then(([locale, theme]) => {
    if (locale === "zh-TW" || locale === "en") useI18n.setState({ locale });
    if (theme === "dark" || theme === "light" || theme === "system") {
      useTheme.setState({ mode: theme as ThemeMode });
    }
  })
  .finally(() => {
    ReactDOM.createRoot(document.getElementById("root")!).render(
      <React.StrictMode>
        <App />
      </React.StrictMode>,
    );
    window.__ternionBoot?.("boot: react mounted");
  });
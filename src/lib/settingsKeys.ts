/** Settings keys — mirrors `src-tauri/src/settings.rs`. Keep both in sync. */
export const settingsKeys = {
  ollamaBaseUrl: "ollama.base_url",
  chatDefaultModel: "chat.default_model",
  chatTemperature: "chat.temperature",
  chatContextTokens: "chat.context_tokens",
  chatKeepAlive: "chat.keep_alive",
  appCloseToTray: "app.close_to_tray",
  uiTheme: "ui.theme",
} as const;
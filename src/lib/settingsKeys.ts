/** Settings keys — mirrors `src-tauri/src/settings.rs`. Keep both in sync. */
export const settingsKeys = {
  ollamaBaseUrl: "ollama.base_url",
  chatDefaultModel: "chat.default_model",
  chatTemperature: "chat.temperature",
  chatContextTokens: "chat.context_tokens",
  chatKeepAlive: "chat.keep_alive",
  appCloseToTray: "app.close_to_tray",
  uiTheme: "ui.theme",

  // Tool runtime (§6)
  toolsMaxHops: "tools.max_hops",
  toolsShellEnabled: "tools.shell_enabled",

  // Triad router (M1, design §3.5)
  triadEnabled: "triad.enabled",
  triadSkipRouter: "triad.skip_router",
  triadRoleHerald: "triad.role.herald",
  triadRoleScout: "triad.role.scout",
  triadRoleTitan: "triad.role.titan",
  triadMinConfidence: "triad.min_confidence",
  triadDeescalationConfidence: "triad.deescalation_confidence",
  triadStickyTurns: "triad.sticky_turns",
  triadScoutOutputCeiling: "triad.scout_output_ceiling",
  triadHandoffRecentMessages: "triad.handoff_recent_messages",
  triadSidecarTitles: "triad.sidecar_titles",
  triadSidecarSuggestions: "triad.sidecar_suggestions",
  triadHeraldTimeoutMs: "triad.herald_timeout_ms",
  triadAdaptiveEnabled: "triad.adaptive.enabled",
  heraldKeepAlive: "herald.keep_alive",
  triadScoutKeepAlive: "triad.scout_keep_alive",
  triadTitanKeepAlive: "triad.titan_keep_alive",
} as const;
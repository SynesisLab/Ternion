//! Typed setting keys + an in-memory cache over the `settings` table.
//! The DB is the source of truth; the cache mirrors it for synchronous reads
//! from hot paths (residency policy, close-to-tray behavior).

use std::{collections::HashMap, sync::RwLock};

pub mod keys {
    pub const OLLAMA_BASE_URL: &str = "ollama.base_url";
    pub const CHAT_DEFAULT_MODEL: &str = "chat.default_model";
    pub const CHAT_TEMPERATURE: &str = "chat.temperature";
    pub const CHAT_CONTEXT_TOKENS: &str = "chat.context_tokens";
    pub const CHAT_KEEP_ALIVE: &str = "chat.keep_alive";
    pub const APP_CLOSE_TO_TRAY: &str = "app.close_to_tray";
    pub const UI_THEME: &str = "ui.theme";

    // Triad v1 (design §3) — mirrors `router/config.rs`. Keep in sync.
    pub const TRIAD_ENABLED: &str = "triad.enabled";
    pub const TRIAD_SKIP_ROUTER: &str = "triad.skip_router";
    pub const TRIAD_ROLE_HERALD: &str = "triad.role.herald";
    pub const TRIAD_ROLE_SCOUT: &str = "triad.role.scout";
    pub const TRIAD_ROLE_TITAN: &str = "triad.role.titan";
    pub const TRIAD_MIN_CONFIDENCE: &str = "triad.min_confidence";
    pub const TRIAD_DEESCALATION_CONFIDENCE: &str = "triad.deescalation_confidence";
    pub const TRIAD_STICKY_TURNS: &str = "triad.sticky_turns";
    pub const TRIAD_SCOUT_OUTPUT_CEILING: &str = "triad.scout_output_ceiling";
    pub const TRIAD_HANDOFF_RECENT_MESSAGES: &str = "triad.handoff_recent_messages";
    pub const TRIAD_SIDECAR_TITLES: &str = "triad.sidecar_titles";
    pub const TRIAD_SIDECAR_SUGGESTIONS: &str = "triad.sidecar_suggestions";
    pub const TRIAD_HERALD_TIMEOUT_MS: &str = "triad.herald_timeout_ms";
    pub const HERALD_KEEP_ALIVE: &str = "herald.keep_alive";
    pub const TRIAD_SCOUT_KEEP_ALIVE: &str = "triad.scout_keep_alive";
    pub const TRIAD_TITAN_KEEP_ALIVE: &str = "triad.titan_keep_alive";
}

pub struct SettingsCache {
    values: RwLock<HashMap<String, String>>,
}

impl SettingsCache {
    pub fn new(values: HashMap<String, String>) -> Self {
        Self {
            values: RwLock::new(values),
        }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.values
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(key)
            .cloned()
    }

    pub fn get_or(&self, key: &str, default: &str) -> String {
        self.get(key).unwrap_or_else(|| default.to_string())
    }

    pub fn set(&self, key: &str, value: String) {
        self.values
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key.to_string(), value);
    }
}
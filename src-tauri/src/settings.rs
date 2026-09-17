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
    /// §9.5: UI language — "en" (default) or "zh-TW".
    pub const UI_LOCALE: &str = "ui.locale";

    // Privacy (§3.10): local-only mode suppresses cloud endpoints app-wide.
    pub const PRIVACY_LOCAL_ONLY: &str = "privacy.local_only";

    // Tool runtime (design §6.1).
    pub const TOOLS_MAX_HOPS: &str = "tools.max_hops";
    /// Shell tool opt-in (§6.2): off until the user enables it.
    pub const TOOLS_SHELL_ENABLED: &str = "tools.shell_enabled";

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
    /// Free-form text appended to the Herald classify prompt — the user's
    /// own "what kind of scenario should use what model" guidance.
    pub const TRIAD_ROUTING_GUIDANCE: &str = "triad.routing_guidance";
    pub const HERALD_KEEP_ALIVE: &str = "herald.keep_alive";
    pub const TRIAD_SCOUT_KEEP_ALIVE: &str = "triad.scout_keep_alive";
    pub const TRIAD_TITAN_KEEP_ALIVE: &str = "triad.titan_keep_alive";

    // §3.11 adaptive tuning (experimental): flag-class threshold bumps
    // learned from pin overrides. `flag` is one of code/tools/long_form/
    // multi_step/plain.
    pub const TRIAD_ADAPTIVE_ENABLED: &str = "triad.adaptive.enabled";
    pub const ADAPTIVE_BUMP_PREFIX: &str = "triad.adaptive.bump.";
    pub const ADAPTIVE_COUNT_PREFIX: &str = "triad.adaptive.count.";

    pub fn adaptive_bump_key(flag: &str) -> String {
        format!("{ADAPTIVE_BUMP_PREFIX}{flag}")
    }

    pub fn adaptive_count_key(flag: &str) -> String {
        format!("{ADAPTIVE_COUNT_PREFIX}{flag}")
    }
}

/// Flag classes the adaptive loop tunes (§3.11). `plain` covers decisions
/// with none of the others set.
pub const ADAPTIVE_FLAGS: &[&str] = &["code", "tools", "long_form", "multi_step", "plain"];

pub struct SettingsCache {
    values: RwLock<HashMap<String, String>>,
}

impl Clone for SettingsCache {
    fn clone(&self) -> Self {
        Self {
            values: RwLock::new(
                self.values
                    .read()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone(),
            ),
        }
    }
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

    /// All (key, value) pairs whose key starts with `prefix` — the adaptive
    /// bump table is keyed per flag class, so it can't be a fixed list.
    pub fn prefix(&self, prefix: &str) -> Vec<(String, String)> {
        self.values
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    pub fn remove(&self, key: &str) {
        self.values
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(key);
    }
}
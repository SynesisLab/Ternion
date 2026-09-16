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
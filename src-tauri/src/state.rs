//! Shared application state managed by Tauri.

use std::{collections::HashMap, sync::Arc, sync::RwLock};

use crate::{db::Database, error::CmdError, providers::Provider, settings::SettingsCache};

/// The single endpoint id shipped in M0 (M3 adds user-managed profiles).
pub const DEFAULT_ENDPOINT: &str = "ep_local_ollama";

pub struct AppState {
    pub db: Database,
    pub settings: SettingsCache,
    pub http: reqwest::Client,
    /// Streaming cancellation registry (keyed by conversation — M0 has one
    /// active stream per conversation by construction; M1 may re-key).
    pub streams: RwLock<HashMap<String, StreamEntry>>,
    providers: RwLock<HashMap<String, Arc<dyn Provider>>>,
}

/// Cancellation entry for an in-flight stream.
pub struct StreamEntry {
    pub cancel: tokio_util::sync::CancellationToken,
    pub message_id: String,
}

impl AppState {
    pub fn new(
        db: Database,
        settings: SettingsCache,
        http: reqwest::Client,
        providers: HashMap<String, Arc<dyn Provider>>,
    ) -> Self {
        Self {
            db,
            settings,
            http,
            streams: RwLock::new(HashMap::new()),
            providers: RwLock::new(providers),
        }
    }

    pub fn provider_for(&self, endpoint_id: &str) -> Result<Arc<dyn Provider>, CmdError> {
        self.providers
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(endpoint_id)
            .cloned()
            .ok_or_else(|| CmdError::internal(format!("unknown endpoint: {endpoint_id}")))
    }

    /// Rebuild the provider registry from current settings (called when
    /// `ollama.base_url` changes).
    pub fn rebuild_providers(&self) {
        let base = self.settings.get_or(
            crate::settings::keys::OLLAMA_BASE_URL,
            "http://127.0.0.1:11434",
        );
        let map = crate::providers::build_providers(&base, self.http.clone());
        *self.providers.write().unwrap_or_else(|p| p.into_inner()) = map;
    }
}
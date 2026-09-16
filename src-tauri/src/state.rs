//! Shared application state managed by Tauri.

use crate::{db::Database, settings::SettingsCache};

/// The streams registry (cancellation tokens per conversation) is added in
/// step 7; providers registry in step 6.
pub struct AppState {
    pub db: Database,
    pub settings: SettingsCache,
}
//! Shared application state managed by Tauri.

use std::{collections::HashMap, sync::Arc, sync::RwLock};

use tokio::sync::Mutex as AsyncMutex;

use crate::{db::Database, error::CmdError, permissions, providers::Provider, settings::SettingsCache, types::ApprovalReply};

/// The single endpoint id shipped in M0 (M3 adds user-managed profiles).
pub const DEFAULT_ENDPOINT: &str = "ep_local_ollama";

pub struct AppState {
    pub db: Database,
    pub settings: SettingsCache,
    pub http: reqwest::Client,
    /// Streaming cancellation registry (keyed by conversation — M0 has one
    /// active stream per conversation by construction; M1 may re-key).
    pub streams: RwLock<HashMap<String, StreamEntry>>,
    /// Capability facts from the last successful model discovery — the
    /// policy engine's vision/context hard rules read this (design §5.4).
    pub model_registry: RwLock<Vec<crate::types::ModelInfo>>,
    /// Follow-up suggestion chips per conversation (Herald sidecar, §3.7).
    /// Cleared whenever a new send starts for that conversation. Arc-shared
    /// so spawned sidecar tasks can write their results.
    pub suggestions: Arc<RwLock<HashMap<String, Vec<String>>>>,
    /// Bundled tool set (§6.1). Immutable after startup.
    pub tool_registry: crate::tools::ToolRegistry,
    /// §6.5 MCP sessions — spawned lazily per enabled server, reused across
    /// turns; a config change drops the cache (`invalidate`).
    pub mcp: crate::mcp::McpManager,
    /// §6.6 pending approval requests, keyed by request id. The executor
    /// parks its `oneshot` here; `respond_approval` resolves it.
    pub approvals: AsyncMutex<HashMap<String, tokio::sync::oneshot::Sender<ApprovalReply>>>,
    /// §6.6 "allow for session" grants — in-memory, lost on exit.
    pub session_grants: AsyncMutex<permissions::SessionGrants>,
    /// Provider registry keyed by endpoint id. `pub(crate)` so tests can seed
    /// extra endpoints (chat.rs multi-endpoint tests).
    pub(crate) providers: RwLock<HashMap<String, Arc<dyn Provider>>>,
    /// Directory for attachment file bodies (§7.2/§8.1):
    /// `%APPDATA%\Ternion\attachments\` in production, a temp dir in tests.
    pub attachments_dir: std::path::PathBuf,
    /// Which window initiated the current §7.1 region capture ("main" or
    /// "quick") — `capture_region` announces the stored attachment there.
    pub capture_target: RwLock<String>,
}

/// Cancellation entry for an in-flight stream.
#[derive(Clone)]
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
        attachments_dir: std::path::PathBuf,
    ) -> Self {
        Self {
            db,
            settings,
            http,
            streams: RwLock::new(HashMap::new()),
            model_registry: RwLock::new(Vec::new()),
            suggestions: Arc::new(RwLock::new(HashMap::new())),
            tool_registry: crate::tools::ToolRegistry::bundled(),
            mcp: crate::mcp::McpManager::default(),
            approvals: AsyncMutex::new(HashMap::new()),
            session_grants: AsyncMutex::new(permissions::SessionGrants::default()),
            providers: RwLock::new(providers),
            attachments_dir,
            capture_target: RwLock::new("main".to_string()),
        }
    }

    /// Capability facts for a model id, if discovery has run.
    pub fn registry_model(&self, model: &str) -> Option<crate::types::ModelInfo> {
        self.model_registry
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .find(|m| m.id == model)
            .cloned()
    }

    /// Swap in fresh discovery results (called by the list_models command).
    pub fn update_model_registry(&self, models: Vec<crate::types::ModelInfo>) {
        *self
            .model_registry
            .write()
            .unwrap_or_else(|p| p.into_inner()) = models;
    }

    /// Herald follow-up chips for a conversation, if the sidecar ran.
    pub fn take_suggestions(&self, conversation_id: &str) -> Vec<String> {
        self.suggestions
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(conversation_id)
            .unwrap_or_default()
    }

    pub fn provider_for(&self, endpoint_id: &str) -> Result<Arc<dyn Provider>, CmdError> {
        self.providers
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(endpoint_id)
            .cloned()
            .ok_or_else(|| CmdError::internal(format!("unknown endpoint: {endpoint_id}")))
    }

    /// Rebuild the provider registry from the endpoint profile table plus the
    /// built-in local Ollama (called when profiles change or the Ollama base
    /// URL setting changes).
    pub async fn rebuild_providers(&self) {
        let profiles = self.db.list_endpoint_profiles().await.unwrap_or_default();
        let base = self.settings.get_or(
            crate::settings::keys::OLLAMA_BASE_URL,
            "http://127.0.0.1:11434",
        );
        let map = crate::providers::build_providers(&profiles, &base, self.http.clone());
        *self.providers.write().unwrap_or_else(|p| p.into_inner()) = map;
    }
}
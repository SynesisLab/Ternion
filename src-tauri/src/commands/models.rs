//! Model discovery across all enabled endpoint profiles (design §5.1/§5.4).
//! The built-in local Ollama is probed first and keeps bare ids (every pre-M3
//! pin/role reference stays valid); profile models get `"model@endpoint"`
//! references. A down profile degrades to a warning — the picker still shows
//! every endpoint that answered.

use std::sync::Arc;

use tauri::State;

use crate::{
    error::CmdError,
    providers::{qualified_model_ref, Provider},
    state::{AppState, DEFAULT_ENDPOINT},
    types::ModelInfo,
};

#[tauri::command]
pub async fn list_models(state: State<'_, AppState>) -> Result<Vec<ModelInfo>, CmdError> {
    let mut all: Vec<ModelInfo> = Vec::new();

    if let Ok(provider) = state.provider_for(DEFAULT_ENDPOINT) {
        match provider.list_models().await {
            Ok(models) => all.extend(models.into_iter().map(|mut m| {
                m.endpoint_id = DEFAULT_ENDPOINT.to_string();
                m
            })),
            Err(e) => log::warn!("builtin model discovery failed: {e}"),
        }
    }

    for profile in state
        .db
        .list_endpoint_profiles()
        .await?
        .into_iter()
        .filter(|p| p.enabled)
    {
        let provider: Arc<dyn Provider> = match state.provider_for(&profile.id) {
            Ok(p) => p,
            Err(_) => continue,
        };
        match provider.list_models().await {
            Ok(models) => all.extend(models.into_iter().map(|mut m| {
                m.id = qualified_model_ref(&profile.id, &m.id);
                m.endpoint_id = profile.id.clone();
                m
            })),
            Err(e) => log::warn!(
                "model discovery for '{}' ({}) failed: {e}",
                profile.name,
                profile.id
            ),
        }
    }

    // Cache capability facts for the policy engine's hard rules (§5.4).
    state.update_model_registry(all.clone());
    Ok(all)
}
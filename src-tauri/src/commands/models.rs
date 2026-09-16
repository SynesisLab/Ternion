//! Model discovery commands (fed to the header picker / StatusChip).

use std::sync::Arc;

use tauri::State;

use crate::{
    error::CmdError,
    providers::Provider,
    state::{AppState, DEFAULT_ENDPOINT},
    types::ModelInfo,
};

#[tauri::command]
pub async fn list_models(state: State<'_, AppState>) -> Result<Vec<ModelInfo>, CmdError> {
    let provider: Arc<dyn Provider> = state.provider_for(DEFAULT_ENDPOINT)?;
    let models = provider.list_models().await.map_err(CmdError::from)?;
    // Cache capability facts for the policy engine's hard rules (§5.4).
    state.update_model_registry(models.clone());
    Ok(models)
}
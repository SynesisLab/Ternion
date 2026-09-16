//! Settings read/write. The cache is updated on set so hot paths see the new
//! value immediately; the DB row is the durable source of truth.

use tauri::State;

use crate::{error::CmdError, state::AppState};

#[tauri::command]
pub async fn get_setting(
    state: State<'_, AppState>,
    key: String,
) -> Result<Option<String>, CmdError> {
    Ok(state.settings.get(&key))
}

#[tauri::command]
pub async fn set_setting(
    state: State<'_, AppState>,
    key: String,
    value: String,
) -> Result<(), CmdError> {
    state.db.set_setting(&key, &value).await?;
    state.settings.set(&key, value);
    Ok(())
}
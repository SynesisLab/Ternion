//! Conversation + message read commands.

use tauri::State;

use crate::{
    error::CmdError,
    ids,
    state::AppState,
    types::{Conversation, Message},
};

#[tauri::command]
pub async fn list_conversations(
    state: State<'_, AppState>,
) -> Result<Vec<Conversation>, CmdError> {
    state.db.list_conversations().await
}

#[tauri::command]
pub async fn create_conversation(
    state: State<'_, AppState>,
) -> Result<Conversation, CmdError> {
    state.db.create_conversation(ids::new_id(), ids::now_ms()).await
}

#[tauri::command]
pub async fn rename_conversation(
    state: State<'_, AppState>,
    id: String,
    title: String,
) -> Result<(), CmdError> {
    state.db.rename_conversation(id, title).await
}

#[tauri::command]
pub async fn delete_conversation(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), CmdError> {
    state.db.delete_conversation(id).await
}

/// Per-chat model selection (M0: explicit model id in `pinned_model`).
#[tauri::command]
pub async fn set_conversation_model(
    state: State<'_, AppState>,
    id: String,
    model: Option<String>,
) -> Result<(), CmdError> {
    state.db.set_conversation_model(id, model).await
}

/// Last 1000 messages, ascending (virtualized pagination arrives in M4).
#[tauri::command]
pub async fn get_messages(
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<Vec<Message>, CmdError> {
    state.db.get_messages(conversation_id).await
}
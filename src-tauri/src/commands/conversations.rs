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

/// Bind 0–3 workspace roots (design §6.3). Each root is canonicalized at
/// bind time and must be an existing directory; FS tools in bound chats
/// operate only inside these roots.
#[tauri::command]
pub async fn set_conversation_workspaces(
    state: State<'_, AppState>,
    id: String,
    roots: Vec<String>,
) -> Result<(), CmdError> {
    if roots.len() > 3 {
        return Err(CmdError::internal("a chat can bind at most 3 workspaces"));
    }
    let mut canonical: Vec<String> = Vec::with_capacity(roots.len());
    for root in &roots {
        let path = crate::tools::guard::normalize_root(root)
            .map_err(|e| CmdError::new("workspace_invalid", e.to_string()))?;
        let s = path.to_string_lossy().to_string();
        if !canonical.contains(&s) {
            canonical.push(s);
        }
    }
    state.db.set_conversation_workspaces(id, canonical).await
}

/// Last 1000 messages, ascending (virtualized pagination arrives in M4).
#[tauri::command]
pub async fn get_messages(
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<Vec<Message>, CmdError> {
    state.db.get_messages(conversation_id).await
}
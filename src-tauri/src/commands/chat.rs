//! Streaming chat commands: `chat_send` (long-running, resolves when the
//! stream ends; events flow through the `Channel`) and `chat_stop`.

use serde::Deserialize;
use tauri::{ipc::Channel, State};

use crate::{
    chat,
    error::CmdError,
    state::AppState,
    types::{ChatSendResult, RoutingEvent, StreamEvent},
};

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ChatSendArgs {
    pub conversation_id: String,
    /// Client-generated id; makes retries idempotent (design §3.2).
    pub user_message_id: String,
    pub content: String,
    /// Model *reference* (§5.1): bare `"gemma3:4b"` targets the built-in
    /// local Ollama; `"model@endpoint_id"` targets an endpoint profile.
    pub model: String,
}

#[tauri::command]
pub async fn chat_send(
    state: State<'_, AppState>,
    args: ChatSendArgs,
    on_event: Channel<StreamEvent>,
) -> Result<ChatSendResult, CmdError> {
    let forward = move |ev: &StreamEvent| {
        // Ignore send errors: a closed webview must not kill the stream.
        let _ = on_event.send(ev.clone());
    };
    chat::send(&state, args, std::sync::Arc::new(forward)).await
}

#[tauri::command]
pub async fn chat_stop(
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<(), CmdError> {
    let entry = state
        .streams
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .get(&conversation_id)
        .cloned();
    if let Some(entry) = entry {
        entry.cancel.cancel();
    }
    Ok(())
}

/// Router log drawer (§9.2): recent decisions for one conversation, newest
/// first, capped at 100.
#[tauri::command]
pub async fn list_routing_events(
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<Vec<RoutingEvent>, CmdError> {
    state.db.list_routing_events(conversation_id, 100).await
}

/// Follow-up suggestion chips (§3.7): returns and clears the set the Herald
/// sidecar produced for the last exchange (empty until it lands).
#[tauri::command]
pub async fn take_suggestions(
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<Vec<String>, CmdError> {
    Ok(state.take_suggestions(&conversation_id))
}
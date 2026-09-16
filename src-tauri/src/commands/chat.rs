//! Streaming chat commands: `chat_send` (long-running, resolves when the
//! stream ends; events flow through the `Channel`) and `chat_stop`.

use serde::Deserialize;
use tauri::{ipc::Channel, State};

use crate::{
    chat,
    error::CmdError,
    state::AppState,
    types::{ChatSendResult, StreamEvent},
};

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ChatSendArgs {
    pub conversation_id: String,
    /// Client-generated id; makes retries idempotent (design §3.2).
    pub user_message_id: String,
    pub content: String,
    /// Explicit model id in M0 (no "auto" routing until M1).
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
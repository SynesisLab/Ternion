//! Streaming chat commands: `chat_send` (long-running, resolves when the
//! stream ends; events flow through the `Channel`) and `chat_stop`.

use serde::Deserialize;
use tauri::{ipc::Channel, State};

use crate::{
    chat,
    error::CmdError,
    state::AppState,
    types::{ChatSendResult, RoutingEvent, StreamEvent, TriadReport},
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
    /// Attachment rows (§7.2) the composer saved before sending; they are
    /// linked to the user message and ride the request as image parts.
    #[serde(default)]
    pub attachment_ids: Vec<String>,
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

/// Settings → Triad report (§3.11): aggregates over every routing event and
/// completed assistant message, plus the learned §3.11 threshold bumps.
#[tauri::command]
pub async fn triad_report(state: State<'_, AppState>) -> Result<TriadReport, CmdError> {
    use crate::settings::keys;
    use crate::types::AdaptiveBump;

    let mut report = state.db.triad_report().await?;
    report.adaptive = state
        .settings
        .prefix(keys::ADAPTIVE_BUMP_PREFIX)
        .into_iter()
        .filter_map(|(k, v)| {
            let flag = k.strip_prefix(keys::ADAPTIVE_BUMP_PREFIX)?.to_string();
            let delta: f32 = v.parse().ok()?;
            let overrides = state
                .settings
                .get(&keys::adaptive_count_key(&flag))
                .and_then(|c| c.parse().ok())
                .unwrap_or(0);
            Some(AdaptiveBump {
                flag,
                delta,
                overrides,
            })
        })
        .filter(|b| b.delta.abs() > f32::EPSILON)
        .collect();
    Ok(report)
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
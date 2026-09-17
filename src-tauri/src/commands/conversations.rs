//! Conversation + message read commands.

use tauri::State;

use crate::{
    error::CmdError,
    ids,
    router::config::{classes_of, Pin, TriadConfig},
    settings::{self, keys},
    state::AppState,
    types::{Conversation, Message, Target},
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
/// With §3.11 adaptive tuning on, a pin that contradicts the conversation's
/// latest auto decision also nudges that flag class's escalation threshold.
#[tauri::command]
pub async fn set_conversation_model(
    state: State<'_, AppState>,
    id: String,
    model: Option<String>,
) -> Result<(), CmdError> {
    state.db.set_conversation_model(id.clone(), model.clone()).await?;
    learn_adaptive(&state, id, model.as_deref()).await;
    Ok(())
}

/// §3.11 (experimental): a pin whose target differs from the latest auto
/// decision is a verdict on the router for that flag class — Scout pins
/// raise the escalation threshold (Herald escalated too eagerly), Titan
/// pins lower it. ±0.05 per event, clamped to [−0.10, +0.20]. Best effort:
/// learning failures never block the pin.
async fn learn_adaptive(state: &AppState, conversation_id: String, model: Option<&str>) {
    let cfg = TriadConfig::load(&state.settings);
    if !cfg.adaptive_enabled {
        return;
    }
    let pin_target = match Pin::parse(model) {
        Pin::Role(t) => Some(t),
        Pin::Model(m) => cfg.roles.role_of(&m),
        Pin::Auto => None,
    };
    let (Some(pin_target), Ok(Some(last))) = (
        pin_target,
        state.db.latest_auto_decision(conversation_id.to_string()).await,
    ) else {
        return;
    };
    if pin_target == last.target {
        return;
    }
    let delta = if pin_target == Target::Scout { 0.05 } else { -0.05 };
    for class in classes_of(&last.flags) {
        let key = keys::adaptive_bump_key(class);
        let current = state
            .settings
            .get(&key)
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.0);
        let next = (current + delta).clamp(-0.10, 0.20);
        let value = format!("{next:.2}");
        if state.db.set_setting(&key, &value).await.is_ok() {
            state.settings.set(&key, value);
        }
        let count_key = keys::adaptive_count_key(class);
        let count = state
            .settings
            .get(&count_key)
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0)
            + 1;
        if state
            .db
            .set_setting(&count_key, &count.to_string())
            .await
            .is_ok()
        {
            state.settings.set(&count_key, count.to_string());
        }
    }
    log::info!("adaptive: {delta:+} threshold for {:?} after pin override", classes_of(&last.flags));
}

/// Clear every learned bump and its override counter (report reset button).
#[tauri::command]
pub async fn reset_adaptive(state: State<'_, AppState>) -> Result<(), CmdError> {
    state
        .db
        .delete_settings_with_prefix(settings::keys::ADAPTIVE_BUMP_PREFIX.to_string())
        .await?;
    state
        .db
        .delete_settings_with_prefix(settings::keys::ADAPTIVE_COUNT_PREFIX.to_string())
        .await?;
    for (k, _) in state.settings.prefix(settings::keys::ADAPTIVE_BUMP_PREFIX) {
        state.settings.remove(&k);
    }
    for (k, _) in state.settings.prefix(settings::keys::ADAPTIVE_COUNT_PREFIX) {
        state.settings.remove(&k);
    }
    Ok(())
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
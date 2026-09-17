//! §6.6 permission-matrix IPC: resolve a pending approval, read the
//! persisted "always" grants, and manage modes.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{error::CmdError, state::AppState, types::ApprovalReply};

/// Resolve a pending ApprovalRequest. Unknown or already-resolved ids are
/// ignored (the executor denies on a dropped channel anyway).
#[tauri::command]
pub async fn respond_approval(
    state: tauri::State<'_, Arc<AppState>>,
    request_id: String,
    reply: ApprovalReply,
) -> Result<(), CmdError> {
    let mut approvals = state.approvals.lock().await;
    if let Some(tx) = approvals.remove(&request_id) {
        let _ = tx.send(reply);
    }
    Ok(())
}

/// One persisted "always" grant, for the settings list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolPermissionRow {
    pub tool: String,
    pub root: String,
    pub mode: String,
}

#[tauri::command]
pub async fn list_tool_permissions(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<Vec<ToolPermissionRow>, CmdError> {
    let rows = state.db.list_tool_permissions().await?;
    Ok(rows
        .into_iter()
        .map(|(tool, root, mode)| ToolPermissionRow { tool, root, mode })
        .collect())
}

/// Set a (tool × root) mode: "always" persists, "ask" clears the row.
#[tauri::command]
pub async fn set_tool_permission(
    state: tauri::State<'_, Arc<AppState>>,
    tool: String,
    root: String,
    mode: String,
) -> Result<(), CmdError> {
    state.db.set_tool_permission(tool, root, mode).await
}

/// Drop every in-session "allow" grant (the Session column's reset button).
#[tauri::command]
pub async fn clear_session_permissions(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<(), CmdError> {
    state.session_grants.lock().await.clear();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_permission_row_serializes_camel_case() {
        let v = serde_json::to_value(ToolPermissionRow {
            tool: "fs_write".into(),
            root: r"C:\w".into(),
            mode: "always".into(),
        })
        .unwrap();
        assert_eq!(v["tool"], "fs_write");
        assert_eq!(v["mode"], "always");
    }
}
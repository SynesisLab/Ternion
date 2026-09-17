//! OpenWebUI tools CRUD + manifest test (design §6.5b). Execution is
//! stateless — every call re-reads the row and re-parses the manifest — so
//! unlike MCP there is no session cache to invalidate.

use tauri::State;

use crate::{
    error::CmdError,
    ids,
    mcp::sanitize_server_name,
    owui::parse_manifest,
    state::AppState,
    types::{OwuiTool, OwuiTestResult},
};

#[tauri::command]
pub async fn list_owui_tools(state: State<'_, AppState>) -> Result<Vec<OwuiTool>, CmdError> {
    state.db.list_owui_tools().await
}

#[tauri::command]
pub async fn save_owui_tool(
    state: State<'_, AppState>,
    tool: OwuiTool,
) -> Result<OwuiTool, CmdError> {
    let mut tool = tool;
    tool.name = tool.name.trim().to_string();
    validate(&tool)?;

    // The namespace keys on the sanitized name — two manifests sanitizing
    // the same would collide in the tool list and in `owui__<name>__<tool>`.
    let sanitized = sanitize_server_name(&tool.name);
    if sanitized.is_empty() {
        return Err(CmdError::new(
            "invalid_tool",
            "Name must contain at least one letter or digit",
        ));
    }
    for existing in state.db.list_owui_tools().await? {
        if existing.id != tool.id && sanitize_server_name(&existing.name) == sanitized {
            return Err(CmdError::new(
                "invalid_tool",
                format!("Another tool already uses the name `{}`", existing.name),
            ));
        }
    }

    // The source must be a parseable manifest — reject garbage at save time
    // rather than silently skipping the tool at every send.
    if let Err(e) = parse_manifest(&tool.source) {
        return Err(CmdError::new("invalid_tool", format!("Manifest error: {e}")));
    }

    if tool.id.trim().is_empty() {
        tool.id = format!("owui_{}", ids::new_id());
    }
    state.db.upsert_owui_tool(tool.clone(), ids::now_ms()).await?;
    Ok(tool)
}

#[tauri::command]
pub async fn delete_owui_tool(state: State<'_, AppState>, id: String) -> Result<(), CmdError> {
    state.db.delete_owui_tool(id).await?;
    Ok(())
}

/// Manifest test (§6.5b): parse the class and probe for a Python
/// interpreter. Method names surface even without an interpreter so the
/// user can see what would load once one is available.
#[tauri::command]
pub async fn test_owui_tool(source: String) -> Result<OwuiTestResult, CmdError> {
    Ok(crate::owui::test_connect(source.trim()).await)
}

fn validate(tool: &OwuiTool) -> Result<(), CmdError> {
    if tool.name.trim().is_empty() {
        return Err(CmdError::new("invalid_tool", "Name is required"));
    }
    if tool.source.trim().is_empty() {
        return Err(CmdError::new("invalid_tool", "Source is required"));
    }
    Ok(())
}
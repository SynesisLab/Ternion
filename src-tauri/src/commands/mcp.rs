//! MCP server CRUD + connection test (design §6.5). Saving, deleting, or
//! toggling a server drops its cached session so the next use re-spawns with
//! the new configuration.

use tauri::State;

use crate::{
    error::CmdError,
    ids,
    mcp::{sanitize_server_name, McpManager},
    state::AppState,
    types::{McpServer, McpTestResult},
};

#[tauri::command]
pub async fn list_mcp_servers(state: State<'_, AppState>) -> Result<Vec<McpServer>, CmdError> {
    state.db.list_mcp_servers().await
}

#[tauri::command]
pub async fn save_mcp_server(
    state: State<'_, AppState>,
    server: McpServer,
) -> Result<McpServer, CmdError> {
    let mut server = server;
    server.name = server.name.trim().to_string();
    server.command = server.command.trim().to_string();
    validate(&server)?;

    // The namespace keys on the sanitized name — two servers sanitizing the
    // same would collide in the tool list and in `mcp__<name>__<tool>`.
    let sanitized = sanitize_server_name(&server.name);
    if sanitized.is_empty() {
        return Err(CmdError::new(
            "invalid_server",
            "Name must contain at least one letter or digit",
        ));
    }
    for existing in state.db.list_mcp_servers().await? {
        if existing.id != server.id
            && sanitize_server_name(&existing.name) == sanitized
        {
            return Err(CmdError::new(
                "invalid_server",
                format!("Another server already uses the name `{}`", existing.name),
            ));
        }
    }

    if server.id.trim().is_empty() {
        server.id = format!("mcp_{}", ids::new_id());
    }
    let id = server.id.clone();
    state
        .db
        .upsert_mcp_server(server.clone(), ids::now_ms())
        .await?;
    state.mcp.invalidate(Some(&id)).await;
    Ok(server)
}

#[tauri::command]
pub async fn delete_mcp_server(state: State<'_, AppState>, id: String) -> Result<(), CmdError> {
    state.db.delete_mcp_server(id.clone()).await?;
    state.mcp.invalidate(Some(&id)).await;
    Ok(())
}

/// Connection test (§5.1's test, for MCP): spawn, initialize, tools/list —
/// bare tool names surfaced inline. Unreachable servers return `ok: false`.
#[tauri::command]
pub async fn test_mcp_server(command: String) -> Result<McpTestResult, CmdError> {
    match McpManager::test_connect(command.trim()).await {
        Ok((latency_ms, tools)) => Ok(McpTestResult {
            ok: true,
            latency_ms,
            tools,
            error: None,
        }),
        Err(e) => Ok(McpTestResult {
            ok: false,
            latency_ms: 0,
            tools: Vec::new(),
            error: Some(e),
        }),
    }
}

fn validate(server: &McpServer) -> Result<(), CmdError> {
    if server.name.trim().is_empty() {
        return Err(CmdError::new("invalid_server", "Name is required"));
    }
    if server.command.trim().is_empty() {
        return Err(CmdError::new("invalid_server", "Command is required"));
    }
    Ok(())
}
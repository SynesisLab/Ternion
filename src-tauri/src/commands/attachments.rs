//! Attachment entry points (design §7.1/§7.2): clipboard-paste bytes and
//! drag-and-drop / picked file paths both land here, flow through the IMG
//! pipeline, and persist as rows keyed by content hash (dedupe, §8.1).
//! Preview rendering happens via the asset protocol scoped to the
//! attachments dir.

use base64::Engine;
use serde::Deserialize;
use tauri::State;

use crate::{
    error::CmdError,
    ids,
    img,
    state::AppState,
    types::Attachment,
};

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SaveAttachmentArgs {
    /// Image bytes, base64 (clipboard paste / file picker path).
    pub data_base64: String,
    /// Original file name when known — kept only for error messages.
    #[serde(default)]
    pub source_name: Option<String>,
}

async fn save_bytes(state: &State<'_, AppState>, raw: &[u8]) -> Result<Attachment, CmdError> {
    let stored = img::store(&state.attachments_dir, raw)
        .map_err(|e| CmdError::new("attachment", e))?;
    let att = Attachment {
        id: stored.id,
        message_id: None,
        kind: "image".into(),
        path: stored.path,
        processed_path: stored.processed_path,
        mime: stored.mime,
        width: stored.width,
        height: stored.height,
        bytes: stored.bytes,
        sha256: stored.sha256,
        created_at: ids::now_ms(),
    };
    state.db.insert_attachment(att.clone()).await?;
    Ok(att)
}

#[tauri::command]
pub async fn save_attachment(
    state: State<'_, AppState>,
    args: SaveAttachmentArgs,
) -> Result<Attachment, CmdError> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(args.data_base64.as_bytes())
        .map_err(|e| CmdError::new("invalid_attachment", format!("bad base64: {e}")))?;
    save_bytes(&state, &raw).await
}

/// Drag-and-drop hands the frontend a real path; reading stays in Rust so
/// the webview never sees file contents.
#[tauri::command]
pub async fn save_attachment_file(
    state: State<'_, AppState>,
    path: String,
) -> Result<Attachment, CmdError> {
    let raw = std::fs::read(&path)
        .map_err(|e| CmdError::new("invalid_attachment", format!("read {path}: {e}")))?;
    save_bytes(&state, &raw).await
}
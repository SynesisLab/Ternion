//! IPC command surface (registered in lib.rs via generate_handler!).

/// Dev/IPC smoke test: returns the app version so the frontend can prove the
/// bridge works before any data flows (step 4 checkpoint).
#[tauri::command]
pub fn ping(app: tauri::AppHandle) -> Result<String, crate::error::CmdError> {
    Ok(app.package_info().version.to_string())
}
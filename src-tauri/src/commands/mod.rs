//! IPC command surface (registered in lib.rs via generate_handler!).
//!
//! Submodules define the commands; the glob re-exports below make both the
//! command fns and Tauri's macro-generated `__cmd__` helpers reachable at
//! `commands::<name>`, which is what `generate_handler!` requires.

pub mod attachments;
pub mod chat;
pub mod conversations;
pub mod endpoints;
pub mod models;
pub mod permissions;
pub mod settings;

pub use attachments::*;
pub use chat::*;
pub use conversations::*;
pub use endpoints::*;
pub use models::*;
pub use permissions::*;
pub use settings::*;

use crate::error::CmdError;

/// Dev/IPC smoke test: returns the app version so the frontend can prove the
/// bridge works before any data flows (step 4 checkpoint).
#[tauri::command]
pub fn ping(app: tauri::AppHandle) -> Result<String, CmdError> {
    Ok(app.package_info().version.to_string())
}

/// Open a URL in the system browser (markdown links must not navigate the
/// webview away).
#[tauri::command]
pub fn open_external(app: tauri::AppHandle, url: String) -> Result<(), CmdError> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| CmdError::internal(format!("open url: {e}")))
}

/// Forward a webview-side error/boot marker into the app log — release
/// builds have no devtools, so this is the only way JS failures surface.
#[tauri::command]
pub fn log_js(level: String, message: String) {
    match level.as_str() {
        "error" => log::error!(target: "webview", "{message}"),
        _ => log::info!(target: "webview", "{message}"),
    }
}
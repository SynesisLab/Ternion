//! IPC command surface (registered in lib.rs via generate_handler!).
//!
//! Submodules define the commands; the glob re-exports below make both the
//! command fns and Tauri's macro-generated `__cmd__` helpers reachable at
//! `commands::<name>`, which is what `generate_handler!` requires.

pub mod conversations;
pub mod settings;

pub use conversations::*;
pub use settings::*;

use crate::error::CmdError;

/// Dev/IPC smoke test: returns the app version so the frontend can prove the
/// bridge works before any data flows (step 4 checkpoint).
#[tauri::command]
pub fn ping(app: tauri::AppHandle) -> Result<String, CmdError> {
    Ok(app.package_info().version.to_string())
}
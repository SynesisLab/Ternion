//! Quick capture window (design §9.3): `Win+Alt+T` opens a small
//! always-on-top palette that shares the main bundle — it sends through the
//! full Triad pipeline (chat_send) and can hand the exchange back to the
//! main window ("open in main window").

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::error::CmdError;

/// Quick window geometry (CSS px): small palette, top-right of whichever
/// monitor the cursor is on.
const QUICK_WIDTH: f64 = 420.0;
const QUICK_HEIGHT: f64 = 560.0;
const SCREEN_MARGIN: f64 = 24.0;

/// Open (or re-focus) the quick-ask window near the top-right of the monitor
/// under the cursor.
pub fn open_quick_window(app: &AppHandle) {
    if let Some(existing) = app.get_webview_window("quick") {
        let _ = existing.show();
        let _ = existing.set_focus();
        return;
    }
    let Some(monitor) = super::capture::target_monitor(app) else {
        log::warn!("quick capture: no monitor found");
        return;
    };
    let scale = monitor.scale_factor();
    let pos = monitor.position();
    let size = monitor.size();
    let x = (pos.x + size.width as i32) as f64 / scale - QUICK_WIDTH - SCREEN_MARGIN;
    let y = pos.y as f64 / scale + SCREEN_MARGIN;
    let result = WebviewWindowBuilder::new(
        app,
        "quick",
        WebviewUrl::App("index.html".into()),
    )
    .title("Ternion quick ask")
    .decorations(false)
    .shadow(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .focused(true)
    .position(x, y)
    .inner_size(QUICK_WIDTH, QUICK_HEIGHT)
    .build();
    if let Err(e) = result {
        log::warn!("quick capture window failed to open: {e}");
    }
}

/// Hand an exchange over to the main window (§9.3 "open in main window"):
/// surface + focus it, then tell its store to select the conversation.
#[tauri::command]
pub async fn open_in_main(
    app: AppHandle,
    conversation_id: String,
) -> Result<(), CmdError> {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    let _ = app.emit_to("main", "ternion://open-conversation", conversation_id);
    Ok(())
}

/// Start a §7.1 region capture on behalf of `target` ("main" | "quick") —
/// the stored attachment is announced to that window's composer.
#[tauri::command]
pub async fn start_capture(app: AppHandle, target: String) -> Result<(), CmdError> {
    if target != "main" && target != "quick" {
        return Err(CmdError::new(
            "capture",
            format!("unknown capture target: {target}"),
        ));
    }
    super::capture::open_capture_overlay(&app, &target);
    Ok(())
}
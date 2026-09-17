//! Region capture commands (design §7.1): the global hotkey opens a
//! transparent overlay webview ("capture"); the drag selection is captured
//! with GDI, pushed through the IMG pipeline, and announced to the main
//! window's composer as an event so it lands in the pending chips.

use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::{error::CmdError, ids, state::AppState, types::Attachment};

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CaptureRegionArgs {
    /// Selection in overlay-relative *physical* pixels (CSS × devicePixelRatio).
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Open (or re-focus) the capture overlay over the monitor under the cursor.
/// `target` names the window the finished attachment is announced to
/// ("main" composer chips or the quick-capture window, §9.3).
pub fn open_capture_overlay(app: &AppHandle, target: &str) {
    {
        let state = app.state::<AppState>();
        *state
            .capture_target
            .write()
            .unwrap_or_else(|p| p.into_inner()) = target.to_string();
    }
    if let Some(existing) = app.get_webview_window("capture") {
        let _ = existing.show();
        let _ = existing.set_focus();
        return;
    }
    let Some(monitor) = target_monitor(app) else {
        log::warn!("region capture: no monitor found");
        return;
    };
    let scale = monitor.scale_factor();
    let pos = monitor.position();
    let size = monitor.size();
    let result = WebviewWindowBuilder::new(
        app,
        "capture",
        WebviewUrl::App("index.html".into()),
    )
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .focused(true)
    .position(pos.x as f64 / scale, pos.y as f64 / scale)
    .inner_size(size.width as f64 / scale, size.height as f64 / scale)
    .build();
    if let Err(e) = result {
        log::warn!("capture overlay failed to open: {e}");
    }
}

/// The monitor the cursor is on (fallback: primary) — capture starts where
/// the user is looking, not wherever Windows thinks "primary" is. Shared
/// with the quick-capture window placement (§9.3).
pub(crate) fn target_monitor(app: &AppHandle) -> Option<tauri::Monitor> {
    if let Ok(cursor) = app.cursor_position() {
        let monitors = app.available_monitors().unwrap_or_default();
        for m in monitors {
            let p = m.position();
            let s = m.size();
            let (mx, my) = (p.x, p.y);
            let (mw, mh) = (s.width as i32, s.height as i32);
            if cursor.x >= mx as f64
                && cursor.x < (mx + mw) as f64
                && cursor.y >= my as f64
                && cursor.y < (my + mh) as f64
            {
                return Some(m);
            }
        }
    }
    app.primary_monitor().ok().flatten()
}

/// Capture the drag selection: hide the overlay first (it must not be in
/// the shot), grab the rect, run the IMG pipeline, and tell the main
/// window's composer there's a new attachment pending.
#[tauri::command]
pub async fn capture_region(
    app: AppHandle,
    state: State<'_, AppState>,
    args: CaptureRegionArgs,
) -> Result<Attachment, CmdError> {
    if args.width <= 0 || args.height <= 0 {
        return Err(CmdError::new("attachment", "empty capture region"));
    }
    // Hide before capture and let DWM settle, or the dim overlay shows up
    // in the picture.
    if let Some(win) = app.get_webview_window("capture") {
        let _ = win.hide();
    }
    tokio::time::sleep(std::time::Duration::from_millis(180)).await;

    // The overlay covers one monitor; selection coords are relative to it.
    let (mx, my) = app
        .get_webview_window("capture")
        .and_then(|w| w.current_monitor().ok().flatten())
        .map(|m| (m.position().x, m.position().y))
        .unwrap_or((0, 0));
    let raw = crate::capture::capture_screen_region(
        mx + args.x,
        my + args.y,
        args.width,
        args.height,
    )
    .map_err(|e| CmdError::new("capture", e))?;

    let stored = crate::img::store(&state.attachments_dir, &raw)
        .map_err(|e| CmdError::new("attachment", e))?;
    let att = Attachment {
        id: stored.id.clone(),
        message_id: None,
        kind: "image".into(),
        path: stored.path.clone(),
        processed_path: stored.processed_path.clone(),
        mime: stored.mime.clone(),
        width: stored.width,
        height: stored.height,
        bytes: stored.bytes,
        sha256: stored.sha256.clone(),
        created_at: ids::now_ms(),
    };
    state.db.insert_attachment(att.clone()).await?;

    // The overlay closes itself after this resolves; the composer chip
    // arrives via the event in whichever window started the capture
    // (§7.1 falls back to the clipboard flow, which is just Ctrl+V on the
    // pasted screenshot).
    let target = state
        .capture_target
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    let _ = app.emit_to(target.as_str(), "ternion://capture-attached", &att);
    if let Some(win) = app.get_webview_window("capture") {
        let _ = win.close();
    }
    Ok(att)
}
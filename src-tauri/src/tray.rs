//! System tray (design §7): Show/Hide · New Chat · Quick ask · Screenshot ·
//! Local-only toggle · Quit. Left-click focuses the window instead of
//! opening the menu.

use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};

pub fn setup(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    use crate::settings::keys;

    let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, "hide", "Hide", true, None::<&str>)?;
    let new_chat = MenuItem::with_id(app, "new_chat", "New Chat", true, None::<&str>)?;
    let quick_ask = MenuItem::with_id(app, "quick_ask", "Quick ask", true, None::<&str>)?;
    let screenshot =
        MenuItem::with_id(app, "screenshot", "Screenshot", true, None::<&str>)?;
    // §3.10 privacy mode toggle, mirrored in Settings.
    let local_only_on = app
        .state::<crate::state::AppState>()
        .settings
        .get(keys::PRIVACY_LOCAL_ONLY)
        .map(|v| v == "true")
        .unwrap_or(false);
    let local_only = CheckMenuItem::with_id(
        app,
        "local_only",
        "Local-only mode",
        true,
        local_only_on,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[
            &show,
            &hide,
            &sep1,
            &new_chat,
            &quick_ask,
            &screenshot,
            &local_only,
            &sep2,
            &quit,
        ],
    )?;

    let icon = app
        .default_window_icon()
        .cloned()
        .expect("tauri bundles a default window icon");

    let local_only_item = local_only.clone();
    TrayIconBuilder::with_id("ternion-tray")
        .icon(icon)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("Ternion")
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "show" => show_window(app),
            "hide" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            // The frontend listens on this and opens a fresh conversation.
            "new_chat" => {
                show_window(app);
                let _ = app.emit("ternion://new-chat", ());
            }
            // §9.3 tray actions: quick-ask palette and the capture overlay.
            "quick_ask" => crate::commands::quick::open_quick_window(app),
            "screenshot" => {
                crate::commands::capture::open_capture_overlay(app, "main");
            }
            // §3.10: flip + persist privacy mode; the check follows.
            "local_only" => {
                let state = app.state::<crate::state::AppState>();
                let next = !state
                    .settings
                    .get(keys::PRIVACY_LOCAL_ONLY)
                    .map(|v| v == "true")
                    .unwrap_or(false);
                let value = if next { "true" } else { "false" };
                let written = state.db.with_conn_sync(|c| {
                    c.execute(
                        "INSERT INTO settings (key, value) VALUES (?1, ?2)
                         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                        rusqlite::params![keys::PRIVACY_LOCAL_ONLY, value],
                    )
                });
                match written {
                    Ok(_) => {
                        state
                            .settings
                            .set(keys::PRIVACY_LOCAL_ONLY, value.to_string());
                        let _ = local_only_item.set_checked(next);
                    }
                    Err(e) => log::warn!("local-only toggle write failed: {e}"),
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

fn show_window(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}
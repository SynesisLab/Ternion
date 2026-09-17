//! System tray (design §7): Show/Hide · New Chat · Quick ask · Screenshot ·
//! Quit. Left-click focuses the window instead of opening the menu.

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};

pub fn setup(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, "hide", "Hide", true, None::<&str>)?;
    let new_chat = MenuItem::with_id(app, "new_chat", "New Chat", true, None::<&str>)?;
    let quick_ask = MenuItem::with_id(app, "quick_ask", "Quick ask", true, None::<&str>)?;
    let screenshot =
        MenuItem::with_id(app, "screenshot", "Screenshot", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[&show, &hide, &sep1, &new_chat, &quick_ask, &screenshot, &sep2, &quit],
    )?;

    let icon = app
        .default_window_icon()
        .cloned()
        .expect("tauri bundles a default window icon");

    TrayIconBuilder::with_id("ternion-tray")
        .icon(icon)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("Ternion")
        .on_menu_event(|app, event| match event.id().as_ref() {
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
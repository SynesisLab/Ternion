mod chat;
mod commands;
mod db;
mod error;
mod ids;
mod providers;
mod settings;
mod state;
mod tray;
mod types;

use std::time::Duration;

use state::AppState;

pub fn run() {
    tauri::Builder::default()
        // Must be the FIRST plugin: it detects the second launch before
        // anything else runs (docs: register before any other plugin).
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            use tauri::Manager;
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(
            tauri_plugin_log::Builder::new()
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                        file_name: Some("ternion".into()),
                    }),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                ])
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::ping,
            commands::open_external,
            commands::chat_send,
            commands::chat_stop,
            commands::list_conversations,
            commands::create_conversation,
            commands::rename_conversation,
            commands::delete_conversation,
            commands::set_conversation_model,
            commands::get_messages,
            commands::list_models,
            commands::get_setting,
            commands::set_setting,
        ])
        .setup(|app| {
            use tauri::Manager;

            let dir = app
                .path()
                .app_data_dir()
                .map_err(|e| -> Box<dyn std::error::Error> { format!("app data dir: {e}").into() })?;
            std::fs::create_dir_all(&dir)
                .map_err(|e| -> Box<dyn std::error::Error> { format!("create data dir: {e}").into() })?;

            let database = db::Database::open(&dir.join("ternion.db"))
                .map_err(|e| -> Box<dyn std::error::Error> { format!("open database: {e}").into() })?;

            // Sweep assistant rows left 'streaming' by a previous crash.
            let swept = database
                .with_conn_sync(|c| {
                    c.execute(
                        "UPDATE messages
                         SET status = 'error', error = COALESCE(error, 'stream interrupted by shutdown')
                         WHERE status = 'streaming'",
                        [],
                    )
                })
                .map_err(|e| -> Box<dyn std::error::Error> { format!("orphan sweep: {e}").into() })?;
            if swept > 0 {
                log::info!("startup sweep: marked {swept} orphaned stream(s) as errored");
            }

            // get_all_settings is async (spawn_blocking); use the sync path during setup.
            let pairs = database
                .with_conn_sync(|c| {
                    let mut stmt = c.prepare("SELECT key, value FROM settings")?;
                    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
                    rows.collect::<Result<Vec<(String, String)>, rusqlite::Error>>()
                })
                .map_err(|e| -> Box<dyn std::error::Error> { format!("load settings: {e}").into() })?;

            let settings = crate::settings::SettingsCache::new(pairs.iter().cloned().collect());
            let base = settings
                .get_or(settings::keys::OLLAMA_BASE_URL, "http://127.0.0.1:11434");

            // Streams idle while a model loads — only a connect timeout.
            let http = reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .build()
                .map_err(|e| -> Box<dyn std::error::Error> { format!("http client: {e}").into() })?;
            let providers = providers::build_providers(&base, http.clone());

            app.manage(AppState::new(database, settings, http, providers));

            tray::setup(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            // Close-to-tray (design §7): the ✕ hides the window while the
            // setting is on; Quit comes from the tray menu.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    use tauri::Manager;
                    let app = window.app_handle();
                    let close_to_tray = app
                        .state::<AppState>()
                        .settings
                        .get_or(settings::keys::APP_CLOSE_TO_TRAY, "true")
                        != "false";
                    if close_to_tray {
                        let _ = window.hide();
                        api.prevent_close();
                    }
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
mod commands;
mod db;
mod error;
mod ids;
mod settings;
mod state;
mod types;

use state::AppState;

pub fn run() {
    tauri::Builder::default()
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
        .invoke_handler(tauri::generate_handler![
            commands::ping,
            commands::list_conversations,
            commands::create_conversation,
            commands::rename_conversation,
            commands::delete_conversation,
            commands::set_conversation_model,
            commands::get_messages,
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

            let db = db::Database::open(&dir.join("ternion.db"))
                .map_err(|e| -> Box<dyn std::error::Error> { format!("open database: {e}").into() })?;

            // Sweep assistant rows left 'streaming' by a previous crash.
            let swept = db
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
            let pairs = db
                .with_conn_sync(|c| {
                    let mut stmt = c.prepare("SELECT key, value FROM settings")?;
                    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
                    rows.collect::<Result<Vec<(String, String)>, rusqlite::Error>>()
                })
                .map_err(|e| -> Box<dyn std::error::Error> { format!("load settings: {e}").into() })?;

            app.manage(AppState {
                db,
                settings: crate::settings::SettingsCache::new(pairs.into_iter().collect()),
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
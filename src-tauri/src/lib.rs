mod audio;
mod config;
#[allow(dead_code, unused_imports)]
mod enhance;
mod error;
mod hotkey;
mod logging;
mod notifications;
#[allow(dead_code, unused_imports)]
mod paste;
mod state;
#[allow(dead_code, unused_imports)]
mod stt;
mod tray;
#[allow(dead_code, unused_imports)]
mod vad;

use std::sync::Mutex;

use tauri::{AppHandle, Manager, Runtime};

use crate::config::schema::AppConfig;
use crate::state::{AppEvent, SharedAppState};

/// Применяет событие к state machine, обновляет tray и отправляет уведомление.
pub(crate) fn dispatch_and_update<R: Runtime>(app: &AppHandle<R>, event: AppEvent) {
    let shared = app.state::<SharedAppState>();
    let (old, new) = shared.dispatch_with_old(&event);

    if old == new {
        return;
    }

    tray::update_tray(app, new);
    notifications::notify_state_change(app, old, new);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    logging::init_logging();

    tracing::info!("VoiceDictator starting");

    let app_config = config::storage::load_config().unwrap_or_else(|e| {
        tracing::error!(error = %e, "failed to load config, using defaults");
        AppConfig::default()
    });

    let recording_mode = app_config.recording_mode.clone();
    let hotkey_str = app_config.hotkey.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(hotkey::on_shortcut_event)
                .build(),
        )
        .manage(SharedAppState::new(recording_mode))
        .manage(Mutex::new(app_config))
        .setup(move |app| {
            tray::create_tray(app)?;

            if let Err(e) = hotkey::register_hotkey(app.handle(), &hotkey_str) {
                tracing::error!(error = %e, "failed to register hotkey, tray menu is available as fallback");
                let config_path = crate::config::storage::config_dir()
                    .map(|d| d.join("config.json").display().to_string())
                    .unwrap_or_else(|_| "<config dir unknown>".to_string());
                notifications::notify_error(
                    app.handle(),
                    &format!(
                        "Failed to register hotkey: {}. Use tray menu instead. \
                         Change hotkey in: {}",
                        e, config_path
                    ),
                );
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

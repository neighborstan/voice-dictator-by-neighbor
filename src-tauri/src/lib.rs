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
use std::time::Duration;

use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::GlobalShortcutExt;

use crate::config::schema::AppConfig;
use crate::state::{AppEvent, AppState, SharedAppState};

// --- Tauri commands ---

/// Возвращает текущий конфиг приложения.
#[tauri::command]
fn get_config(config: tauri::State<'_, Mutex<AppConfig>>) -> Result<AppConfig, String> {
    let cfg = config.lock().expect("config mutex poisoned").clone();
    Ok(cfg)
}

/// Сохраняет обновленный конфиг (файл + in-memory state).
#[tauri::command]
fn save_config(
    updated_config: AppConfig,
    config_state: tauri::State<'_, Mutex<AppConfig>>,
    shared_state: tauri::State<'_, SharedAppState>,
) -> Result<(), String> {
    config::storage::save_config(&updated_config).map_err(|e| e.to_string())?;
    shared_state.set_recording_mode(updated_config.recording_mode.clone());
    *config_state.lock().expect("config mutex poisoned") = updated_config;
    Ok(())
}

/// Сбрасывает конфиг в дефолтные значения, возвращает новый конфиг.
#[tauri::command]
fn reset_config(
    config_state: tauri::State<'_, Mutex<AppConfig>>,
    shared_state: tauri::State<'_, SharedAppState>,
) -> Result<AppConfig, String> {
    let defaults = AppConfig::default();
    config::storage::save_config(&defaults).map_err(|e| e.to_string())?;
    shared_state.set_recording_mode(defaults.recording_mode.clone());
    *config_state.lock().expect("config mutex poisoned") = defaults.clone();
    Ok(defaults)
}

/// Проверяет наличие API-ключа в OS keychain.
#[tauri::command]
fn get_has_api_key() -> bool {
    config::secrets::has_api_key()
}

/// Сохраняет API-ключ в OS keychain.
#[tauri::command]
fn save_api_key(key: String) -> Result<(), String> {
    config::secrets::store_api_key(&key).map_err(|e| e.to_string())
}

/// Проверяет валидность API-ключа запросом к OpenAI API.
///
/// Отправляет GET /v1/models с переданным ключом.
/// 200 -> true (валиден), 401 -> false (невалиден), иное -> error.
#[tauri::command]
async fn validate_api_key(
    key: String,
    config: tauri::State<'_, Mutex<AppConfig>>,
) -> Result<bool, String> {
    let base_url = config
        .lock()
        .expect("config mutex poisoned")
        .api_base_url
        .clone();

    let url = format!("{}/v1/models", base_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", key))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;

    match response.status().as_u16() {
        200..=299 => Ok(true),
        401 => Ok(false),
        code => Err(format!("Unexpected API response: {}", code)),
    }
}

/// Перерегистрирует глобальный хоткей (unregister all + register new).
#[tauri::command]
fn update_hotkey(app: AppHandle, hotkey_str: String) -> Result<(), String> {
    app.global_shortcut()
        .unregister_all()
        .map_err(|e| format!("Failed to unregister hotkeys: {}", e))?;
    hotkey::register_hotkey(&app, &hotkey_str)
}

// --- Settings window ---

/// Открывает окно настроек. Если уже открыто - фокусирует существующее.
pub(crate) fn open_settings_window<R: Runtime>(app: &AppHandle<R>) {
    open_settings_window_inner(app, WebviewUrl::App("/settings".into()));
}

/// Открывает окно настроек в режиме онбординга (первый запуск).
fn open_settings_onboarding<R: Runtime>(app: &AppHandle<R>) {
    open_settings_window_inner(app, WebviewUrl::App("/settings?onboarding=1".into()));
}

fn open_settings_window_inner<R: Runtime>(app: &AppHandle<R>, url: WebviewUrl) {
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.unminimize();
        let _ = window.show();
        if let Err(e) = window.set_focus() {
            tracing::warn!(error = %e, "failed to focus settings window");
        }
        return;
    }

    match WebviewWindowBuilder::new(app, "settings", url)
        .title("VoiceDictator - Settings")
        .inner_size(500.0, 620.0)
        .center()
        .resizable(true)
        .build()
    {
        Ok(_) => tracing::info!("settings window opened"),
        Err(e) => tracing::error!(error = %e, "failed to open settings window"),
    }
}

// --- Core dispatch ---

/// Применяет событие к state machine, обновляет tray и отправляет уведомление.
///
/// Перед началом записи проверяет наличие API-ключа. Если ключ не задан,
/// открывает настройки и показывает уведомление.
pub(crate) fn dispatch_and_update<R: Runtime>(app: &AppHandle<R>, event: AppEvent) {
    if matches!(event, AppEvent::HotkeyPressed | AppEvent::HotkeyDown) {
        let shared = app.state::<SharedAppState>();
        if shared.current_state() == AppState::Idle && !config::secrets::has_api_key() {
            notifications::notify_error(app, "Set API key in Settings first");
            open_settings_window(app);
            return;
        }
    }

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
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            reset_config,
            get_has_api_key,
            save_api_key,
            validate_api_key,
            update_hotkey,
        ])
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

            // Onboarding: открыть настройки при первом запуске (нет API-ключа)
            if !config::secrets::has_api_key() {
                open_settings_onboarding(app.handle());
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

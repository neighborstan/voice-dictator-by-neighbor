use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::config::schema::RecordingMode;
use crate::state::{AppEvent, AppState, SharedAppState};

/// Регистрирует глобальный хоткей из строки конфига.
///
/// При ошибке парсинга или регистрации возвращает описание проблемы.
/// Приложение продолжит работать через tray-меню (fallback).
pub fn register_hotkey<R: Runtime>(app: &AppHandle<R>, hotkey_str: &str) -> Result<(), String> {
    let shortcut: Shortcut = hotkey_str
        .parse()
        .map_err(|e| format!("invalid hotkey \"{}\": {}", hotkey_str, e))?;

    app.global_shortcut()
        .register(shortcut)
        .map_err(|e| format!("failed to register hotkey \"{}\": {}", hotkey_str, e))?;

    tracing::info!(hotkey = %hotkey_str, "global hotkey registered");
    Ok(())
}

/// Обработчик события глобального хоткея.
///
/// Определяет AppEvent в зависимости от режима записи (Toggle/PTT)
/// и состояния клавиши (Pressed/Released). Вызывается плагином
/// global-shortcut при каждом срабатывании зарегистрированного хоткея.
pub fn on_shortcut_event<R: Runtime>(
    app: &AppHandle<R>,
    _shortcut: &Shortcut,
    event: tauri_plugin_global_shortcut::ShortcutEvent,
) {
    let shared = app.state::<SharedAppState>();
    let mode = shared.recording_mode();
    let current = shared.current_state();

    // В любом режиме нажатие хоткея во время processing отменяет pipeline.
    let is_processing = matches!(
        current,
        AppState::Transcribing | AppState::Enhancing | AppState::Pasting
    );

    // NOTE: Toggle reacts on Pressed. If a platform only sends Released,
    // the hotkey will appear non-functional -- verify on target OS.
    let app_event = match (&mode, event.state) {
        // Отмена processing независимо от режима записи
        (_, ShortcutState::Pressed) if is_processing => AppEvent::Cancel,
        (_, ShortcutState::Released) if is_processing => return,

        (RecordingMode::Toggle, ShortcutState::Pressed) => AppEvent::HotkeyPressed,
        (RecordingMode::Toggle, ShortcutState::Released) => return,
        (RecordingMode::PushToTalk, ShortcutState::Pressed) => AppEvent::HotkeyDown,
        (RecordingMode::PushToTalk, ShortcutState::Released) => AppEvent::HotkeyUp,
    };

    tracing::debug!(mode = ?mode, event = ?app_event, "hotkey event dispatched");
    crate::dispatch_and_update(app, app_event);
}

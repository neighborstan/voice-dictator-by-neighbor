use tauri::{AppHandle, Manager, Runtime};

use crate::config::schema::AppConfig;
use crate::state::AppState;

/// Отправляет OS-уведомление о смене состояния.
///
/// Проверяет `show_notifications` в конфиге. Если выключено - не отправляет.
/// При ошибке отправки логирует warning, не блокирует pipeline.
pub fn notify_state_change<R: Runtime>(app: &AppHandle<R>, old: AppState, new: AppState) {
    let config = app.state::<std::sync::Mutex<AppConfig>>();
    let show = config
        .lock()
        .expect("config mutex poisoned")
        .show_notifications;
    if !show {
        return;
    }

    let (title, body) = notification_text(old, new);
    if let Err(e) = send_notification(app, title, body) {
        tracing::warn!(error = %e, "failed to send notification");
    }
}

/// Отправляет уведомление об ошибке (всегда, без проверки конфига).
pub fn notify_error<R: Runtime>(app: &AppHandle<R>, message: &str) {
    if let Err(e) = send_notification(app, "VoiceDictator - Error", message) {
        tracing::warn!(error = %e, "failed to send error notification");
    }
}

/// Возвращает (title, body) для уведомления при переходе состояний.
fn notification_text(old: AppState, new: AppState) -> (&'static str, &'static str) {
    match new {
        AppState::Recording => ("VoiceDictator", "Recording started"),
        AppState::Transcribing => ("VoiceDictator", "Transcribing..."),
        AppState::Enhancing => ("VoiceDictator", "Enhancing text..."),
        AppState::Pasting => ("VoiceDictator", "Inserting text..."),
        AppState::Idle if old == AppState::Pasting => ("VoiceDictator", "Text inserted"),
        AppState::Idle if old == AppState::Error => ("VoiceDictator", "Error dismissed"),
        AppState::Idle => ("VoiceDictator", "Processing cancelled"),
        AppState::Error => ("VoiceDictator", "An error occurred"),
    }
}

fn send_notification<R: Runtime>(
    app: &AppHandle<R>,
    title: &str,
    body: &str,
) -> Result<(), String> {
    use tauri_plugin_notification::NotificationExt;

    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_text_should_report_recording_started() {
        // Given / When
        let (title, body) = notification_text(AppState::Idle, AppState::Recording);

        // Then
        assert_eq!(title, "VoiceDictator");
        assert_eq!(body, "Recording started");
    }

    #[test]
    fn notification_text_should_report_text_inserted_after_pasting() {
        // Given / When
        let (title, body) = notification_text(AppState::Pasting, AppState::Idle);

        // Then
        assert_eq!(title, "VoiceDictator");
        assert_eq!(body, "Text inserted");
    }

    #[test]
    fn notification_text_should_report_cancelled_when_idle_from_non_pasting() {
        // Given / When
        let (title, body) = notification_text(AppState::Transcribing, AppState::Idle);

        // Then
        assert_eq!(title, "VoiceDictator");
        assert_eq!(body, "Processing cancelled");
    }

    #[test]
    fn notification_text_should_report_error_dismissed_when_idle_from_error() {
        // Given / When
        let (title, body) = notification_text(AppState::Error, AppState::Idle);

        // Then
        assert_eq!(title, "VoiceDictator");
        assert_eq!(body, "Error dismissed");
    }

    #[test]
    fn notification_text_should_report_error() {
        // Given / When
        let (_, body) = notification_text(AppState::Recording, AppState::Error);

        // Then
        assert_eq!(body, "An error occurred");
    }

    #[test]
    fn all_transitions_should_produce_non_empty_text() {
        let transitions = [
            (AppState::Idle, AppState::Recording),
            (AppState::Recording, AppState::Transcribing),
            (AppState::Transcribing, AppState::Enhancing),
            (AppState::Enhancing, AppState::Pasting),
            (AppState::Pasting, AppState::Idle),
            (AppState::Transcribing, AppState::Idle),
            (AppState::Error, AppState::Idle),
            (AppState::Recording, AppState::Error),
        ];

        for (old, new) in transitions {
            let (title, body) = notification_text(old, new);
            assert!(!title.is_empty(), "empty title for {:?} -> {:?}", old, new);
            assert!(!body.is_empty(), "empty body for {:?} -> {:?}", old, new);
        }
    }
}

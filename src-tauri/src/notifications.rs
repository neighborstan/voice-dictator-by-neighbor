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

    if let Some((title, body)) = notification_text(old, new) {
        if let Err(e) = send_notification(app, title, body) {
            tracing::warn!(error = %e, "failed to send notification");
        }
    }
}

/// Отправляет уведомление об ошибке.
///
/// Всегда показывается, независимо от `show_notifications` в конфиге,
/// так как ошибки критичны и пользователь должен знать о проблеме.
pub fn notify_error<R: Runtime>(app: &AppHandle<R>, message: &str) {
    if let Err(e) = send_notification(app, "VoiceDictator - Error", message) {
        tracing::warn!(error = %e, "failed to send error notification");
    }
}

/// Возвращает (title, body) для уведомления при переходе состояний.
/// Возвращает (title, body) для уведомления при переходе состояний.
///
/// Уведомляем только о ключевых моментах (ТЗ FR-3): Recording started,
/// Text inserted, Processing cancelled, Error. Промежуточные состояния
/// (Transcribing, Enhancing, Pasting) не уведомляют - иначе спам.
/// Возвращает `None` если уведомление не нужно.
fn notification_text(old: AppState, new: AppState) -> Option<(&'static str, &'static str)> {
    match new {
        AppState::Recording => Some(("VoiceDictator", "Recording started")),
        AppState::Idle if old == AppState::Pasting => Some(("VoiceDictator", "Text inserted")),
        AppState::Idle if old == AppState::Error => Some(("VoiceDictator", "Error dismissed")),
        AppState::Idle => Some(("VoiceDictator", "Processing cancelled")),
        AppState::Error => Some(("VoiceDictator", "An error occurred")),
        // Промежуточные: Transcribing, Enhancing, Pasting - без уведомлений
        _ => None,
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
        let result = notification_text(AppState::Idle, AppState::Recording);

        // Then
        let (title, body) = result.expect("should produce notification");
        assert_eq!(title, "VoiceDictator");
        assert_eq!(body, "Recording started");
    }

    #[test]
    fn notification_text_should_report_text_inserted_after_pasting() {
        // Given / When
        let result = notification_text(AppState::Pasting, AppState::Idle);

        // Then
        let (title, body) = result.expect("should produce notification");
        assert_eq!(title, "VoiceDictator");
        assert_eq!(body, "Text inserted");
    }

    #[test]
    fn notification_text_should_report_cancelled_when_idle_from_non_pasting() {
        // Given / When
        let result = notification_text(AppState::Transcribing, AppState::Idle);

        // Then
        let (title, body) = result.expect("should produce notification");
        assert_eq!(title, "VoiceDictator");
        assert_eq!(body, "Processing cancelled");
    }

    #[test]
    fn notification_text_should_report_error_dismissed_when_idle_from_error() {
        // Given / When
        let result = notification_text(AppState::Error, AppState::Idle);

        // Then
        let (title, body) = result.expect("should produce notification");
        assert_eq!(title, "VoiceDictator");
        assert_eq!(body, "Error dismissed");
    }

    #[test]
    fn notification_text_should_report_error() {
        // Given / When
        let result = notification_text(AppState::Recording, AppState::Error);

        // Then
        let (_, body) = result.expect("should produce notification");
        assert_eq!(body, "An error occurred");
    }

    #[test]
    fn notification_text_should_skip_intermediate_states() {
        // Промежуточные состояния (Transcribing, Enhancing, Pasting) не уведомляют
        assert!(notification_text(AppState::Recording, AppState::Transcribing).is_none());
        assert!(notification_text(AppState::Transcribing, AppState::Enhancing).is_none());
        assert!(notification_text(AppState::Enhancing, AppState::Pasting).is_none());
    }

    #[test]
    fn key_transitions_should_produce_non_empty_text() {
        let transitions = [
            (AppState::Idle, AppState::Recording),
            (AppState::Pasting, AppState::Idle),
            (AppState::Transcribing, AppState::Idle),
            (AppState::Error, AppState::Idle),
            (AppState::Recording, AppState::Error),
        ];

        for (old, new) in transitions {
            let (title, body) = notification_text(old, new)
                .unwrap_or_else(|| panic!("expected text for {:?} -> {:?}", old, new));
            assert!(!title.is_empty(), "empty title for {:?} -> {:?}", old, new);
            assert!(!body.is_empty(), "empty body for {:?} -> {:?}", old, new);
        }
    }
}

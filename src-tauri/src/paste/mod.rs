pub mod clipboard;
pub mod input;

use std::thread;
use std::time::Duration;

pub use self::clipboard::ClipboardManager;

/// Задержка перед восстановлением clipboard (мс).
///
/// Дает приложению-получателю время обработать Ctrl+V.
const RESTORE_DELAY_MS: u64 = 300;

/// Ошибки модуля вставки текста.
#[derive(Debug, thiserror::Error)]
pub enum PasteError {
    #[error("clipboard unavailable: {0}")]
    ClipboardUnavailable(String),

    #[error("clipboard write failed: {0}")]
    ClipboardWrite(String),

    #[error("input simulation failed: {0}")]
    InputSimulation(String),
}

pub type Result<T> = std::result::Result<T, PasteError>;

/// Результат операции вставки.
#[derive(Debug, Clone, PartialEq)]
pub enum PasteStatus {
    /// Текст вставлен через Ctrl+V/Cmd+V, clipboard восстановлен.
    Pasted,
    /// Текст записан в clipboard, но симуляция клавиш не удалась.
    /// Пользователь должен вставить вручную (Ctrl+V).
    ClipboardOnly,
    /// Clipboard недоступен, текст нужно показать в окне результата.
    ResultWindow,
}

/// Вставляет текст в активное поле ввода.
///
/// Pipeline:
/// 1. Сохранить текущее содержимое clipboard
/// 2. Записать текст в clipboard
/// 3. Симулировать Ctrl+V / Cmd+V
/// 4. Подождать пока приложение обработает вставку
/// 5. Восстановить содержимое clipboard
///
/// При ошибке симуляции клавиш (Wayland, отсутствие permissions):
/// текст остается в clipboard, возвращается `ClipboardOnly`.
///
/// При ошибке clipboard: возвращается `ResultWindow`.
pub fn paste_text(text: &str) -> PasteStatus {
    tracing::info!("Starting paste pipeline ({} chars)", text.len());

    let mut manager = match ClipboardManager::new() {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("Clipboard unavailable: {e}, falling back to ResultWindow");
            return PasteStatus::ResultWindow;
        }
    };

    if let Err(e) = manager.save() {
        tracing::warn!("Failed to save clipboard: {e}, continuing without restore");
    }

    if let Err(e) = manager.write(text) {
        tracing::warn!("Failed to write to clipboard: {e}, falling back to ResultWindow");
        return PasteStatus::ResultWindow;
    }

    if let Err(e) = input::simulate_paste() {
        tracing::warn!("Key simulation failed: {e}, text is in clipboard (ClipboardOnly mode)");
        return PasteStatus::ClipboardOnly;
    }

    thread::sleep(Duration::from_millis(RESTORE_DELAY_MS));

    if let Err(e) = manager.restore() {
        tracing::warn!("Failed to restore clipboard: {e} (text was pasted successfully)");
    }

    tracing::info!("Paste completed successfully");
    PasteStatus::Pasted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_error_should_display_clipboard_unavailable() {
        // Given
        let error = PasteError::ClipboardUnavailable("access denied".to_string());

        // When
        let msg = error.to_string();

        // Then
        assert_eq!(msg, "clipboard unavailable: access denied");
    }

    #[test]
    fn paste_error_should_display_clipboard_write() {
        // Given
        let error = PasteError::ClipboardWrite("write failed".to_string());

        // When
        let msg = error.to_string();

        // Then
        assert_eq!(msg, "clipboard write failed: write failed");
    }

    #[test]
    fn paste_error_should_display_input_simulation() {
        // Given
        let error = PasteError::InputSimulation("enigo init failed".to_string());

        // When
        let msg = error.to_string();

        // Then
        assert_eq!(msg, "input simulation failed: enigo init failed");
    }

    #[test]
    fn paste_status_should_support_equality() {
        // Given / When / Then
        assert_eq!(PasteStatus::Pasted, PasteStatus::Pasted);
        assert_eq!(PasteStatus::ClipboardOnly, PasteStatus::ClipboardOnly);
        assert_eq!(PasteStatus::ResultWindow, PasteStatus::ResultWindow);
        assert_ne!(PasteStatus::Pasted, PasteStatus::ClipboardOnly);
    }

    #[test]
    fn paste_status_should_support_debug() {
        // Given / When
        let debug = format!("{:?}", PasteStatus::Pasted);

        // Then
        assert_eq!(debug, "Pasted");
    }

    #[test]
    fn paste_status_should_be_cloneable() {
        // Given
        let status = PasteStatus::ClipboardOnly;

        // When
        let cloned = status.clone();

        // Then
        assert_eq!(status, cloned);
    }
}

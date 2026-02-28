use arboard::Clipboard;

/// Менеджер буфера обмена с поддержкой save/restore.
///
/// Сохраняет текущее содержимое clipboard перед записью нового текста,
/// чтобы восстановить его после вставки.
pub struct ClipboardManager {
    clipboard: Clipboard,
    saved_content: Option<String>,
}

impl ClipboardManager {
    /// Создает новый менеджер буфера обмена.
    pub fn new() -> super::Result<Self> {
        let clipboard =
            Clipboard::new().map_err(|e| super::PasteError::ClipboardUnavailable(e.to_string()))?;
        Ok(Self {
            clipboard,
            saved_content: None,
        })
    }

    /// Сохраняет текущее текстовое содержимое clipboard.
    ///
    /// Если clipboard содержит не текст (изображение, файлы) - пропускает,
    /// чтобы не блокировать pipeline вставки.
    pub fn save(&mut self) -> super::Result<()> {
        match self.clipboard.get_text() {
            Ok(text) => {
                tracing::debug!("Clipboard content saved ({} chars)", text.len());
                self.saved_content = Some(text);
            }
            Err(_) => {
                tracing::debug!("Clipboard has no text content, skipping save");
                self.saved_content = None;
            }
        }
        Ok(())
    }

    /// Записывает текст в clipboard.
    pub fn write(&mut self, text: &str) -> super::Result<()> {
        self.clipboard
            .set_text(text)
            .map_err(|e| super::PasteError::ClipboardWrite(e.to_string()))?;
        tracing::debug!("Text written to clipboard ({} chars)", text.len());
        Ok(())
    }

    /// Восстанавливает ранее сохраненное содержимое clipboard.
    ///
    /// Если save не вызывался или clipboard не содержал текст - очищает clipboard.
    pub fn restore(&mut self) -> super::Result<()> {
        match self.saved_content.take() {
            Some(content) => {
                self.clipboard
                    .set_text(&content)
                    .map_err(|e| super::PasteError::ClipboardWrite(e.to_string()))?;
                tracing::debug!("Clipboard content restored ({} chars)", content.len());
            }
            None => {
                self.clipboard.clear().map_err(|e| {
                    super::PasteError::ClipboardWrite(format!("failed to clear: {e}"))
                })?;
                tracing::debug!("Clipboard cleared (no saved content)");
            }
        }
        Ok(())
    }

    /// Читает текущее текстовое содержимое clipboard.
    pub fn read(&mut self) -> super::Result<Option<String>> {
        match self.clipboard.get_text() {
            Ok(text) => Ok(Some(text)),
            Err(_) => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;

    #[test]
    #[serial]
    fn new_should_create_clipboard_manager() {
        // Given / When
        let result = ClipboardManager::new();

        // Then
        assert!(result.is_ok());
    }

    #[test]
    #[serial]
    fn write_should_set_text_in_clipboard() {
        // Given
        let mut manager = ClipboardManager::new().unwrap();

        // When
        let result = manager.write("test text");

        // Then
        assert!(result.is_ok());
        let content = manager.read().unwrap();
        assert_eq!(content, Some("test text".to_string()));
    }

    #[test]
    #[serial]
    fn save_and_restore_should_preserve_clipboard_content() {
        // Given
        let mut manager = ClipboardManager::new().unwrap();
        manager.write("original content").unwrap();

        // When
        manager.save().unwrap();
        manager.write("temporary text").unwrap();
        manager.restore().unwrap();

        // Then
        let content = manager.read().unwrap();
        assert_eq!(content, Some("original content".to_string()));
    }

    #[test]
    #[serial]
    fn restore_without_save_should_clear_clipboard() {
        // Given
        let mut manager = ClipboardManager::new().unwrap();
        manager.write("some text").unwrap();

        // When
        manager.restore().unwrap();

        // Then
        let content = manager.read().unwrap();
        let is_empty = content.is_none() || content.as_deref() == Some("");
        assert!(is_empty);
    }

    #[test]
    #[serial]
    fn save_should_handle_empty_clipboard() {
        // Given
        let mut manager = ClipboardManager::new().unwrap();
        manager.clipboard.clear().ok();

        // When
        let result = manager.save();

        // Then
        assert!(result.is_ok());
    }

    #[test]
    #[serial]
    fn read_should_return_none_when_no_text() {
        // Given
        let mut manager = ClipboardManager::new().unwrap();
        manager.clipboard.clear().ok();

        // When
        let result = manager.read().unwrap();

        // Then
        let is_empty = result.is_none() || result.as_deref() == Some("");
        assert!(is_empty);
    }

    #[test]
    #[serial]
    fn write_and_read_roundtrip_with_unicode() {
        // Given
        let mut manager = ClipboardManager::new().unwrap();
        let unicode_text = "Привет мир! Hello World! 你好世界!";

        // When
        manager.write(unicode_text).unwrap();
        let result = manager.read().unwrap();

        // Then
        assert_eq!(result, Some(unicode_text.to_string()));
    }

    #[test]
    #[serial]
    fn save_restore_cycle_should_be_repeatable() {
        // Given
        let mut manager = ClipboardManager::new().unwrap();

        // When - first cycle
        manager.write("first").unwrap();
        manager.save().unwrap();
        manager.write("temp1").unwrap();
        manager.restore().unwrap();
        let after_first = manager.read().unwrap();

        // When - second cycle
        manager.write("second").unwrap();
        manager.save().unwrap();
        manager.write("temp2").unwrap();
        manager.restore().unwrap();
        let after_second = manager.read().unwrap();

        // Then
        assert_eq!(after_first, Some("first".to_string()));
        assert_eq!(after_second, Some("second".to_string()));
    }
}

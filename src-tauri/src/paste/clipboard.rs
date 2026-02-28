use arboard::Clipboard;

/// Состояние сохраненного содержимого clipboard.
#[derive(Debug)]
enum SavedClipboard {
    /// Clipboard содержал текст, который был успешно сохранен.
    Text(String),
    /// Clipboard был пуст или содержал нетекстовые данные (изображение, файлы).
    /// При restore не трогаем clipboard - не хотим потерять non-text содержимое.
    NonTextOrEmpty,
    /// Save еще не вызывался.
    NotSaved,
}

/// Максимальное количество retry при `ClipboardOccupied`.
const CLIPBOARD_RETRY_COUNT: u32 = 3;

/// Задержка между retry (мс).
const CLIPBOARD_RETRY_DELAY_MS: u64 = 50;

/// Менеджер буфера обмена с поддержкой save/restore.
///
/// Сохраняет текущее содержимое clipboard перед записью нового текста,
/// чтобы восстановить его после вставки.
pub struct ClipboardManager {
    clipboard: Clipboard,
    saved: SavedClipboard,
}

impl ClipboardManager {
    /// Создает новый менеджер буфера обмена.
    pub fn new() -> super::Result<Self> {
        let clipboard =
            Clipboard::new().map_err(|e| super::PasteError::ClipboardUnavailable(e.to_string()))?;
        Ok(Self {
            clipboard,
            saved: SavedClipboard::NotSaved,
        })
    }

    /// Сохраняет текущее текстовое содержимое clipboard.
    ///
    /// - Текст -> сохраняется для последующего restore.
    /// - Нет текста / пустой / non-text -> запоминает `NonTextOrEmpty` (restore будет no-op).
    /// - `ClipboardOccupied` -> retry с backoff (до `CLIPBOARD_RETRY_COUNT` попыток).
    /// - Прочие ошибки -> пробрасываются вверх.
    pub fn save(&mut self) -> super::Result<()> {
        match self.get_text_with_retry() {
            Ok(text) => {
                tracing::debug!("Clipboard content saved ({} chars)", text.len());
                self.saved = SavedClipboard::Text(text);
            }
            Err(arboard::Error::ContentNotAvailable) => {
                tracing::debug!("Clipboard has no text content, save as NonTextOrEmpty");
                self.saved = SavedClipboard::NonTextOrEmpty;
            }
            Err(e) => {
                tracing::warn!("Clipboard save failed: {e}");
                return Err(super::PasteError::ClipboardUnavailable(e.to_string()));
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
    /// - `Text(s)` -> записывает сохраненный текст обратно.
    /// - `NonTextOrEmpty` -> no-op (не трогаем clipboard, чтобы не потерять non-text данные).
    /// - `NotSaved` -> no-op (save не вызывался).
    pub fn restore(&mut self) -> super::Result<()> {
        let saved = std::mem::replace(&mut self.saved, SavedClipboard::NotSaved);
        match saved {
            SavedClipboard::Text(content) => {
                self.clipboard
                    .set_text(&content)
                    .map_err(|e| super::PasteError::ClipboardWrite(e.to_string()))?;
                tracing::debug!("Clipboard content restored ({} chars)", content.len());
            }
            SavedClipboard::NonTextOrEmpty => {
                tracing::debug!("Clipboard had non-text/empty content, skipping restore");
            }
            SavedClipboard::NotSaved => {
                tracing::debug!("No saved clipboard state, skipping restore");
            }
        }
        Ok(())
    }

    /// Читает текущее текстовое содержимое clipboard.
    ///
    /// - Текст доступен -> `Ok(Some(text))`
    /// - Нет текста / non-text содержимое -> `Ok(None)`
    /// - Прочие ошибки (occupied, system) -> `Err`
    pub fn read(&mut self) -> super::Result<Option<String>> {
        match self.clipboard.get_text() {
            Ok(text) => Ok(Some(text)),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(e) => Err(super::PasteError::ClipboardUnavailable(e.to_string())),
        }
    }

    /// Читает текст из clipboard с retry при `ClipboardOccupied`.
    fn get_text_with_retry(&mut self) -> std::result::Result<String, arboard::Error> {
        let mut last_err = arboard::Error::ContentNotAvailable;
        for attempt in 0..=CLIPBOARD_RETRY_COUNT {
            match self.clipboard.get_text() {
                Ok(text) => return Ok(text),
                Err(arboard::Error::ClipboardOccupied) if attempt < CLIPBOARD_RETRY_COUNT => {
                    tracing::debug!(
                        "Clipboard occupied, retry {}/{}",
                        attempt + 1,
                        CLIPBOARD_RETRY_COUNT,
                    );
                    std::thread::sleep(std::time::Duration::from_millis(
                        CLIPBOARD_RETRY_DELAY_MS * (attempt as u64 + 1),
                    ));
                    last_err = arboard::Error::ClipboardOccupied;
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_err)
    }
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;

    /// Guard, который сохраняет текущее текстовое содержимое clipboard
    /// при создании и восстанавливает в `Drop`. Минимизирует влияние тестов
    /// на реальный буфер обмена пользователя.
    struct ClipboardTestGuard {
        original: Option<String>,
    }

    impl ClipboardTestGuard {
        fn new() -> Self {
            let original = Clipboard::new().ok().and_then(|mut c| c.get_text().ok());
            Self { original }
        }
    }

    impl Drop for ClipboardTestGuard {
        fn drop(&mut self) {
            if let Some(ref text) = self.original {
                if let Ok(mut c) = Clipboard::new() {
                    let _ = c.set_text(text);
                }
            }
        }
    }

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
        let _guard = ClipboardTestGuard::new();
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
        let _guard = ClipboardTestGuard::new();
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
    fn restore_without_save_should_be_noop() {
        // Given
        let _guard = ClipboardTestGuard::new();
        let mut manager = ClipboardManager::new().unwrap();
        manager.write("existing text").unwrap();

        // When - restore without prior save should NOT touch clipboard
        manager.restore().unwrap();

        // Then - clipboard should still contain the text
        let content = manager.read().unwrap();
        assert_eq!(content, Some("existing text".to_string()));
    }

    #[test]
    #[serial]
    fn save_should_handle_empty_clipboard() {
        // Given
        let _guard = ClipboardTestGuard::new();
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
        let _guard = ClipboardTestGuard::new();
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
        let _guard = ClipboardTestGuard::new();
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
        let _guard = ClipboardTestGuard::new();
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

    #[test]
    #[serial]
    fn restore_with_non_text_save_should_not_clear_clipboard() {
        // Given
        let _guard = ClipboardTestGuard::new();
        let mut manager = ClipboardManager::new().unwrap();
        // Симулируем ситуацию, когда save нашел не-текстовое содержимое
        manager.clipboard.clear().ok();
        manager.save().unwrap(); // saved = NonTextOrEmpty

        // Записываем текст (как это делает paste pipeline)
        manager.write("pasted text").unwrap();

        // When - restore после NonTextOrEmpty save должен быть no-op
        manager.restore().unwrap();

        // Then - "pasted text" все еще в clipboard (не очищен)
        let content = manager.read().unwrap();
        assert_eq!(content, Some("pasted text".to_string()));
    }
}

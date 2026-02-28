/// Общий тип ошибки приложения.
///
/// Каждый вариант соответствует модулю, который может генерировать ошибки.
/// Детальные типы ошибок модулей будут определены в соответствующих фичах.
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum AppError {
    #[error("Audio error: {0}")]
    Audio(String),

    #[error("VAD error: {0}")]
    Vad(#[from] crate::vad::VadError),

    #[error("STT error: {0}")]
    Stt(#[from] crate::stt::SttError),

    #[error("Enhance error: {0}")]
    Enhance(String),

    #[error("Paste error: {0}")]
    Paste(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("Hotkey error: {0}")]
    Hotkey(String),
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        AppError::Config(err.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        AppError::Config(err.to_string())
    }
}

/// Общий Result-тип приложения.
#[allow(dead_code)]
pub type Result<T> = std::result::Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_error_should_display_module_prefix() {
        // Given
        let error = AppError::Audio("no microphone found".to_string());

        // When
        let msg = error.to_string();

        // Then
        assert_eq!(msg, "Audio error: no microphone found");
    }

    #[test]
    fn app_error_should_convert_from_io_error() {
        // Given
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");

        // When
        let app_err: AppError = io_err.into();

        // Then
        assert!(matches!(app_err, AppError::Config(_)));
        assert!(app_err.to_string().contains("file not found"));
    }

    #[test]
    fn result_type_should_work_with_ok_and_err() {
        // Given / When
        let ok: Result<i32> = Ok(42);
        let err: Result<i32> = Err(AppError::Stt(crate::stt::SttError::Timeout));

        // Then
        assert!(ok.is_ok());
        assert!(err.is_err());
    }
}

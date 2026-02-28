pub mod openai_responses;

pub use self::openai_responses::OpenAiEnhancer;

/// Ошибки модуля улучшения текста.
#[derive(Debug, Clone, thiserror::Error)]
pub enum EnhanceError {
    #[error("network error: {0}")]
    Network(String),

    #[error("authentication failed: check API key")]
    AuthFailed,

    #[error("rate limited, retry after {retry_after_sec}s")]
    RateLimited { retry_after_sec: u64 },

    #[error("request timeout")]
    Timeout,

    #[error("API error ({status}): {message}")]
    ApiError { status: u16, message: String },

    #[error("invalid response: {0}")]
    InvalidResponse(String),
}

pub type Result<T> = std::result::Result<T, EnhanceError>;

/// Контракт для провайдеров улучшения текста.
///
/// Позволяет подменять реализацию через generics.
pub trait EnhanceProvider: Send + Sync {
    fn enhance(
        &self,
        raw_text: &str,
        language: Option<&str>,
    ) -> impl std::future::Future<Output = Result<String>> + Send;
}

/// Результат валидации улучшенного текста.
#[derive(Debug, Clone, PartialEq)]
pub enum ValidationResult {
    /// Улучшенный текст прошел проверку.
    Ok(String),
    /// Fallback к исходному тексту (причина логируется через tracing).
    Fallback(String),
}

/// Максимальная допустимая длина улучшенного текста (символов).
const MAX_ENHANCED_CHARS: usize = 5000;

/// Минимальное отношение слов enhanced/raw (30%).
const MIN_WORD_RATIO: f64 = 0.3;

/// Максимальное отношение слов enhanced/raw (150%).
const MAX_WORD_RATIO: f64 = 1.5;

/// Проверяет результат улучшения текста на адекватность.
///
/// Защита от галлюцинаций LLM: если модель выдала пустой,
/// слишком короткий или слишком длинный текст, возвращаем исходный.
pub fn validate_enhancement(raw: &str, enhanced: &str) -> ValidationResult {
    if raw.trim().is_empty() {
        return ValidationResult::Fallback(raw.to_string());
    }

    let enhanced_trimmed = enhanced.trim();

    if enhanced_trimmed.is_empty() {
        tracing::warn!("Enhancement returned empty text, falling back to raw");
        return ValidationResult::Fallback(raw.to_string());
    }

    let raw_words = count_words(raw);
    let enhanced_words = count_words(enhanced_trimmed);

    // Для очень коротких текстов (1-2 слова) пропускаем проверку ratio
    if raw_words > 2 {
        let ratio = enhanced_words as f64 / raw_words as f64;

        if ratio < MIN_WORD_RATIO {
            tracing::warn!(
                "Enhancement too short ({enhanced_words} vs {raw_words} words, ratio {ratio:.2}), \
                 probable loss of content, falling back to raw"
            );
            return ValidationResult::Fallback(raw.to_string());
        }

        if ratio > MAX_WORD_RATIO {
            tracing::warn!(
                "Enhancement too long ({enhanced_words} vs {raw_words} words, ratio {ratio:.2}), \
                 probable hallucination, falling back to raw"
            );
            return ValidationResult::Fallback(raw.to_string());
        }
    }

    if enhanced_trimmed.len() > MAX_ENHANCED_CHARS {
        let truncated: String = enhanced_trimmed.chars().take(MAX_ENHANCED_CHARS).collect();
        tracing::warn!(
            "Enhancement exceeds {MAX_ENHANCED_CHARS} chars ({}), truncating",
            enhanced_trimmed.len()
        );
        return ValidationResult::Ok(truncated);
    }

    ValidationResult::Ok(enhanced_trimmed.to_string())
}

fn count_words(text: &str) -> usize {
    text.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_should_pass_normal_enhancement() {
        // Given
        let raw = "привет как дела у тебя сегодня";
        let enhanced = "Привет, как дела у тебя сегодня?";

        // When
        let result = validate_enhancement(raw, enhanced);

        // Then
        assert_eq!(
            result,
            ValidationResult::Ok("Привет, как дела у тебя сегодня?".to_string())
        );
    }

    #[test]
    fn validate_should_fallback_on_empty_response() {
        // Given
        let raw = "some text here";

        // When
        let result = validate_enhancement(raw, "");

        // Then
        assert_eq!(
            result,
            ValidationResult::Fallback("some text here".to_string())
        );
    }

    #[test]
    fn validate_should_fallback_on_whitespace_only_response() {
        // Given
        let raw = "some text here";

        // When
        let result = validate_enhancement(raw, "   \n\t  ");

        // Then
        assert_eq!(
            result,
            ValidationResult::Fallback("some text here".to_string())
        );
    }

    #[test]
    fn validate_should_fallback_on_too_short_response() {
        // Given
        let raw = "this is a long sentence with many words in it for testing";
        let enhanced = "short";

        // When
        let result = validate_enhancement(raw, enhanced);

        // Then
        assert_eq!(result, ValidationResult::Fallback(raw.to_string()));
    }

    #[test]
    fn validate_should_fallback_on_too_long_response() {
        // Given
        let raw = "hello world test";
        let enhanced = "hello world test and here is a lot of extra words \
            that the model hallucinated because it was not paying attention \
            to the instructions and just kept generating more text endlessly";

        // When
        let result = validate_enhancement(raw, enhanced);

        // Then
        assert_eq!(result, ValidationResult::Fallback(raw.to_string()));
    }

    #[test]
    fn validate_should_truncate_very_long_text() {
        // Given
        let raw = "a ".repeat(3000);
        let enhanced = "b ".repeat(3000);

        // When
        let result = validate_enhancement(&raw, &enhanced);

        // Then
        match result {
            ValidationResult::Ok(text) => assert!(text.len() <= MAX_ENHANCED_CHARS),
            _ => panic!("Expected Ok with truncation"),
        }
    }

    #[test]
    fn validate_should_skip_ratio_check_for_short_text() {
        // Given: 1-2 word text
        let raw = "ok";
        let enhanced = "OK.";

        // When
        let result = validate_enhancement(raw, enhanced);

        // Then
        assert_eq!(result, ValidationResult::Ok("OK.".to_string()));
    }

    #[test]
    fn validate_should_fallback_on_empty_raw() {
        // Given: empty raw text should always fallback, even if enhanced is non-empty
        let result_empty = validate_enhancement("", "some enhanced text");
        let result_whitespace = validate_enhancement("   \n\t  ", "some enhanced text");

        // Then
        assert_eq!(result_empty, ValidationResult::Fallback("".to_string()));
        assert_eq!(
            result_whitespace,
            ValidationResult::Fallback("   \n\t  ".to_string())
        );
    }

    #[test]
    fn validate_should_trim_enhanced_text() {
        // Given
        let raw = "hello world test check";
        let enhanced = "  Hello world, test check.  ";

        // When
        let result = validate_enhancement(raw, enhanced);

        // Then
        assert_eq!(
            result,
            ValidationResult::Ok("Hello world, test check.".to_string())
        );
    }

    #[test]
    fn count_words_should_handle_various_whitespace() {
        assert_eq!(count_words("hello world"), 2);
        assert_eq!(count_words("  hello   world  "), 2);
        assert_eq!(count_words(""), 0);
        assert_eq!(count_words("   "), 0);
        assert_eq!(count_words("one"), 1);
    }

    #[test]
    fn enhance_error_should_display_correctly() {
        // Given / When / Then
        let err = EnhanceError::Network("connection refused".into());
        assert_eq!(err.to_string(), "network error: connection refused");

        let err = EnhanceError::AuthFailed;
        assert_eq!(err.to_string(), "authentication failed: check API key");

        let err = EnhanceError::Timeout;
        assert_eq!(err.to_string(), "request timeout");

        let err = EnhanceError::ApiError {
            status: 500,
            message: "internal error".into(),
        };
        assert_eq!(err.to_string(), "API error (500): internal error");
    }
}

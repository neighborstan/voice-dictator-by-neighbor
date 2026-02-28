use std::time::Duration;

use bytes::Bytes;
use reqwest::header;
use reqwest::StatusCode;
use serde::Deserialize;

use super::{Result, SttError, SttProvider};

const USER_AGENT: &str = "VoiceDictator/0.1.0";

/// Максимум повторных попыток при rate limiting (429).
const MAX_RATE_LIMIT_RETRIES: u32 = 5;

/// Верхняя граница задержки backoff (секунды).
const MAX_BACKOFF_SEC: u64 = 16;

/// Клиент для OpenAI STT API.
///
/// Выполняет `POST /v1/audio/transcriptions` с multipart-данными.
/// Поддерживает retry с exponential backoff и обработку rate limiting (429).
pub struct OpenAiSttClient {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    retry_count: u32,
    read_timeout: Duration,
}

#[derive(Deserialize)]
struct TranscriptionResponse {
    text: String,
}

impl OpenAiSttClient {
    /// Создает клиент OpenAI STT API.
    ///
    /// - `base_url` - базовый URL (например "https://api.openai.com"), слеш в конце убирается
    /// - `api_key` - Bearer-токен
    /// - `model` - модель STT из конфига
    /// - `connect_timeout` - таймаут установки соединения
    /// - `read_timeout` - таймаут ожидания ответа
    /// - `retry_count` - количество повторных попыток (0 = без retry)
    pub fn new(
        base_url: &str,
        api_key: &str,
        model: &str,
        connect_timeout: Duration,
        read_timeout: Duration,
        retry_count: u32,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(connect_timeout)
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| SttError::Network(e.to_string()))?;

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
            retry_count,
            read_timeout,
        })
    }

    /// Создает клиент из AppConfig и API-ключа.
    pub fn from_config(config: &crate::config::schema::AppConfig, api_key: &str) -> Result<Self> {
        Self::new(
            &config.api_base_url,
            api_key,
            &config.stt_model,
            Duration::from_secs(config.connect_timeout_sec as u64),
            Duration::from_secs(config.read_timeout_stt_sec as u64),
            config.retry_count,
        )
    }

    /// Транскрипция с retry и rate limiting.
    async fn do_transcribe(&self, audio: &[u8], language: Option<&str>) -> Result<String> {
        let url = format!("{}/v1/audio/transcriptions", self.base_url);
        let audio_bytes = Bytes::copy_from_slice(audio);
        let mut retries_left = self.retry_count;
        let mut rate_limit_retries: u32 = 0;

        loop {
            match self.send_request(&url, audio_bytes.clone(), language).await {
                Ok(text) => return Ok(text),
                Err(SttError::RateLimited { retry_after_sec }) => {
                    rate_limit_retries += 1;
                    if rate_limit_retries > MAX_RATE_LIMIT_RETRIES {
                        return Err(SttError::RateLimited { retry_after_sec });
                    }
                    tracing::warn!(
                        "API rate limited, waiting {retry_after_sec}s \
                         (attempt {rate_limit_retries}/{MAX_RATE_LIMIT_RETRIES})"
                    );
                    tokio::time::sleep(Duration::from_secs(retry_after_sec)).await;
                    continue;
                }
                Err(e) if !Self::is_retryable(&e) => return Err(e),
                Err(e) => {
                    if retries_left == 0 {
                        return Err(e);
                    }
                    let attempt = self.retry_count - retries_left;
                    let backoff_sec = 1u64
                        .checked_shl(attempt)
                        .unwrap_or(MAX_BACKOFF_SEC)
                        .min(MAX_BACKOFF_SEC);
                    tracing::warn!(
                        "STT request failed (retry {}/{}), backoff {backoff_sec}s: {e}",
                        attempt + 1,
                        self.retry_count
                    );
                    tokio::time::sleep(Duration::from_secs(backoff_sec)).await;
                    retries_left -= 1;
                }
            }
        }
    }

    /// Определяет, стоит ли повторять запрос при данной ошибке.
    fn is_retryable(err: &SttError) -> bool {
        match err {
            SttError::Network(_) | SttError::Timeout => true,
            SttError::ApiError { status, .. } => *status >= 500,
            _ => false,
        }
    }

    /// Одиночный HTTP-запрос транскрипции.
    async fn send_request(
        &self,
        url: &str,
        audio: Bytes,
        language: Option<&str>,
    ) -> Result<String> {
        let file_part = reqwest::multipart::Part::stream(audio)
            .file_name("audio.ogg")
            .mime_str("audio/ogg")
            .map_err(|e| SttError::Network(e.to_string()))?;

        let mut form = reqwest::multipart::Form::new()
            .text("model", self.model.clone())
            .text("response_format", "json")
            .part("file", file_part);

        if let Some(lang) = language {
            if lang != "auto" {
                form = form.text("language", lang.to_string());
            }
        }

        let response = self
            .client
            .post(url)
            .header(header::AUTHORIZATION, format!("Bearer {}", self.api_key))
            .timeout(self.read_timeout)
            .multipart(form)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    SttError::Timeout
                } else {
                    SttError::Network(e.to_string())
                }
            })?;

        let status = response.status();

        // Обработка статусов дублирует enhance/openai_responses.rs - осознанное решение:
        // модули используют разные Error-типы и могут разойтись по логике.
        if status == StatusCode::UNAUTHORIZED {
            return Err(SttError::AuthFailed);
        }

        if status == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(5)
                .clamp(1, 60);
            return Err(SttError::RateLimited {
                retry_after_sec: retry_after,
            });
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(SttError::ApiError {
                status: status.as_u16(),
                message: body,
            });
        }

        let body: TranscriptionResponse = response
            .json()
            .await
            .map_err(|e| SttError::InvalidResponse(e.to_string()))?;

        if body.text.trim().is_empty() {
            return Err(SttError::InvalidResponse(
                "empty transcription text".to_string(),
            ));
        }

        Ok(body.text)
    }
}

impl SttProvider for OpenAiSttClient {
    async fn transcribe(&self, audio: &[u8], language: Option<&str>) -> Result<String> {
        self.do_transcribe(audio, language).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_should_be_created_with_valid_params() {
        let client = OpenAiSttClient::new(
            "https://api.openai.com",
            "test-key",
            "gpt-4o-mini-transcribe",
            Duration::from_secs(5),
            Duration::from_secs(30),
            3,
        );
        assert!(client.is_ok());
    }

    #[test]
    fn client_should_trim_trailing_slash_from_base_url() {
        // Given / When
        let client = OpenAiSttClient::new(
            "https://api.openai.com/",
            "test-key",
            "gpt-4o-mini-transcribe",
            Duration::from_secs(5),
            Duration::from_secs(30),
            3,
        )
        .unwrap();

        // Then
        assert_eq!(client.base_url, "https://api.openai.com");
    }

    #[test]
    fn client_should_store_model_from_constructor() {
        // Given / When
        let client = OpenAiSttClient::new(
            "https://api.openai.com",
            "key",
            "my-custom-model",
            Duration::from_secs(5),
            Duration::from_secs(30),
            2,
        )
        .unwrap();

        // Then
        assert_eq!(client.model, "my-custom-model");
    }

    #[test]
    fn from_config_should_use_config_values() {
        // Given
        let config = crate::config::schema::AppConfig {
            api_base_url: "https://custom.api.com".to_string(),
            stt_model: "custom-stt".to_string(),
            connect_timeout_sec: 10,
            read_timeout_stt_sec: 60,
            retry_count: 5,
            ..Default::default()
        };

        // When
        let client = OpenAiSttClient::from_config(&config, "api-key-123").unwrap();

        // Then
        assert_eq!(client.base_url, "https://custom.api.com");
        assert_eq!(client.model, "custom-stt");
        assert_eq!(client.retry_count, 5);
    }

    #[test]
    fn is_retryable_should_return_true_for_network_error() {
        assert!(OpenAiSttClient::is_retryable(&SttError::Network(
            "err".into()
        )));
    }

    #[test]
    fn is_retryable_should_return_true_for_timeout() {
        assert!(OpenAiSttClient::is_retryable(&SttError::Timeout));
    }

    #[test]
    fn is_retryable_should_return_true_for_5xx() {
        assert!(OpenAiSttClient::is_retryable(&SttError::ApiError {
            status: 500,
            message: "internal error".into(),
        }));
        assert!(OpenAiSttClient::is_retryable(&SttError::ApiError {
            status: 503,
            message: "unavailable".into(),
        }));
    }

    #[test]
    fn is_retryable_should_return_false_for_auth_error() {
        assert!(!OpenAiSttClient::is_retryable(&SttError::AuthFailed));
    }

    #[test]
    fn is_retryable_should_return_false_for_400() {
        assert!(!OpenAiSttClient::is_retryable(&SttError::ApiError {
            status: 400,
            message: "bad request".into(),
        }));
    }

    #[test]
    fn is_retryable_should_return_false_for_rate_limited() {
        assert!(!OpenAiSttClient::is_retryable(&SttError::RateLimited {
            retry_after_sec: 5,
        }));
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

    fn make_test_audio() -> Vec<u8> {
        vec![0x4f, 0x67, 0x67, 0x53, 0x00, 0x02, 0x00, 0x00]
    }

    /// Matches if the raw request body contains the given substring.
    ///
    /// Useful for verifying multipart form fields without full parsing.
    struct BodyContains(String);

    impl Match for BodyContains {
        fn matches(&self, request: &Request) -> bool {
            let body = String::from_utf8_lossy(&request.body);
            body.contains(&self.0)
        }
    }

    async fn create_test_client(base_url: &str) -> OpenAiSttClient {
        OpenAiSttClient::new(
            base_url,
            "test-api-key",
            "gpt-4o-mini-transcribe",
            Duration::from_secs(5),
            Duration::from_secs(10),
            2,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn transcribe_should_return_text_on_success() {
        // Given
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "text": "hello world" })),
            )
            .mount(&server)
            .await;

        let client = create_test_client(&server.uri()).await;

        // When
        let result = client.do_transcribe(&make_test_audio(), None).await;

        // Then
        assert_eq!(result.unwrap(), "hello world");
    }

    #[tokio::test]
    async fn transcribe_should_fail_on_401() {
        // Given
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let client = create_test_client(&server.uri()).await;

        // When
        let result = client.do_transcribe(&make_test_audio(), None).await;

        // Then
        assert!(matches!(result.unwrap_err(), SttError::AuthFailed));
    }

    #[tokio::test]
    async fn transcribe_should_handle_rate_limiting() {
        // Given: first request -> 429, second -> 200
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(429).append_header("Retry-After", "1"))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "text": "after rate limit" })),
            )
            .mount(&server)
            .await;

        let client = create_test_client(&server.uri()).await;

        // When
        let result = client.do_transcribe(&make_test_audio(), None).await;

        // Then
        assert_eq!(result.unwrap(), "after rate limit");
    }

    #[tokio::test]
    async fn transcribe_should_retry_on_5xx() {
        // Given: first request -> 500, second -> 200
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(500).set_body_string("error"))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "text": "recovered" })),
            )
            .mount(&server)
            .await;

        let client = create_test_client(&server.uri()).await;

        // When
        let result = client.do_transcribe(&make_test_audio(), None).await;

        // Then
        assert_eq!(result.unwrap(), "recovered");
    }

    #[tokio::test]
    async fn transcribe_should_fail_after_exhausting_retries() {
        // Given: always 500
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(500).set_body_string("server error"))
            .mount(&server)
            .await;

        let client = create_test_client(&server.uri()).await;

        // When
        let result = client.do_transcribe(&make_test_audio(), None).await;

        // Then
        assert!(matches!(
            result.unwrap_err(),
            SttError::ApiError { status: 500, .. }
        ));
    }

    #[tokio::test]
    async fn transcribe_should_not_retry_on_400() {
        // Given
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
            .expect(1) // should be called only once
            .mount(&server)
            .await;

        let client = create_test_client(&server.uri()).await;

        // When
        let result = client.do_transcribe(&make_test_audio(), None).await;

        // Then
        assert!(matches!(
            result.unwrap_err(),
            SttError::ApiError { status: 400, .. }
        ));
    }

    #[tokio::test]
    async fn transcribe_should_fail_on_empty_text() {
        // Given
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "text": "   " })),
            )
            .mount(&server)
            .await;

        let client = create_test_client(&server.uri()).await;

        // When
        let result = client.do_transcribe(&make_test_audio(), None).await;

        // Then
        assert!(matches!(result.unwrap_err(), SttError::InvalidResponse(_)));
    }

    #[tokio::test]
    async fn transcribe_should_fail_on_invalid_json() {
        // Given
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json at all"))
            .mount(&server)
            .await;

        let client = create_test_client(&server.uri()).await;

        // When
        let result = client.do_transcribe(&make_test_audio(), None).await;

        // Then
        assert!(matches!(result.unwrap_err(), SttError::InvalidResponse(_)));
    }

    #[tokio::test]
    async fn transcribe_should_pass_language_param() {
        // Given: mock expects "language" field with value "ru" in multipart body
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .and(BodyContains("name=\"language\"".to_string()))
            .and(BodyContains("\r\n\r\nru\r\n".to_string()))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "text": "test" })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = create_test_client(&server.uri()).await;

        // When
        let result = client.do_transcribe(&make_test_audio(), Some("ru")).await;

        // Then
        assert_eq!(result.unwrap(), "test");
    }

    #[tokio::test]
    async fn transcribe_should_timeout_on_slow_response() {
        // Given: server delays response longer than read_timeout
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "text": "too late" }))
                    .set_delay(Duration::from_secs(10)),
            )
            .mount(&server)
            .await;

        // Client with very short read_timeout and no retries
        let client = OpenAiSttClient::new(
            &server.uri(),
            "test-api-key",
            "gpt-4o-mini-transcribe",
            Duration::from_secs(5),
            Duration::from_millis(200), // very short read timeout
            0,                          // no retries
        )
        .unwrap();

        // When
        let result = client.do_transcribe(&make_test_audio(), None).await;

        // Then
        assert!(matches!(result.unwrap_err(), SttError::Timeout));
    }
}

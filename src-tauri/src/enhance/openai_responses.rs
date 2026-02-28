use std::time::Duration;

use reqwest::header;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use super::{validate_enhancement, EnhanceError, EnhanceProvider, Result, ValidationResult};

const USER_AGENT: &str = "VoiceDictator/0.1.0";

/// Максимум повторных попыток при rate limiting (429).
const MAX_RATE_LIMIT_RETRIES: u32 = 5;

/// Верхняя граница задержки backoff (секунды).
const MAX_BACKOFF_SEC: u64 = 16;

const SYSTEM_PROMPT: &str = "\
You are a text post-processor. Fix punctuation, grammar, and normalize \
spacing/capitalization in the following dictated text. Do NOT change meaning, \
do NOT add facts, do NOT rephrase, do NOT shorten or expand. Return only \
the corrected text, nothing else.";

const SYSTEM_PROMPT_WITH_LANG: &str = "\
You are a text post-processor. The text is dictated in {lang}. \
Fix punctuation, grammar, and normalize spacing/capitalization. \
Do NOT change meaning, do NOT add facts, do NOT rephrase, \
do NOT shorten or expand. Return only the corrected text, nothing else.";

/// Клиент улучшения текста через OpenAI Responses API.
///
/// Выполняет `POST /v1/responses` с системным промптом для пост-обработки текста.
/// Поддерживает retry с exponential backoff и обработку rate limiting (429).
pub struct OpenAiEnhancer {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    retry_count: u32,
    read_timeout: Duration,
}

#[derive(Serialize)]
struct ResponsesRequest {
    model: String,
    instructions: String,
    input: String,
}

#[derive(Deserialize)]
struct ResponsesResponse {
    output: Vec<OutputItem>,
}

#[derive(Deserialize)]
struct OutputItem {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    text: String,
}

impl OpenAiEnhancer {
    /// Создает клиент OpenAI Responses API.
    ///
    /// - `base_url` - базовый URL API (например "https://api.openai.com"), слеш в конце убирается
    /// - `api_key` - Bearer-токен
    /// - `model` - модель из конфига
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
            .map_err(|e| EnhanceError::Network(e.to_string()))?;

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
            &config.enhance_model,
            Duration::from_secs(config.connect_timeout_sec as u64),
            Duration::from_secs(config.read_timeout_enhance_sec as u64),
            config.retry_count,
        )
    }

    /// Улучшение текста с retry и rate limiting.
    async fn do_enhance(&self, raw_text: &str, language: Option<&str>) -> Result<String> {
        let url = format!("{}/v1/responses", self.base_url);
        let instructions = build_instructions(language);
        let mut retries_left = self.retry_count;
        let mut rate_limit_retries: u32 = 0;

        loop {
            match self.send_request(&url, &instructions, raw_text).await {
                Ok(enhanced) => {
                    return match validate_enhancement(raw_text, &enhanced) {
                        ValidationResult::Ok(text) => Ok(text),
                        ValidationResult::Fallback(text) => Ok(text),
                    };
                }
                Err(EnhanceError::RateLimited { retry_after_sec }) => {
                    rate_limit_retries += 1;
                    if rate_limit_retries > MAX_RATE_LIMIT_RETRIES {
                        tracing::warn!("Enhance rate limit retries exhausted, returning raw text");
                        return Ok(raw_text.to_string());
                    }
                    tracing::warn!(
                        "API rate limited, waiting {retry_after_sec}s \
                         (attempt {rate_limit_retries}/{MAX_RATE_LIMIT_RETRIES})"
                    );
                    tokio::time::sleep(Duration::from_secs(retry_after_sec)).await;
                    continue;
                }
                Err(e) if !Self::is_retryable(&e) => {
                    tracing::warn!("Enhance failed (non-retryable): {e}, returning raw text");
                    return Ok(raw_text.to_string());
                }
                Err(e) => {
                    if retries_left == 0 {
                        tracing::warn!("Enhance retries exhausted: {e}, returning raw text");
                        return Ok(raw_text.to_string());
                    }
                    let attempt = self.retry_count - retries_left;
                    let backoff_sec = 1u64
                        .checked_shl(attempt)
                        .unwrap_or(MAX_BACKOFF_SEC)
                        .min(MAX_BACKOFF_SEC);
                    tracing::warn!(
                        "Enhance request failed (retry {}/{}), backoff {backoff_sec}s: {e}",
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
    fn is_retryable(err: &EnhanceError) -> bool {
        match err {
            EnhanceError::Network(_) | EnhanceError::Timeout => true,
            EnhanceError::ApiError { status, .. } => *status >= 500,
            _ => false,
        }
    }

    /// Одиночный HTTP-запрос к Responses API.
    async fn send_request(&self, url: &str, instructions: &str, input: &str) -> Result<String> {
        let body = ResponsesRequest {
            model: self.model.clone(),
            instructions: instructions.to_string(),
            input: input.to_string(),
        };

        let response = self
            .client
            .post(url)
            .header(header::AUTHORIZATION, format!("Bearer {}", self.api_key))
            .header(header::CONTENT_TYPE, "application/json")
            .timeout(self.read_timeout)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    EnhanceError::Timeout
                } else {
                    EnhanceError::Network(e.to_string())
                }
            })?;

        let status = response.status();

        // Обработка статусов дублирует stt/openai.rs - осознанное решение:
        // модули используют разные Error-типы и могут разойтись по логике.
        if status == StatusCode::UNAUTHORIZED {
            return Err(EnhanceError::AuthFailed);
        }

        if status == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(5)
                .clamp(1, 60);
            return Err(EnhanceError::RateLimited {
                retry_after_sec: retry_after,
            });
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(EnhanceError::ApiError {
                status: status.as_u16(),
                message: body,
            });
        }

        let resp: ResponsesResponse = response
            .json()
            .await
            .map_err(|e| EnhanceError::InvalidResponse(e.to_string()))?;

        extract_output_text(&resp)
    }
}

impl EnhanceProvider for OpenAiEnhancer {
    async fn enhance(&self, raw_text: &str, language: Option<&str>) -> Result<String> {
        self.do_enhance(raw_text, language).await
    }
}

/// Формирует системный промпт с учетом языка.
fn build_instructions(language: Option<&str>) -> String {
    match language {
        Some(lang) if lang != "auto" => SYSTEM_PROMPT_WITH_LANG.replace("{lang}", lang),
        _ => SYSTEM_PROMPT.to_string(),
    }
}

/// Извлекает текст из ответа Responses API.
fn extract_output_text(resp: &ResponsesResponse) -> Result<String> {
    let text: String = resp
        .output
        .iter()
        .flat_map(|item| item.content.iter())
        .map(|block| block.text.as_str())
        .collect::<Vec<_>>()
        .join("");

    if text.trim().is_empty() {
        return Err(EnhanceError::InvalidResponse(
            "empty output text in response".to_string(),
        ));
    }

    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_should_be_created_with_valid_params() {
        let client = OpenAiEnhancer::new(
            "https://api.openai.com",
            "test-key",
            "gpt-5-mini",
            Duration::from_secs(5),
            Duration::from_secs(15),
            3,
        );
        assert!(client.is_ok());
    }

    #[test]
    fn client_should_trim_trailing_slash_from_base_url() {
        // Given / When
        let client = OpenAiEnhancer::new(
            "https://api.openai.com/",
            "test-key",
            "gpt-5-mini",
            Duration::from_secs(5),
            Duration::from_secs(15),
            3,
        )
        .unwrap();

        // Then
        assert_eq!(client.base_url, "https://api.openai.com");
    }

    #[test]
    fn client_should_store_model_from_constructor() {
        // Given / When
        let client = OpenAiEnhancer::new(
            "https://api.openai.com",
            "key",
            "my-custom-model",
            Duration::from_secs(5),
            Duration::from_secs(15),
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
            enhance_model: "custom-enhance".to_string(),
            connect_timeout_sec: 10,
            read_timeout_enhance_sec: 30,
            retry_count: 5,
            ..Default::default()
        };

        // When
        let client = OpenAiEnhancer::from_config(&config, "api-key-123").unwrap();

        // Then
        assert_eq!(client.base_url, "https://custom.api.com");
        assert_eq!(client.model, "custom-enhance");
        assert_eq!(client.retry_count, 5);
        assert_eq!(client.read_timeout, Duration::from_secs(30));
    }

    #[test]
    fn is_retryable_should_return_true_for_network_error() {
        assert!(OpenAiEnhancer::is_retryable(&EnhanceError::Network(
            "err".into()
        )));
    }

    #[test]
    fn is_retryable_should_return_true_for_timeout() {
        assert!(OpenAiEnhancer::is_retryable(&EnhanceError::Timeout));
    }

    #[test]
    fn is_retryable_should_return_true_for_5xx() {
        assert!(OpenAiEnhancer::is_retryable(&EnhanceError::ApiError {
            status: 500,
            message: "internal error".into(),
        }));
        assert!(OpenAiEnhancer::is_retryable(&EnhanceError::ApiError {
            status: 503,
            message: "unavailable".into(),
        }));
    }

    #[test]
    fn is_retryable_should_return_false_for_auth_error() {
        assert!(!OpenAiEnhancer::is_retryable(&EnhanceError::AuthFailed));
    }

    #[test]
    fn is_retryable_should_return_false_for_400() {
        assert!(!OpenAiEnhancer::is_retryable(&EnhanceError::ApiError {
            status: 400,
            message: "bad request".into(),
        }));
    }

    #[test]
    fn is_retryable_should_return_false_for_rate_limited() {
        assert!(!OpenAiEnhancer::is_retryable(&EnhanceError::RateLimited {
            retry_after_sec: 5,
        }));
    }

    #[test]
    fn build_instructions_should_return_default_prompt_for_none() {
        let result = build_instructions(None);
        assert_eq!(result, SYSTEM_PROMPT);
    }

    #[test]
    fn build_instructions_should_return_default_prompt_for_auto() {
        let result = build_instructions(Some("auto"));
        assert_eq!(result, SYSTEM_PROMPT);
    }

    #[test]
    fn build_instructions_should_include_language() {
        let result = build_instructions(Some("ru"));
        assert!(result.contains("ru"));
        assert!(result.contains("text post-processor"));
    }

    #[test]
    fn extract_output_text_should_get_text_from_valid_response() {
        // Given
        let resp = ResponsesResponse {
            output: vec![OutputItem {
                content: vec![ContentBlock {
                    text: "Hello, world!".to_string(),
                }],
            }],
        };

        // When
        let result = extract_output_text(&resp);

        // Then
        assert_eq!(result.unwrap(), "Hello, world!");
    }

    #[test]
    fn extract_output_text_should_join_multiple_blocks() {
        // Given
        let resp = ResponsesResponse {
            output: vec![OutputItem {
                content: vec![
                    ContentBlock {
                        text: "Hello, ".to_string(),
                    },
                    ContentBlock {
                        text: "world!".to_string(),
                    },
                ],
            }],
        };

        // When
        let result = extract_output_text(&resp);

        // Then
        assert_eq!(result.unwrap(), "Hello, world!");
    }

    #[test]
    fn extract_output_text_should_fail_on_empty_output() {
        // Given
        let resp = ResponsesResponse { output: vec![] };

        // When
        let result = extract_output_text(&resp);

        // Then
        assert!(matches!(
            result.unwrap_err(),
            EnhanceError::InvalidResponse(_)
        ));
    }

    #[test]
    fn extract_output_text_should_fail_on_whitespace_only_text() {
        // Given
        let resp = ResponsesResponse {
            output: vec![OutputItem {
                content: vec![ContentBlock {
                    text: "   ".to_string(),
                }],
            }],
        };

        // When
        let result = extract_output_text(&resp);

        // Then
        assert!(matches!(
            result.unwrap_err(),
            EnhanceError::InvalidResponse(_)
        ));
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_responses_json(text: &str) -> serde_json::Value {
        serde_json::json!({
            "id": "resp_test",
            "output": [{
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": text
                }]
            }]
        })
    }

    async fn create_test_client(base_url: &str) -> OpenAiEnhancer {
        OpenAiEnhancer::new(
            base_url,
            "test-api-key",
            "gpt-5-mini",
            Duration::from_secs(5),
            Duration::from_secs(10),
            2,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn enhance_should_return_improved_text() {
        // Given
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(make_responses_json("Hello, world!")),
            )
            .mount(&server)
            .await;

        let client = create_test_client(&server.uri()).await;

        // When
        let result = client.do_enhance("hello world", None).await;

        // Then
        assert_eq!(result.unwrap(), "Hello, world!");
    }

    #[tokio::test]
    async fn enhance_should_fallback_on_empty_response() {
        // Given
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_json(make_responses_json("")))
            .mount(&server)
            .await;

        let client = create_test_client(&server.uri()).await;

        // When: empty output means InvalidResponse, which is non-retryable -> fallback
        let result = client.do_enhance("hello world", None).await;

        // Then: should return raw text
        assert_eq!(result.unwrap(), "hello world");
    }

    #[tokio::test]
    async fn enhance_should_fallback_on_hallucination() {
        // Given: model returns much more text than input
        let server = MockServer::start().await;
        let hallucinated = "this is a very long hallucinated response that has way too many \
            words compared to the original input text and should be detected as a hallucination \
            by our validation logic because the ratio exceeds the maximum allowed threshold";

        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(make_responses_json(hallucinated)),
            )
            .mount(&server)
            .await;

        let client = create_test_client(&server.uri()).await;

        // When
        let result = client.do_enhance("hello world test", None).await;

        // Then: fallback to raw text due to hallucination
        assert_eq!(result.unwrap(), "hello world test");
    }

    #[tokio::test]
    async fn enhance_should_retry_on_server_error() {
        // Given: first request -> 500, second -> 200
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(ResponseTemplate::new(500).set_body_string("error"))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(make_responses_json("Recovered text.")),
            )
            .mount(&server)
            .await;

        let client = create_test_client(&server.uri()).await;

        // When
        let result = client.do_enhance("recovered text", None).await;

        // Then
        assert_eq!(result.unwrap(), "Recovered text.");
    }

    #[tokio::test]
    async fn enhance_should_return_raw_on_api_failure() {
        // Given: always 401
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let client = create_test_client(&server.uri()).await;

        // When
        let result = client.do_enhance("my original text", None).await;

        // Then: non-retryable error -> fallback to raw
        assert_eq!(result.unwrap(), "my original text");
    }

    #[tokio::test]
    async fn enhance_should_return_raw_after_exhausting_retries() {
        // Given: always 500
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(ResponseTemplate::new(500).set_body_string("server error"))
            .mount(&server)
            .await;

        let client = create_test_client(&server.uri()).await;

        // When
        let result = client.do_enhance("my text here", None).await;

        // Then: retries exhausted -> fallback to raw
        assert_eq!(result.unwrap(), "my text here");
    }

    #[tokio::test]
    async fn enhance_should_handle_rate_limiting() {
        // Given: first -> 429, second -> 200
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(ResponseTemplate::new(429).append_header("Retry-After", "1"))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(make_responses_json("After rate limit.")),
            )
            .mount(&server)
            .await;

        let client = create_test_client(&server.uri()).await;

        // When
        let result = client.do_enhance("after rate limit", None).await;

        // Then
        assert_eq!(result.unwrap(), "After rate limit.");
    }

    #[tokio::test]
    async fn enhance_should_not_retry_on_400() {
        // Given
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
            .expect(1)
            .mount(&server)
            .await;

        let client = create_test_client(&server.uri()).await;

        // When
        let result = client.do_enhance("some text", None).await;

        // Then: non-retryable, returns raw
        assert_eq!(result.unwrap(), "some text");
    }

    #[tokio::test]
    async fn enhance_should_send_correct_request_body() {
        // Given
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .and(header("authorization", "Bearer test-api-key"))
            .and(header("content-type", "application/json"))
            .and(body_json(serde_json::json!({
                "model": "gpt-5-mini",
                "instructions": SYSTEM_PROMPT,
                "input": "test text"
            })))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(make_responses_json("Test text.")),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = create_test_client(&server.uri()).await;

        // When
        let result = client.do_enhance("test text", None).await;

        // Then
        assert_eq!(result.unwrap(), "Test text.");
    }

    #[tokio::test]
    async fn enhance_should_handle_invalid_json_response() {
        // Given
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;

        let client = create_test_client(&server.uri()).await;

        // When
        let result = client.do_enhance("original text here", None).await;

        // Then: InvalidResponse is non-retryable -> fallback
        assert_eq!(result.unwrap(), "original text here");
    }

    #[tokio::test]
    async fn enhance_should_timeout_on_slow_response() {
        // Given
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(make_responses_json("too late"))
                    .set_delay(Duration::from_secs(10)),
            )
            .mount(&server)
            .await;

        let client = OpenAiEnhancer::new(
            &server.uri(),
            "test-api-key",
            "gpt-5-mini",
            Duration::from_secs(5),
            Duration::from_millis(200),
            0, // no retries
        )
        .unwrap();

        // When
        let result = client.do_enhance("my text", None).await;

        // Then: timeout -> fallback to raw
        assert_eq!(result.unwrap(), "my text");
    }
}

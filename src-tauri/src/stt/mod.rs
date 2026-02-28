pub mod offline_whisper;
pub mod openai;

pub use self::openai::OpenAiSttClient;

/// Ошибки STT-модуля.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SttError {
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

    #[error("encoding failed: {0}")]
    EncodingFailed(String),
}

pub type Result<T> = std::result::Result<T, SttError>;

/// Контракт для провайдеров распознавания речи.
///
/// Позволяет подменять реализацию (online/offline) через generics.
pub trait SttProvider: Send + Sync {
    fn transcribe(
        &self,
        audio: &[u8],
        language: Option<&str>,
    ) -> impl std::future::Future<Output = Result<String>> + Send;
}

/// Один фрагмент аудио для отправки в STT.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub samples: Vec<f32>,
}

/// Overlap между чанками (секунды).
const CHUNK_OVERLAP_SEC: f32 = 1.5;

/// Минимальная длительность чанка (секунды), не делим дальше.
const MIN_CHUNK_SEC: f32 = 5.0;

/// Максимальная длительность одного чанка по умолчанию (секунды).
const DEFAULT_MAX_CHUNK_SEC: u32 = 30;

/// Начало зоны поиска тихого места для разреза (проценты от длины чанка).
/// Ищем тишину в последних (100 - QUIET_SEARCH_START_PERCENT)% чанка.
const QUIET_SEARCH_START_PERCENT: usize = 70;

/// Размер окна RMS-анализа энергии (миллисекунды).
const RMS_WINDOW_MS: u32 = 20;

/// Высокоуровневая функция: кодирует PCM в OGG/Opus и транскрибирует.
///
/// Если аудио укладывается в один чанк, кодирует и отправляет как есть.
/// Для длинных записей: разбивает на чанки, кодирует каждый,
/// транскрибирует последовательно (для экономии rate limit), склеивает текст.
pub async fn transcribe_audio<P: SttProvider>(
    provider: &P,
    samples: &[f32],
    sample_rate: u32,
    language: Option<&str>,
    max_chunk_sec: Option<u32>,
) -> Result<String> {
    if sample_rate == 0 {
        return Err(SttError::EncodingFailed(
            "sample_rate must be > 0".to_string(),
        ));
    }

    let max_sec = max_chunk_sec.unwrap_or(DEFAULT_MAX_CHUNK_SEC).max(1);
    let max_chunk_samples = max_sec as usize * sample_rate as usize;

    if samples.len() <= max_chunk_samples {
        let encoded = crate::audio::encode::encode_ogg_opus(samples, sample_rate)
            .map_err(|e| SttError::EncodingFailed(e.to_string()))?;
        return provider.transcribe(&encoded, language).await;
    }

    tracing::info!(
        "Audio too long ({:.1}s), splitting into chunks (max {}s each)",
        samples.len() as f32 / sample_rate as f32,
        max_sec
    );

    let chunks = chunk_audio(samples, sample_rate, max_sec);
    tracing::info!("Split into {} chunks", chunks.len());

    let mut texts = Vec::with_capacity(chunks.len());

    for (i, chunk) in chunks.iter().enumerate() {
        let encoded = crate::audio::encode::encode_ogg_opus(&chunk.samples, sample_rate)
            .map_err(|e| SttError::EncodingFailed(e.to_string()))?;

        tracing::debug!(
            "Transcribing chunk {}/{} ({:.1}s, {} bytes OGG)",
            i + 1,
            chunks.len(),
            chunk.samples.len() as f32 / sample_rate as f32,
            encoded.len()
        );

        let text = provider.transcribe(&encoded, language).await?;
        let text = text.trim().to_string();
        if !text.is_empty() {
            texts.push(text);
        }
    }

    Ok(deduplicate_overlap_texts(&texts))
}

/// Разбивает аудио на чанки подходящего размера.
///
/// Пытается резать по тихим местам (минимум энергии).
/// Если тихих мест нет, режет по таймеру с overlap.
pub fn chunk_audio(samples: &[f32], sample_rate: u32, max_chunk_sec: u32) -> Vec<AudioChunk> {
    if sample_rate == 0 || max_chunk_sec == 0 {
        tracing::warn!(
            "Invalid chunking params (sample_rate={sample_rate}, max_chunk_sec={max_chunk_sec}), \
             returning audio as single chunk"
        );
        return vec![AudioChunk {
            samples: samples.to_vec(),
        }];
    }

    let max_chunk_samples = max_chunk_sec as usize * sample_rate as usize;

    if samples.len() <= max_chunk_samples {
        return vec![AudioChunk {
            samples: samples.to_vec(),
        }];
    }

    let overlap_samples = (CHUNK_OVERLAP_SEC * sample_rate as f32) as usize;
    let min_chunk_samples = (MIN_CHUNK_SEC * sample_rate as f32) as usize;
    let mut chunks = Vec::new();
    let mut offset = 0;

    while offset < samples.len() {
        let remaining = samples.len() - offset;

        if remaining <= max_chunk_samples {
            chunks.push(AudioChunk {
                samples: samples[offset..].to_vec(),
            });
            break;
        }

        // Ищем тихое место в последних 30% окна чанка
        let search_start = offset + max_chunk_samples * QUIET_SEARCH_START_PERCENT / 100;
        let search_end = offset + max_chunk_samples;

        let split_point = find_quiet_split_point(&samples[search_start..search_end], sample_rate)
            .map(|p| search_start + p)
            .unwrap_or(offset + max_chunk_samples);

        // Не создаем слишком маленький хвостик
        let actual_end = if samples.len() - split_point < min_chunk_samples {
            samples.len()
        } else {
            split_point
        };

        chunks.push(AudioChunk {
            samples: samples[offset..actual_end].to_vec(),
        });

        if actual_end >= samples.len() {
            break;
        }

        offset = actual_end.saturating_sub(overlap_samples);
    }

    chunks
}

/// Ищет точку с минимальной энергией (тихий момент) в сегменте.
///
/// Анализирует окна по 20ms с шагом 10ms. Возвращает смещение внутри `segment`.
fn find_quiet_split_point(segment: &[f32], sample_rate: u32) -> Option<usize> {
    let window_size = (sample_rate * RMS_WINDOW_MS / 1000) as usize;
    if segment.len() < window_size * 2 {
        return None;
    }

    let mut min_energy = f32::MAX;
    let mut min_pos = 0;
    let step = window_size / 2;

    for start in (0..segment.len().saturating_sub(window_size)).step_by(step) {
        let window = &segment[start..start + window_size];
        let energy: f32 = window.iter().map(|s| s * s).sum::<f32>() / window.len() as f32;

        if energy < min_energy {
            min_energy = energy;
            min_pos = start + window_size / 2;
        }
    }

    Some(min_pos)
}

/// Склеивает тексты из overlapping чанков с простой дедупликацией.
///
/// Если последние N слов предыдущего текста совпадают с первыми N слов следующего,
/// дубликат удаляется (проверяет 2-5 слов, case-insensitive).
fn deduplicate_overlap_texts(texts: &[String]) -> String {
    if texts.is_empty() {
        return String::new();
    }
    if texts.len() == 1 {
        return texts[0].clone();
    }

    let mut result = texts[0].clone();

    for next in &texts[1..] {
        let overlap_words = find_text_overlap(&result, next);
        if overlap_words > 0 {
            let remaining: String = next
                .split_whitespace()
                .skip(overlap_words)
                .collect::<Vec<_>>()
                .join(" ");
            if !remaining.is_empty() {
                result.push(' ');
                result.push_str(&remaining);
            }
        } else {
            result.push(' ');
            result.push_str(next);
        }
    }

    result
}

/// Ищет overlap между концом `prev` и началом `next` (2-5 слов).
///
/// Возвращает количество совпавших слов (0 если overlap не найден).
fn find_text_overlap(prev: &str, next: &str) -> usize {
    let prev_words: Vec<&str> = prev.split_whitespace().collect();
    let next_words: Vec<&str> = next.split_whitespace().collect();

    let max_check = prev_words.len().min(next_words.len()).min(5);

    for n in (2..=max_check).rev() {
        let prev_tail = &prev_words[prev_words.len() - n..];
        let next_head = &next_words[..n];

        if prev_tail
            .iter()
            .zip(next_head.iter())
            .all(|(a, b)| a.to_lowercase() == b.to_lowercase())
        {
            return n;
        }
    }

    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Тестовый провайдер STT, возвращает заданные тексты по порядку.
    struct StubSttProvider {
        responses: Vec<std::result::Result<String, SttError>>,
        call_count: Arc<AtomicUsize>,
    }

    impl StubSttProvider {
        fn with_responses(responses: Vec<std::result::Result<String, SttError>>) -> Self {
            Self {
                responses,
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn call_count(&self) -> usize {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    impl SttProvider for StubSttProvider {
        async fn transcribe(&self, _audio: &[u8], _language: Option<&str>) -> Result<String> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            if idx < self.responses.len() {
                self.responses[idx].clone()
            } else {
                Ok("default".to_string())
            }
        }
    }

    // -- chunk_audio --

    #[test]
    fn chunk_audio_should_return_single_chunk_when_audio_is_short() {
        // Given
        let sample_rate = 16_000u32;
        let samples: Vec<f32> = vec![0.1; sample_rate as usize * 10]; // 10s

        // When
        let chunks = chunk_audio(&samples, sample_rate, 25);

        // Then
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].samples.len(), samples.len());
    }

    #[test]
    fn chunk_audio_should_split_long_audio() {
        // Given
        let sample_rate = 16_000u32;
        let samples: Vec<f32> = vec![0.1; sample_rate as usize * 60]; // 60s

        // When
        let chunks = chunk_audio(&samples, sample_rate, 25);

        // Then
        assert!(
            chunks.len() >= 2,
            "expected >= 2 chunks, got {}",
            chunks.len()
        );
    }

    #[test]
    fn chunk_audio_should_return_single_chunk_for_empty_audio() {
        // Given / When
        let chunks = chunk_audio(&[], 16_000, 25);

        // Then
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].samples.is_empty());
    }

    #[test]
    fn chunk_audio_should_not_create_tiny_trailing_chunk() {
        // Given: 27s, max_chunk=25, remainder=2s < MIN_CHUNK_SEC
        let sample_rate = 16_000u32;
        let samples: Vec<f32> = vec![0.0; sample_rate as usize * 27];

        // When
        let chunks = chunk_audio(&samples, sample_rate, 25);

        // Then: последний чанк >= MIN_CHUNK_SEC (или это единственный чанк)
        if chunks.len() > 1 {
            let last_sec = chunks.last().unwrap().samples.len() as f32 / sample_rate as f32;
            assert!(
                last_sec >= MIN_CHUNK_SEC,
                "trailing chunk too short: {last_sec}s"
            );
        }
    }

    #[test]
    fn chunk_audio_should_cover_all_samples() {
        // Given
        let sample_rate = 16_000u32;
        let total = sample_rate as usize * 55;
        let samples: Vec<f32> = (0..total).map(|i| (i as f32) / total as f32).collect();

        // When
        let chunks = chunk_audio(&samples, sample_rate, 25);

        // Then: первый и последний семпл присутствуют
        assert_eq!(chunks.first().unwrap().samples[0], samples[0]);
        assert_eq!(
            *chunks.last().unwrap().samples.last().unwrap(),
            *samples.last().unwrap()
        );
    }

    // -- find_quiet_split_point --

    #[test]
    fn find_quiet_split_point_should_find_silent_region() {
        // Given: loud - silent - loud
        let sample_rate = 16_000u32;
        let window = sample_rate as usize / 50; // 20ms = 320 samples
        let mut segment = vec![0.5f32; window * 10];
        for s in &mut segment[window * 4..window * 6] {
            *s = 0.001;
        }

        // When
        let split = find_quiet_split_point(&segment, sample_rate);

        // Then
        assert!(split.is_some());
        let pos = split.unwrap();
        assert!(
            pos >= window * 3 && pos <= window * 7,
            "split at {pos}, expected in [{}, {}]",
            window * 3,
            window * 7,
        );
    }

    #[test]
    fn find_quiet_split_point_should_return_none_for_short_segment() {
        // Given
        let segment = vec![0.1f32; 100];

        // When / Then
        assert!(find_quiet_split_point(&segment, 16_000).is_none());
    }

    // -- deduplicate_overlap_texts --

    #[test]
    fn deduplicate_should_return_empty_for_no_texts() {
        assert_eq!(deduplicate_overlap_texts(&[]), "");
    }

    #[test]
    fn deduplicate_should_return_single_text_as_is() {
        let texts = vec!["hello world".to_string()];
        assert_eq!(deduplicate_overlap_texts(&texts), "hello world");
    }

    #[test]
    fn deduplicate_should_concatenate_without_overlap() {
        let texts = vec!["hello world".to_string(), "foo bar".to_string()];
        assert_eq!(deduplicate_overlap_texts(&texts), "hello world foo bar");
    }

    #[test]
    fn deduplicate_should_remove_overlapping_words() {
        // Given
        let texts = vec![
            "the quick brown fox".to_string(),
            "brown fox jumps over".to_string(),
        ];

        // When / Then
        assert_eq!(
            deduplicate_overlap_texts(&texts),
            "the quick brown fox jumps over"
        );
    }

    #[test]
    fn deduplicate_should_be_case_insensitive() {
        let texts = vec!["Hello World".to_string(), "hello world again".to_string()];
        assert_eq!(deduplicate_overlap_texts(&texts), "Hello World again");
    }

    #[test]
    fn deduplicate_should_handle_three_chunks() {
        let texts = vec![
            "aaa bbb ccc ddd".to_string(),
            "ccc ddd eee fff".to_string(),
            "eee fff ggg hhh".to_string(),
        ];
        assert_eq!(
            deduplicate_overlap_texts(&texts),
            "aaa bbb ccc ddd eee fff ggg hhh"
        );
    }

    // -- find_text_overlap --

    #[test]
    fn find_overlap_should_return_zero_for_no_overlap() {
        assert_eq!(find_text_overlap("hello world", "foo bar"), 0);
    }

    #[test]
    fn find_overlap_should_detect_two_word_overlap() {
        assert_eq!(find_text_overlap("a b c d", "c d e f"), 2);
    }

    #[test]
    fn find_overlap_should_detect_three_word_overlap() {
        assert_eq!(find_text_overlap("a b c d e", "c d e f g"), 3);
    }

    #[test]
    fn find_overlap_should_not_detect_single_word() {
        assert_eq!(find_text_overlap("hello world", "world foo"), 0);
    }

    #[test]
    fn find_overlap_should_prefer_longer_match() {
        assert_eq!(find_text_overlap("x a b c", "a b c y"), 3);
    }

    // -- transcribe_audio --

    #[tokio::test]
    async fn transcribe_audio_should_use_single_request_for_short_audio() {
        // Given
        let provider = StubSttProvider::with_responses(vec![Ok("hello world".to_string())]);
        let samples = vec![0.1f32; 16_000 * 5]; // 5s

        // When
        let result = transcribe_audio(&provider, &samples, 16_000, None, Some(25)).await;

        // Then
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "hello world");
        assert_eq!(provider.call_count(), 1);
    }

    #[tokio::test]
    async fn transcribe_audio_should_split_long_audio() {
        // Given
        let provider = StubSttProvider::with_responses(vec![
            Ok("first part".to_string()),
            Ok("second part".to_string()),
            Ok("third part".to_string()),
            Ok("fourth part".to_string()),
        ]);
        let samples = vec![0.1f32; 16_000 * 60]; // 60s

        // When
        let result = transcribe_audio(&provider, &samples, 16_000, None, Some(25)).await;

        // Then
        assert!(result.is_ok());
        assert!(provider.call_count() >= 2, "expected >= 2 calls");
        let text = result.unwrap();
        assert!(text.contains("first part"));
    }

    #[tokio::test]
    async fn transcribe_audio_should_propagate_provider_error() {
        // Given
        let provider = StubSttProvider::with_responses(vec![Err(SttError::AuthFailed)]);
        let samples = vec![0.1f32; 16_000 * 5];

        // When
        let result = transcribe_audio(&provider, &samples, 16_000, None, Some(25)).await;

        // Then
        assert!(matches!(result.unwrap_err(), SttError::AuthFailed));
    }

    #[tokio::test]
    async fn transcribe_audio_should_skip_empty_chunk_results() {
        // Given
        let provider = StubSttProvider::with_responses(vec![
            Ok("first".to_string()),
            Ok("   ".to_string()), // empty after trim
            Ok("third".to_string()),
            Ok("fourth".to_string()),
        ]);
        let samples = vec![0.1f32; 16_000 * 60];

        // When
        let result = transcribe_audio(&provider, &samples, 16_000, None, Some(25)).await;

        // Then
        let text = result.unwrap();
        assert!(!text.contains("   "));
    }

    // -- SttError --

    #[test]
    fn stt_error_should_display_correctly() {
        assert_eq!(
            SttError::AuthFailed.to_string(),
            "authentication failed: check API key"
        );
        assert_eq!(SttError::Timeout.to_string(), "request timeout");
        assert_eq!(
            SttError::Network("conn refused".into()).to_string(),
            "network error: conn refused"
        );
        assert_eq!(
            SttError::RateLimited {
                retry_after_sec: 10
            }
            .to_string(),
            "rate limited, retry after 10s"
        );
        assert_eq!(
            SttError::ApiError {
                status: 500,
                message: "internal".into()
            }
            .to_string(),
            "API error (500): internal"
        );
    }

    // -- guard: invalid params --

    #[test]
    fn chunk_audio_should_return_single_chunk_when_max_chunk_sec_is_zero() {
        // Given: invalid max_chunk_sec = 0
        let samples = vec![0.1f32; 16_000 * 10];

        // When
        let chunks = chunk_audio(&samples, 16_000, 0);

        // Then: returns all audio as one chunk (fallback, no infinite loop)
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].samples.len(), samples.len());
    }

    #[test]
    fn chunk_audio_should_return_single_chunk_when_sample_rate_is_zero() {
        // Given: invalid sample_rate = 0
        let samples = vec![0.1f32; 1000];

        // When
        let chunks = chunk_audio(&samples, 0, 25);

        // Then: returns all audio as one chunk (fallback, no infinite loop)
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].samples.len(), samples.len());
    }

    #[tokio::test]
    async fn transcribe_audio_should_fail_when_sample_rate_is_zero() {
        // Given
        let provider = StubSttProvider::with_responses(vec![Ok("unused".to_string())]);
        let samples = vec![0.1f32; 1000];

        // When
        let result = transcribe_audio(&provider, &samples, 0, None, None).await;

        // Then
        assert!(matches!(result.unwrap_err(), SttError::EncodingFailed(_)));
    }

    #[tokio::test]
    async fn transcribe_audio_should_clamp_max_chunk_sec_zero_to_one() {
        // Given: max_chunk_sec = 0 should be clamped to 1, not cause infinite loop
        let provider = StubSttProvider::with_responses(vec![
            Ok("a".to_string()),
            Ok("b".to_string()),
            Ok("c".to_string()),
            Ok("d".to_string()),
            Ok("e".to_string()),
            Ok("f".to_string()),
        ]);
        let samples = vec![0.1f32; 16_000 * 3]; // 3 seconds

        // When
        let result = transcribe_audio(&provider, &samples, 16_000, None, Some(0)).await;

        // Then: should complete without hanging
        assert!(result.is_ok());
    }
}

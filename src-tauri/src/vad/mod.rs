pub mod silero;

use std::time::{Duration, Instant};

pub use self::silero::SileroVad;

/// Размер кадра для Silero VAD v5 при 16kHz (32ms).
pub const VAD_FRAME_SIZE: usize = 512;

/// Ошибки VAD-модуля.
#[derive(Debug, thiserror::Error)]
pub enum VadError {
    #[error("failed to load VAD model: {0}")]
    ModelLoadFailed(String),

    #[error("VAD inference failed: {0}")]
    InferenceFailed(String),

    #[error("invalid frame size: expected {expected}, got {got}")]
    InvalidFrameSize { expected: usize, got: usize },
}

pub type Result<T> = std::result::Result<T, VadError>;

/// Трейт для детекции голосовой активности.
///
/// Позволяет подменять реализацию (например, стабом) в тестах.
pub trait VoiceDetector {
    /// Определяет, содержит ли кадр аудио речь.
    fn is_speech(&mut self, frame: &[f32]) -> Result<bool>;

    /// Сбрасывает внутреннее состояние для нового аудио.
    fn reset(&mut self);
}

/// Результат обработки кадра детектором тишины.
#[derive(Debug, Clone, PartialEq)]
pub enum SilenceStatus {
    /// Обнаружена речь.
    Speech,
    /// Тишина, указана длительность с начала паузы.
    Silence(Duration),
    /// Порог тишины превышен - нужно остановить запись.
    SilenceTimeout,
}

/// Детектор тишины для auto-stop в toggle-режиме.
///
/// Оборачивает `VoiceDetector` и отслеживает длительность тишины.
/// Когда тишина превышает `threshold`, возвращает `SilenceTimeout`.
pub struct SilenceDetector<V: VoiceDetector> {
    vad: V,
    silence_start: Option<Instant>,
    threshold: Duration,
}

impl<V: VoiceDetector> SilenceDetector<V> {
    /// Создает детектор тишины с заданным порогом в секундах.
    ///
    /// Невалидные значения (`NaN`, `inf`, отрицательные) заменяются дефолтом (10 сек).
    pub fn new(vad: V, threshold_sec: f32) -> Self {
        const DEFAULT_THRESHOLD: f32 = 10.0;
        let safe_threshold = if threshold_sec.is_finite() && threshold_sec >= 0.0 {
            threshold_sec
        } else {
            DEFAULT_THRESHOLD
        };

        Self {
            vad,
            silence_start: None,
            threshold: Duration::from_secs_f32(safe_threshold),
        }
    }

    /// Обрабатывает один кадр аудио и возвращает статус.
    pub fn process_frame(&mut self, frame: &[f32]) -> Result<SilenceStatus> {
        let is_speech = self.vad.is_speech(frame)?;

        if is_speech {
            self.silence_start = None;
            return Ok(SilenceStatus::Speech);
        }

        let now = Instant::now();
        let silence_start = *self.silence_start.get_or_insert(now);
        let silence_duration = now.duration_since(silence_start);

        if silence_duration >= self.threshold {
            Ok(SilenceStatus::SilenceTimeout)
        } else {
            Ok(SilenceStatus::Silence(silence_duration))
        }
    }

    /// Сбрасывает состояние детектора и внутреннего VAD.
    pub fn reset(&mut self) {
        self.silence_start = None;
        self.vad.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubVad {
        responses: Vec<bool>,
        index: usize,
    }

    impl StubVad {
        fn new(responses: Vec<bool>) -> Self {
            Self {
                responses,
                index: 0,
            }
        }
    }

    impl VoiceDetector for StubVad {
        fn is_speech(&mut self, _frame: &[f32]) -> Result<bool> {
            let result = self.responses.get(self.index).copied().unwrap_or(false);
            self.index += 1;
            Ok(result)
        }

        fn reset(&mut self) {
            // Не сбрасываем index: StubVad возвращает ответы по порядку,
            // сброс индекса нарушает последовательность в тестах SilenceDetector.
        }
    }

    #[test]
    fn silence_detector_should_return_speech_when_vad_detects_voice() {
        // Given
        let vad = StubVad::new(vec![true]);
        let mut detector = SilenceDetector::new(vad, 5.0);
        let frame = vec![0.0; VAD_FRAME_SIZE];

        // When
        let status = detector.process_frame(&frame).unwrap();

        // Then
        assert_eq!(status, SilenceStatus::Speech);
    }

    #[test]
    fn silence_detector_should_return_silence_when_no_speech() {
        // Given
        let vad = StubVad::new(vec![false]);
        let mut detector = SilenceDetector::new(vad, 5.0);
        let frame = vec![0.0; VAD_FRAME_SIZE];

        // When
        let status = detector.process_frame(&frame).unwrap();

        // Then
        assert!(matches!(status, SilenceStatus::Silence(_)));
    }

    #[test]
    fn silence_detector_should_reset_timer_on_speech() {
        // Given: silence then speech then silence
        let vad = StubVad::new(vec![false, true, false]);
        let mut detector = SilenceDetector::new(vad, 5.0);
        let frame = vec![0.0; VAD_FRAME_SIZE];

        // When
        let _ = detector.process_frame(&frame).unwrap();
        let speech = detector.process_frame(&frame).unwrap();
        let after_speech = detector.process_frame(&frame).unwrap();

        // Then
        assert_eq!(speech, SilenceStatus::Speech);
        assert!(matches!(after_speech, SilenceStatus::Silence(_)));
    }

    #[test]
    fn silence_detector_should_timeout_after_threshold() {
        // Given
        let vad = StubVad::new(vec![false, false]);
        let threshold_sec = 0.0;
        let mut detector = SilenceDetector::new(vad, threshold_sec);
        let frame = vec![0.0; VAD_FRAME_SIZE];

        // When: первый кадр начинает тишину, второй превышает порог (0 сек)
        let _ = detector.process_frame(&frame).unwrap();
        let status = detector.process_frame(&frame).unwrap();

        // Then
        assert_eq!(status, SilenceStatus::SilenceTimeout);
    }

    #[test]
    fn silence_detector_should_reset_state() {
        // Given
        let vad = StubVad::new(vec![false, true]);
        let mut detector = SilenceDetector::new(vad, 5.0);
        let frame = vec![0.0; VAD_FRAME_SIZE];

        // When
        let _ = detector.process_frame(&frame).unwrap();
        detector.reset();
        let status = detector.process_frame(&frame).unwrap();

        // Then
        assert_eq!(status, SilenceStatus::Speech);
    }

    #[test]
    fn silence_detector_should_not_timeout_before_threshold() {
        // Given
        let vad = StubVad::new(vec![false]);
        let mut detector = SilenceDetector::new(vad, 60.0);
        let frame = vec![0.0; VAD_FRAME_SIZE];

        // When
        let status = detector.process_frame(&frame).unwrap();

        // Then
        assert!(matches!(status, SilenceStatus::Silence(_)));
        assert_ne!(status, SilenceStatus::SilenceTimeout);
    }

    #[test]
    fn vad_error_should_display_model_load_message() {
        // Given
        let error = VadError::ModelLoadFailed("file not found".to_string());

        // When
        let msg = error.to_string();

        // Then
        assert_eq!(msg, "failed to load VAD model: file not found");
    }

    #[test]
    fn vad_error_should_display_inference_message() {
        // Given
        let error = VadError::InferenceFailed("tensor mismatch".to_string());

        // When
        let msg = error.to_string();

        // Then
        assert_eq!(msg, "VAD inference failed: tensor mismatch");
    }

    #[test]
    fn vad_error_should_display_invalid_frame_size() {
        // Given
        let error = VadError::InvalidFrameSize {
            expected: 512,
            got: 256,
        };

        // When
        let msg = error.to_string();

        // Then
        assert_eq!(msg, "invalid frame size: expected 512, got 256");
    }

    #[test]
    fn silence_detector_should_use_default_threshold_for_nan() {
        // Given
        let vad = StubVad::new(vec![false]);

        // When
        let detector = SilenceDetector::new(vad, f32::NAN);

        // Then: should not panic, threshold = 10.0 sec default
        assert_eq!(detector.threshold, Duration::from_secs(10));
    }

    #[test]
    fn silence_detector_should_use_default_threshold_for_negative() {
        // Given
        let vad = StubVad::new(vec![false]);

        // When
        let detector = SilenceDetector::new(vad, -1.0);

        // Then
        assert_eq!(detector.threshold, Duration::from_secs(10));
    }

    #[test]
    fn silence_detector_should_use_default_threshold_for_infinity() {
        // Given
        let vad = StubVad::new(vec![false]);

        // When
        let detector = SilenceDetector::new(vad, f32::INFINITY);

        // Then
        assert_eq!(detector.threshold, Duration::from_secs(10));
    }

    #[test]
    fn silence_detector_should_accept_zero_threshold() {
        // Given
        let vad = StubVad::new(vec![false]);

        // When
        let detector = SilenceDetector::new(vad, 0.0);

        // Then: zero is a valid threshold (immediate timeout)
        assert_eq!(detector.threshold, Duration::from_secs(0));
    }
}

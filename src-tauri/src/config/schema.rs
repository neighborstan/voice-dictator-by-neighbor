use serde::{Deserialize, Serialize};

/// Режим записи: toggle (нажал-говоришь-нажал) или push-to-talk (удержание).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingMode {
    #[default]
    Toggle,
    PushToTalk,
}

/// Основная структура конфигурации приложения.
///
/// Хранится в JSON-файле в app config dir. Все дефолты - из ТЗ.
/// API-ключ хранится отдельно в OS keychain (не здесь).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct AppConfig {
    /// Версия схемы конфига (для будущих миграций)
    pub config_version: u32,

    /// Глобальный хоткей записи
    pub hotkey: String,

    /// Режим записи
    pub recording_mode: RecordingMode,

    /// Язык распознавания: "auto", "ru", "en"
    pub language: String,

    /// Модель STT (строка, никакого хардкода)
    pub stt_model: String,

    /// Модель улучшения текста (строка, никакого хардкода)
    pub enhance_model: String,

    /// Включено ли улучшение текста
    pub enhance_enabled: bool,

    /// Авто-стоп по тишине (VAD)
    pub vad_auto_stop: bool,

    /// Порог тишины для авто-стопа (секунды)
    pub vad_silence_threshold_sec: f32,

    /// Обрезать тишину в начале/конце аудио
    pub vad_trim_silence: bool,

    /// Максимальная длительность записи (секунды, 10-120)
    pub max_recording_duration_sec: u32,

    /// Минимальная длительность записи (миллисекунды)
    pub min_recording_duration_ms: u32,

    /// Показывать уведомления
    pub show_notifications: bool,

    /// Базовый URL OpenAI API
    pub api_base_url: String,

    /// Таймаут подключения (секунды)
    pub connect_timeout_sec: u32,

    /// Таймаут чтения для STT-запросов (секунды)
    pub read_timeout_stt_sec: u32,

    /// Таймаут чтения для enhance-запросов (секунды)
    pub read_timeout_enhance_sec: u32,

    /// Количество повторных попыток при сетевых ошибках
    pub retry_count: u32,

    /// Уровень логирования: "trace", "debug", "info", "warn", "error"
    pub log_level: String,

    /// Сохранять последний аудиофайл для отладки
    pub debug_save_audio: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            config_version: 1,
            hotkey: "Ctrl+Shift+S".to_string(),
            recording_mode: RecordingMode::default(),
            language: "auto".to_string(),
            stt_model: "gpt-4o-mini-transcribe".to_string(),
            enhance_model: "gpt-5-mini".to_string(),
            enhance_enabled: true,
            vad_auto_stop: true,
            vad_silence_threshold_sec: 5.0,
            vad_trim_silence: true,
            max_recording_duration_sec: 60,
            min_recording_duration_ms: 300,
            show_notifications: true,
            api_base_url: "https://api.openai.com".to_string(),
            connect_timeout_sec: 5,
            read_timeout_stt_sec: 30,
            read_timeout_enhance_sec: 15,
            retry_count: 3,
            log_level: "info".to_string(),
            debug_save_audio: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_should_have_expected_values() {
        // Given / When
        let config = AppConfig::default();

        // Then
        assert_eq!(config.config_version, 1);
        assert_eq!(config.hotkey, "Ctrl+Shift+S");
        assert_eq!(config.recording_mode, RecordingMode::Toggle);
        assert_eq!(config.language, "auto");
        assert_eq!(config.stt_model, "gpt-4o-mini-transcribe");
        assert_eq!(config.enhance_model, "gpt-5-mini");
        assert!(config.enhance_enabled);
        assert!(config.vad_auto_stop);
        assert!((config.vad_silence_threshold_sec - 5.0).abs() < f32::EPSILON);
        assert!(config.vad_trim_silence);
        assert_eq!(config.max_recording_duration_sec, 60);
        assert_eq!(config.min_recording_duration_ms, 300);
        assert!(config.show_notifications);
        assert_eq!(config.api_base_url, "https://api.openai.com");
        assert_eq!(config.connect_timeout_sec, 5);
        assert_eq!(config.read_timeout_stt_sec, 30);
        assert_eq!(config.read_timeout_enhance_sec, 15);
        assert_eq!(config.retry_count, 3);
        assert_eq!(config.log_level, "info");
        assert!(!config.debug_save_audio);
    }

    #[test]
    fn default_recording_mode_should_be_toggle() {
        assert_eq!(RecordingMode::default(), RecordingMode::Toggle);
    }

    #[test]
    fn config_should_roundtrip_json_serialization() {
        // Given
        let config = AppConfig::default();

        // When
        let json = serde_json::to_string_pretty(&config).expect("serialize");
        let restored: AppConfig = serde_json::from_str(&json).expect("deserialize");

        // Then
        assert_eq!(restored, config);
    }

    #[test]
    fn recording_mode_should_serialize_as_snake_case() {
        // Given
        let toggle = RecordingMode::Toggle;
        let ptt = RecordingMode::PushToTalk;

        // When / Then
        assert_eq!(serde_json::to_string(&toggle).unwrap(), "\"toggle\"");
        assert_eq!(serde_json::to_string(&ptt).unwrap(), "\"push_to_talk\"");
    }
}

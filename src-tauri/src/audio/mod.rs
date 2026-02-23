pub mod capture_cpal;
pub mod encode;
pub mod preprocess;

/// Метаданные захваченного аудио (формат устройства).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CaptureFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

/// Ошибки аудио-модуля.
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum AudioError {
    #[error("no audio input device found")]
    NoInputDevice,

    #[error("failed to get default input config: {0}")]
    NoInputConfig(String),

    #[error("audio capture failed: {0}")]
    CaptureFailed(String),

    #[error("already recording")]
    AlreadyRecording,

    #[error("capture not started")]
    NotRecording,

    #[error("encoding failed: {0}")]
    EncodingFailed(String),
}

#[allow(dead_code)]
pub type Result<T> = std::result::Result<T, AudioError>;

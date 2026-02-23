use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};

use super::{AudioError, CaptureFormat, Result};

/// Захват аудио с микрофона через cpal.
///
/// Накапливает PCM-данные в RAM-буфере. Формат устройства
/// сохраняется для последующего препроцессинга.
#[allow(dead_code)]
pub struct AudioCapture {
    stream: Option<Stream>,
    buffer: Arc<Mutex<Vec<f32>>>,
    format: Option<CaptureFormat>,
    is_recording: Arc<AtomicBool>,
}

#[allow(dead_code)]
impl AudioCapture {
    /// Создает AudioCapture с дефолтным input device.
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or(AudioError::NoInputDevice)?;

        let device_name = device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| String::from("unknown"));
        tracing::info!(device = device_name, "audio input device selected");

        Ok(Self {
            stream: None,
            buffer: Arc::new(Mutex::new(Vec::new())),
            format: None,
            is_recording: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Начинает запись с микрофона.
    ///
    /// PCM-данные накапливаются в RAM-буфере как f32.
    /// Формат устройства (sample rate, channels) сохраняется.
    pub fn start_recording(&mut self) -> Result<()> {
        if self.is_recording() {
            return Err(AudioError::AlreadyRecording);
        }

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or(AudioError::NoInputDevice)?;

        let config = device
            .default_input_config()
            .map_err(|e| AudioError::NoInputConfig(e.to_string()))?;

        let device_sample_rate = config.sample_rate();
        let channels = config.channels();
        let sample_format = config.sample_format();

        self.buffer.lock().expect("buffer mutex poisoned").clear();

        let buffer = Arc::clone(&self.buffer);
        let is_recording = Arc::clone(&self.is_recording);

        let err_callback = |err: cpal::StreamError| {
            tracing::error!(error = %err, "audio stream error");
        };

        let stream = match sample_format {
            SampleFormat::F32 => {
                let stream = device
                    .build_input_stream(
                        &config.into(),
                        move |data: &[f32], _: &cpal::InputCallbackInfo| {
                            if is_recording.load(Ordering::SeqCst) {
                                if let Ok(mut buf) = buffer.lock() {
                                    buf.extend_from_slice(data);
                                }
                            }
                        },
                        err_callback,
                        None,
                    )
                    .map_err(|e| AudioError::CaptureFailed(e.to_string()))?;
                stream
            }
            SampleFormat::I16 => {
                let stream = device
                    .build_input_stream(
                        &config.into(),
                        move |data: &[i16], _: &cpal::InputCallbackInfo| {
                            if is_recording.load(Ordering::SeqCst) {
                                if let Ok(mut buf) = buffer.lock() {
                                    buf.extend(data.iter().map(|&s| s as f32 / i16::MAX as f32));
                                }
                            }
                        },
                        err_callback,
                        None,
                    )
                    .map_err(|e| AudioError::CaptureFailed(e.to_string()))?;
                stream
            }
            SampleFormat::U16 => {
                let stream = device
                    .build_input_stream(
                        &config.into(),
                        move |data: &[u16], _: &cpal::InputCallbackInfo| {
                            if is_recording.load(Ordering::SeqCst) {
                                if let Ok(mut buf) = buffer.lock() {
                                    buf.extend(
                                        data.iter()
                                            .map(|&s| (s as f32 / u16::MAX as f32) * 2.0 - 1.0),
                                    );
                                }
                            }
                        },
                        err_callback,
                        None,
                    )
                    .map_err(|e| AudioError::CaptureFailed(e.to_string()))?;
                stream
            }
            _ => {
                return Err(AudioError::CaptureFailed(format!(
                    "unsupported sample format: {sample_format:?}"
                )));
            }
        };

        stream
            .play()
            .map_err(|e| AudioError::CaptureFailed(e.to_string()))?;

        self.stream = Some(stream);
        self.format = Some(CaptureFormat {
            sample_rate: device_sample_rate,
            channels,
        });
        self.is_recording.store(true, Ordering::SeqCst);

        tracing::info!(
            sample_rate = device_sample_rate,
            channels = channels,
            format = ?sample_format,
            "audio recording started"
        );

        Ok(())
    }

    /// Останавливает запись и возвращает захваченный буфер + формат.
    ///
    /// После вызова stream уничтожается, буфер очищается.
    pub fn stop_recording(&mut self) -> Result<(Vec<f32>, CaptureFormat)> {
        self.is_recording.store(false, Ordering::SeqCst);

        // Drop stream для остановки
        self.stream.take();

        let format = self.format.take().ok_or(AudioError::NotRecording)?;

        let samples = {
            let mut buf = self.buffer.lock().expect("buffer mutex poisoned");
            std::mem::take(&mut *buf)
        };

        tracing::info!(
            samples = samples.len(),
            sample_rate = format.sample_rate,
            channels = format.channels,
            "audio recording stopped"
        );

        Ok((samples, format))
    }

    /// Проверяет, идет ли запись.
    pub fn is_recording(&self) -> bool {
        self.is_recording.load(Ordering::SeqCst)
    }
}

//! Pipeline orchestration: hotkey -> recording -> STT -> enhance -> paste.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

use crate::audio::capture_cpal::AudioCapture;
use crate::audio::preprocess::{self, TARGET_SAMPLE_RATE};
use crate::audio::CaptureFormat;
use crate::config::schema::AppConfig;
use crate::enhance::{EnhanceProvider, OpenAiEnhancer};
use crate::notifications;
use crate::paste::{self, PasteStatus};
use crate::state::{AppEvent, SharedAppState};
use crate::stt::{self, OpenAiSttClient};
use crate::tray;

#[cfg(target_os = "macos")]
const PASTE_SHORTCUT: &str = "Cmd+V";
#[cfg(not(target_os = "macos"))]
const PASTE_SHORTCUT: &str = "Ctrl+V";

/// Состояние pipeline, управляемое Tauri.
///
/// Хранит активный захват аудио, флаг отмены, таймаут безопасности
/// и handle задачи pipeline для принудительного abort при отмене.
pub struct PipelineState {
    capture: Mutex<Option<AudioCapture>>,
    cancel: Arc<AtomicBool>,
    timeout_handle: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    pipeline_handle: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

impl PipelineState {
    pub fn new() -> Self {
        Self {
            capture: Mutex::new(None),
            cancel: Arc::new(AtomicBool::new(false)),
            timeout_handle: Mutex::new(None),
            pipeline_handle: Mutex::new(None),
        }
    }
}

/// Текст для окна результата (показывается когда буфер обмена недоступен).
pub struct ResultText(pub Mutex<Option<String>>);

impl ResultText {
    pub fn new() -> Self {
        Self(Mutex::new(None))
    }
}

// --- Public API (called from dispatch_and_update) ---

/// Запускает захват аудио и таймаут безопасности.
///
/// Вызывается при переходе состояния Idle -> Recording.
/// При ошибке: переходит в Error -> Idle с уведомлением.
pub fn start_recording<R: Runtime>(app: &AppHandle<R>) {
    let pipeline = app.state::<PipelineState>();

    // Прерываем оставшуюся задачу предыдущего pipeline, чтобы отмененный
    // и сразу перезапущенный pipeline не "ожил" после возврата из await.
    if let Some(old_handle) = pipeline
        .pipeline_handle
        .lock()
        .expect("pipeline_handle mutex poisoned")
        .take()
    {
        old_handle.abort();
        tracing::debug!("aborted leftover pipeline task on new recording start");
    }

    pipeline.cancel.store(false, Ordering::SeqCst);

    let mut capture = match AudioCapture::new() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "failed to create audio capture");
            handle_pipeline_error(app, &format!("Microphone not available: {e}"));
            return;
        }
    };

    if let Err(e) = capture.start_recording() {
        tracing::error!(error = %e, "failed to start recording");
        handle_pipeline_error(app, &format!("Failed to start recording: {e}"));
        return;
    }

    *pipeline.capture.lock().expect("capture mutex poisoned") = Some(capture);

    // Таймаут безопасности: авто-остановка по истечении max_recording_duration_sec
    let max_sec = app
        .state::<Mutex<AppConfig>>()
        .lock()
        .expect("config mutex poisoned")
        .max_recording_duration_sec;

    let app_handle = app.clone();
    let handle = tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(max_sec as u64)).await;
        tracing::info!(seconds = max_sec, "safety timeout reached, auto-stopping");
        crate::dispatch_and_update(&app_handle, AppEvent::MaxDurationTimeout);
    });

    *pipeline
        .timeout_handle
        .lock()
        .expect("timeout mutex poisoned") = Some(handle);
}

/// Останавливает захват аудио и запускает pipeline обработки.
///
/// Вызывается при переходе состояния Recording -> Transcribing.
pub fn stop_recording_and_run_pipeline<R: Runtime>(app: &AppHandle<R>) {
    let pipeline = app.state::<PipelineState>();

    if let Some(handle) = pipeline
        .timeout_handle
        .lock()
        .expect("timeout mutex poisoned")
        .take()
    {
        handle.abort();
    }

    let mut capture = match pipeline
        .capture
        .lock()
        .expect("capture mutex poisoned")
        .take()
    {
        Some(c) => c,
        None => {
            tracing::warn!("no active recording to stop");
            abort_pipeline(app);
            return;
        }
    };

    let (audio, format) = match capture.stop_recording() {
        Ok(data) => data,
        Err(e) => {
            tracing::error!(error = %e, "failed to stop recording");
            handle_pipeline_error(app, &format!("Failed to stop recording: {e}"));
            return;
        }
    };

    let config = app
        .state::<Mutex<AppConfig>>()
        .lock()
        .expect("config mutex poisoned")
        .clone();

    let api_key = match crate::config::secrets::load_api_key() {
        Ok(Some(key)) => key,
        Ok(None) => {
            handle_pipeline_error(app, "API key not configured");
            return;
        }
        Err(e) => {
            handle_pipeline_error(app, &format!("Failed to load API key: {e}"));
            return;
        }
    };

    let cancel = Arc::clone(&pipeline.cancel);
    let app_handle = app.clone();

    let handle = tauri::async_runtime::spawn(async move {
        run_pipeline(app_handle, audio, format, config, api_key, cancel).await;
    });

    *pipeline
        .pipeline_handle
        .lock()
        .expect("pipeline_handle mutex poisoned") = Some(handle);
}

/// Устанавливает флаг отмены для прерывания pipeline.
///
/// Вызывается при отмене обработки (хоткей или трей во время Transcribing/Enhancing/Pasting).
pub fn cancel_pipeline<R: Runtime>(app: &AppHandle<R>) {
    let pipeline = app.state::<PipelineState>();
    pipeline.cancel.store(true, Ordering::SeqCst);
    tracing::info!("pipeline cancellation requested");

    let timeout = pipeline
        .timeout_handle
        .lock()
        .expect("timeout mutex poisoned")
        .take();
    if let Some(handle) = timeout {
        handle.abort();
    }

    // Прерываем задачу pipeline, чтобы гарантировать остановку даже если
    // новая запись стартует и сбросит флаг cancel.
    let task = pipeline
        .pipeline_handle
        .lock()
        .expect("pipeline_handle mutex poisoned")
        .take();
    if let Some(handle) = task {
        handle.abort();
        tracing::debug!("pipeline task aborted");
    }
}

// --- Pipeline core ---

/// Результат обработки аудио-pipeline (до вставки/доставки).
#[derive(Debug)]
pub(crate) enum ProcessingOutcome {
    /// Текст готов к вставке или показу в окне результата.
    Text(String),
    /// Запись слишком короткая после обрезки тишины.
    TooShort { duration_ms: u32 },
    /// STT вернул пустой текст (речь не обнаружена).
    NoSpeech,
    /// Обработка отменена пользователем.
    Cancelled,
    /// Неустранимая ошибка в процессе обработки.
    Error(String),
}

/// Основная обработка аудио: препроцессинг -> обрезка -> STT -> enhance.
///
/// Без зависимостей от Tauri. Вызывает `on_transcription_done` после STT,
/// чтобы вызывающий код мог обновить UI-состояние (Transcribing -> Enhancing).
pub(crate) async fn process_audio(
    audio: &[f32],
    format: &CaptureFormat,
    config: &AppConfig,
    api_key: &str,
    cancel: &AtomicBool,
    on_transcription_done: impl FnOnce() + Send,
) -> ProcessingOutcome {
    let pipeline_start = Instant::now();
    let is_cancelled = || cancel.load(Ordering::SeqCst);

    tracing::info!(
        samples = audio.len(),
        sample_rate = format.sample_rate,
        channels = format.channels,
        "pipeline processing started"
    );

    // Шаг 1: Препроцессинг (моно 16кГц)
    let step = Instant::now();
    let processed = preprocess::preprocess(audio, format.channels, format.sample_rate);
    tracing::info!(
        ms = step.elapsed().as_millis() as u64,
        samples = processed.len(),
        "preprocess complete"
    );

    // Шаг 2: Обрезка тишины (если включено)
    let trimmed = if config.vad_trim_silence {
        let step = Instant::now();
        let result = preprocess::trim_silence(&processed, TARGET_SAMPLE_RATE);
        tracing::info!(
            ms = step.elapsed().as_millis() as u64,
            before = processed.len(),
            after = result.len(),
            "trim silence complete"
        );
        result.to_vec()
    } else {
        processed
    };

    // Шаг 3: Проверка минимальной длительности
    let duration_ms = (trimmed.len() as u64 * 1000 / TARGET_SAMPLE_RATE as u64) as u32;
    if duration_ms < config.min_recording_duration_ms {
        tracing::info!(
            duration_ms,
            min_ms = config.min_recording_duration_ms,
            "recording too short"
        );
        return ProcessingOutcome::TooShort { duration_ms };
    }

    if is_cancelled() {
        tracing::info!("pipeline cancelled before STT");
        return ProcessingOutcome::Cancelled;
    }

    // Шаг 4: STT
    let step = Instant::now();
    let language = match config.language.as_str() {
        "auto" => None,
        lang => Some(lang),
    };

    let stt_client = match OpenAiSttClient::from_config(config, api_key) {
        Ok(c) => c,
        Err(e) => {
            return ProcessingOutcome::Error(format!("STT client error: {e}"));
        }
    };

    let raw_text = match stt::transcribe_audio(
        Arc::new(stt_client),
        &trimmed,
        TARGET_SAMPLE_RATE,
        language,
        None,
        None,
    )
    .await
    {
        Ok(text) => text,
        Err(e) => {
            return ProcessingOutcome::Error(format!("Transcription failed: {e}"));
        }
    };

    tracing::info!(
        ms = step.elapsed().as_millis() as u64,
        chars = raw_text.len(),
        "STT complete"
    );

    if raw_text.trim().is_empty() {
        tracing::info!("STT returned empty text");
        return ProcessingOutcome::NoSpeech;
    }

    if is_cancelled() {
        tracing::info!("pipeline cancelled after STT");
        return ProcessingOutcome::Cancelled;
    }

    // Уведомляем вызывающий код о завершении STT (для перехода состояния UI)
    on_transcription_done();

    // Шаг 5: Enhance (если включено)
    let text = if config.enhance_enabled {
        let step = Instant::now();
        match enhance_text(config, api_key, &raw_text, language).await {
            Ok(enhanced) => {
                tracing::info!(
                    ms = step.elapsed().as_millis() as u64,
                    raw_chars = raw_text.len(),
                    enhanced_chars = enhanced.len(),
                    "enhance complete"
                );
                enhanced
            }
            Err(e) => {
                tracing::warn!(error = %e, "enhance failed, using raw text");
                raw_text
            }
        }
    } else {
        tracing::debug!("enhance disabled, using raw text");
        raw_text
    };

    if is_cancelled() {
        tracing::info!("pipeline cancelled after enhance");
        return ProcessingOutcome::Cancelled;
    }

    tracing::info!(
        total_ms = pipeline_start.elapsed().as_millis() as u64,
        chars = text.len(),
        "pipeline processing completed"
    );

    ProcessingOutcome::Text(text)
}

/// Полный pipeline диктовки: препроцессинг -> STT -> enhance -> вставка.
///
/// Запускается асинхронно после остановки записи. Делегирует основную обработку
/// в `process_audio`, сам занимается вставкой и переходами состояний Tauri.
async fn run_pipeline<R: Runtime>(
    app: AppHandle<R>,
    audio: Vec<f32>,
    format: CaptureFormat,
    config: AppConfig,
    api_key: String,
    cancel: Arc<AtomicBool>,
) {
    let pipeline_start = Instant::now();

    let app_for_transition = app.clone();
    let outcome = process_audio(&audio, &format, &config, &api_key, &cancel, move || {
        dispatch_pipeline_event(&app_for_transition, AppEvent::TranscriptionDone)
    })
    .await;

    match outcome {
        ProcessingOutcome::Text(text) => {
            // Переход: Enhancing -> Pasting
            dispatch_pipeline_event(&app, AppEvent::EnhancementDone);

            // Вставка (в отдельном потоке для чистого Win32-состояния)
            let step = Instant::now();
            let text_for_paste = text.clone();
            let status = tokio::task::spawn_blocking(move || paste::paste_text(&text_for_paste))
                .await
                .unwrap_or_else(|e| {
                    tracing::error!(error = %e, "paste thread panicked");
                    PasteStatus::ResultWindow
                });
            tracing::info!(
                ms = step.elapsed().as_millis() as u64,
                status = ?status,
                "paste complete"
            );

            match status {
                PasteStatus::Pasted => {}
                PasteStatus::ClipboardOnly => {
                    notifications::notify_info(
                        &app,
                        &format!("Text copied to clipboard (paste with {PASTE_SHORTCUT})"),
                    );
                }
                PasteStatus::ResultWindow => {
                    show_result_window(&app, &text);
                }
            }

            // Переход: Pasting -> Idle
            dispatch_pipeline_event(&app, AppEvent::PasteDone);
        }
        ProcessingOutcome::TooShort { duration_ms } => {
            tracing::info!(duration_ms, "recording too short");
            notifications::notify_error(&app, "Recording too short, try again");
            abort_pipeline(&app);
        }
        ProcessingOutcome::NoSpeech => {
            notifications::notify_error(&app, "No speech detected");
            abort_pipeline(&app);
        }
        ProcessingOutcome::Cancelled => {
            tracing::info!("pipeline cancelled");
        }
        ProcessingOutcome::Error(msg) => {
            handle_pipeline_error(&app, &msg);
        }
    }

    tracing::info!(
        total_ms = pipeline_start.elapsed().as_millis() as u64,
        "pipeline completed"
    );
}

// --- Helpers ---

/// Улучшает текст через OpenAI Responses API.
async fn enhance_text(
    config: &AppConfig,
    api_key: &str,
    raw_text: &str,
    language: Option<&str>,
) -> std::result::Result<String, String> {
    let enhancer =
        OpenAiEnhancer::from_config(config, api_key).map_err(|e| format!("enhance init: {e}"))?;
    enhancer
        .enhance(raw_text, language)
        .await
        .map_err(|e| e.to_string())
}

/// Отправляет событие pipeline: переход состояния + трей + уведомление.
fn dispatch_pipeline_event<R: Runtime>(app: &AppHandle<R>, event: AppEvent) {
    let shared = app.state::<SharedAppState>();
    let (old, new) = shared.dispatch_with_old(&event);
    if old != new {
        tray::update_tray(app, new);
        notifications::notify_state_change(app, old, new);
    }
}

/// Обрабатывает ошибку pipeline: уведомление, переход в Error, авто-восстановление в Idle.
fn handle_pipeline_error<R: Runtime>(app: &AppHandle<R>, message: &str) {
    tracing::error!("pipeline error: {message}");
    notifications::notify_error(app, message);

    let shared = app.state::<SharedAppState>();

    let (_, error_state) = shared.dispatch_with_old(&AppEvent::Failed(message.to_string()));
    tray::update_tray(app, error_state);

    // Авто-восстановление в Idle
    let (_, idle_state) = shared.dispatch_with_old(&AppEvent::ErrorAcknowledged);
    tray::update_tray(app, idle_state);
}

/// Прерывает pipeline штатно -> Idle (слишком короткая запись, нет речи и т.д.).
fn abort_pipeline<R: Runtime>(app: &AppHandle<R>) {
    let shared = app.state::<SharedAppState>();
    let (old, new) = shared.dispatch_with_old(&AppEvent::Cancel);
    if old != new {
        tray::update_tray(app, new);
    }
}

/// Сохраняет текст и открывает/обновляет окно результата.
fn show_result_window<R: Runtime>(app: &AppHandle<R>, text: &str) {
    let result = app.state::<ResultText>();
    *result.0.lock().expect("result mutex poisoned") = Some(text.to_string());

    if let Some(window) = app.get_webview_window("result") {
        if let Err(e) = app.emit("result-text-updated", ()) {
            tracing::warn!(error = %e, "failed to emit result-text-updated event");
        }
        if let Err(e) = window.unminimize() {
            tracing::warn!(error = %e, "failed to unminimize result window");
        }
        if let Err(e) = window.show() {
            tracing::warn!(error = %e, "failed to show result window");
        }
        if let Err(e) = window.set_focus() {
            tracing::warn!(error = %e, "failed to focus result window");
        }
        return;
    }

    match WebviewWindowBuilder::new(app, "result", WebviewUrl::App("/result".into()))
        .title("VoiceDictator - Result")
        .inner_size(400.0, 300.0)
        .always_on_top(true)
        .center()
        .resizable(true)
        .build()
    {
        Ok(_) => tracing::info!("result window opened"),
        Err(e) => tracing::error!(error = %e, "failed to open result window"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_state_should_initialize_with_defaults() {
        // Given / When
        let state = PipelineState::new();

        // Then
        assert!(state.capture.lock().unwrap().is_none());
        assert!(!state.cancel.load(Ordering::SeqCst));
        assert!(state.timeout_handle.lock().unwrap().is_none());
        assert!(state.pipeline_handle.lock().unwrap().is_none());
    }

    #[test]
    fn cancel_flag_should_toggle() {
        // Given
        let state = PipelineState::new();
        assert!(!state.cancel.load(Ordering::SeqCst));

        // When / Then
        state.cancel.store(true, Ordering::SeqCst);
        assert!(state.cancel.load(Ordering::SeqCst));

        state.cancel.store(false, Ordering::SeqCst);
        assert!(!state.cancel.load(Ordering::SeqCst));
    }

    #[test]
    fn result_text_should_store_and_retrieve() {
        // Given
        let result = ResultText::new();
        assert!(result.0.lock().unwrap().is_none());

        // When
        *result.0.lock().unwrap() = Some("Hello world".to_string());

        // Then
        assert_eq!(result.0.lock().unwrap().as_deref(), Some("Hello world"));
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::audio::CaptureFormat;
    use crate::config::schema::AppConfig;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Генерирует синусоидальный тон 440Гц с заданной частотой дискретизации.
    fn generate_tone(sample_rate: u32, duration_ms: u32, amplitude: f32) -> Vec<f32> {
        let num_samples = (sample_rate * duration_ms / 1000) as usize;
        (0..num_samples)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                amplitude * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
            })
            .collect()
    }

    fn make_test_config(base_url: &str) -> AppConfig {
        AppConfig {
            api_base_url: base_url.to_string(),
            min_recording_duration_ms: 300,
            vad_trim_silence: false,
            enhance_enabled: true,
            connect_timeout_sec: 5,
            read_timeout_stt_sec: 10,
            read_timeout_enhance_sec: 10,
            retry_count: 0,
            language: "auto".to_string(),
            ..Default::default()
        }
    }

    fn make_test_format() -> CaptureFormat {
        CaptureFormat {
            sample_rate: 16000,
            channels: 1,
        }
    }

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

    #[tokio::test]
    async fn pipeline_should_complete_happy_path_with_mock_stt_and_enhance() {
        // Given: mock STT возвращает сырой текст, mock enhance - улучшенный.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "text": "hello world" })),
            )
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(make_responses_json("Hello, world!")),
            )
            .mount(&server)
            .await;

        let audio = generate_tone(16000, 1000, 0.3);
        let config = make_test_config(&server.uri());
        let cancel = AtomicBool::new(false);

        // When
        let outcome = process_audio(
            &audio,
            &make_test_format(),
            &config,
            "test-key",
            &cancel,
            || {},
        )
        .await;

        // Then
        match outcome {
            ProcessingOutcome::Text(text) => {
                assert_eq!(text, "Hello, world!");
            }
            other => panic!("ожидался Text, получено: {other:?}"),
        }
    }

    #[tokio::test]
    async fn pipeline_should_extract_text_when_enhance_returns_mixed_output_with_reasoning() {
        // Given: STT возвращает текст; enhance - смешанный вывод с reasoning-
        // элементами (пустые content) и message-элементами.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "text": "test transcription result" })),
            )
            .mount(&server)
            .await;

        // Responses API: reasoning-элемент (без content) + message-элемент (с текстом)
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "resp_mixed",
                "output": [
                    {
                        "type": "reasoning",
                        "content": []
                    },
                    {
                        "type": "message",
                        "content": [{
                            "type": "output_text",
                            "text": "Test transcription result."
                        }]
                    }
                ]
            })))
            .mount(&server)
            .await;

        let audio = generate_tone(16000, 1000, 0.3);
        let config = make_test_config(&server.uri());
        let cancel = AtomicBool::new(false);

        // When
        let outcome = process_audio(
            &audio,
            &make_test_format(),
            &config,
            "test-key",
            &cancel,
            || {},
        )
        .await;

        // Then: текст извлечен из message-элемента, reasoning-элемент пропущен
        match outcome {
            ProcessingOutcome::Text(text) => {
                assert_eq!(
                    text, "Test transcription result.",
                    "текст должен быть извлечен из message-элемента enhance-ответа"
                );
            }
            other => panic!("ожидался Text, получено: {other:?}"),
        }
    }

    #[tokio::test]
    async fn pipeline_should_abort_before_stt_when_trimmed_audio_is_too_short() {
        // Given: аудио всего 100ms, но min_recording_duration_ms = 500ms.
        // STT endpoint НЕ должен вызываться.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0) // не должен вызываться
            .mount(&server)
            .await;

        let audio = generate_tone(16000, 100, 0.3);
        let mut config = make_test_config(&server.uri());
        config.min_recording_duration_ms = 500;
        let cancel = AtomicBool::new(false);

        // When
        let outcome = process_audio(
            &audio,
            &make_test_format(),
            &config,
            "test-key",
            &cancel,
            || {},
        )
        .await;

        // Then
        assert!(
            matches!(outcome, ProcessingOutcome::TooShort { .. }),
            "ожидался TooShort, получено: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn pipeline_should_cancel_to_idle_when_cancel_requested_before_stt() {
        // Given: флаг отмены установлен до начала обработки.
        // STT endpoint НЕ должен вызываться.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        let audio = generate_tone(16000, 1000, 0.3);
        let config = make_test_config(&server.uri());
        let cancel = AtomicBool::new(true); // установлен заранее

        // When
        let outcome = process_audio(
            &audio,
            &make_test_format(),
            &config,
            "test-key",
            &cancel,
            || {},
        )
        .await;

        // Then
        assert!(
            matches!(outcome, ProcessingOutcome::Cancelled),
            "ожидался Cancelled, получено: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn pipeline_should_use_raw_text_when_enhance_disabled() {
        // Given: enhance_enabled = false, нужен только mock STT.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "text": "raw dictation text" })),
            )
            .mount(&server)
            .await;

        // Enhance endpoint НЕ должен вызываться
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;

        let audio = generate_tone(16000, 1000, 0.3);
        let mut config = make_test_config(&server.uri());
        config.enhance_enabled = false;
        let cancel = AtomicBool::new(false);

        // When
        let outcome = process_audio(
            &audio,
            &make_test_format(),
            &config,
            "test-key",
            &cancel,
            || {},
        )
        .await;

        // Then
        match outcome {
            ProcessingOutcome::Text(text) => {
                assert_eq!(text, "raw dictation text");
            }
            other => panic!("ожидался Text, получено: {other:?}"),
        }
    }

    #[tokio::test]
    async fn pipeline_should_call_on_transcription_done_after_stt() {
        // Given
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "text": "test text" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(make_responses_json("Test text.")),
            )
            .mount(&server)
            .await;

        let audio = generate_tone(16000, 1000, 0.3);
        let config = make_test_config(&server.uri());
        let cancel = AtomicBool::new(false);
        let callback_called = Arc::new(AtomicBool::new(false));
        let callback_flag = Arc::clone(&callback_called);

        // When
        let _outcome = process_audio(
            &audio,
            &make_test_format(),
            &config,
            "test-key",
            &cancel,
            move || {
                callback_flag.store(true, Ordering::SeqCst);
            },
        )
        .await;

        // Then
        assert!(
            callback_called.load(Ordering::SeqCst),
            "callback on_transcription_done должен быть вызван после успешного STT"
        );
    }

    #[tokio::test]
    async fn pipeline_should_return_no_speech_when_stt_returns_empty_text() {
        // Given: STT возвращает пустой/только пробелы текст.
        // Pipeline интерпретирует пустой текст как отсутствие речи.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "text": "   " })),
            )
            .mount(&server)
            .await;

        let audio = generate_tone(16000, 1000, 0.3);
        let config = make_test_config(&server.uri());
        let cancel = AtomicBool::new(false);

        // When
        let outcome = process_audio(
            &audio,
            &make_test_format(),
            &config,
            "test-key",
            &cancel,
            || {},
        )
        .await;

        // Then: пустой текст от STT = NoSpeech
        assert!(
            matches!(outcome, ProcessingOutcome::NoSpeech),
            "ожидался NoSpeech для пустого результата STT, получено: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn pipeline_should_return_error_when_stt_fails_with_401() {
        // Given
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let audio = generate_tone(16000, 1000, 0.3);
        let mut config = make_test_config(&server.uri());
        config.retry_count = 0;
        let cancel = AtomicBool::new(false);

        // When
        let outcome = process_audio(
            &audio,
            &make_test_format(),
            &config,
            "test-key",
            &cancel,
            || {},
        )
        .await;

        // Then
        match outcome {
            ProcessingOutcome::Error(msg) => {
                assert!(
                    msg.contains("Transcription failed"),
                    "ошибка должна содержать 'Transcription failed': {msg}"
                );
            }
            other => panic!("expected Error, got: {other:?}"),
        }
    }
}

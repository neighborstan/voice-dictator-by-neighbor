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

/// Pipeline state managed by Tauri.
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

/// Text for the result window (shown when clipboard is unavailable).
pub struct ResultText(pub Mutex<Option<String>>);

impl ResultText {
    pub fn new() -> Self {
        Self(Mutex::new(None))
    }
}

// --- Public API (called from dispatch_and_update) ---

/// Starts audio capture and safety timeout.
///
/// Called when state transitions Idle -> Recording.
/// On error: transitions to Error -> Idle with notification.
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

    // Safety timeout: auto-stop after max_recording_duration_sec
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

/// Stops audio capture and spawns the processing pipeline.
///
/// Called when state transitions Recording -> Transcribing.
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

/// Sets cancel flag to abort the running pipeline.
///
/// Called when processing is cancelled (hotkey or tray during processing states).
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

/// Full dictation pipeline: preprocess -> STT -> enhance -> paste.
///
/// Runs asynchronously after recording stops. Each step checks
/// the cancel flag before proceeding. On error, notifies and recovers.
async fn run_pipeline<R: Runtime>(
    app: AppHandle<R>,
    audio: Vec<f32>,
    format: CaptureFormat,
    config: AppConfig,
    api_key: String,
    cancel: Arc<AtomicBool>,
) {
    let pipeline_start = Instant::now();
    let is_cancelled = || cancel.load(Ordering::SeqCst);

    tracing::info!(
        samples = audio.len(),
        sample_rate = format.sample_rate,
        channels = format.channels,
        "pipeline started"
    );

    // Step 1: Preprocess (mono 16kHz)
    let step = Instant::now();
    let processed = preprocess::preprocess(&audio, format.channels, format.sample_rate);
    tracing::info!(
        ms = step.elapsed().as_millis() as u64,
        samples = processed.len(),
        "preprocess complete"
    );

    // Step 2: Trim silence (if enabled)
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

    // Step 3: Check minimum duration
    let duration_ms = (trimmed.len() as u64 * 1000 / TARGET_SAMPLE_RATE as u64) as u32;
    if duration_ms < config.min_recording_duration_ms {
        tracing::info!(
            duration_ms,
            min_ms = config.min_recording_duration_ms,
            "recording too short"
        );
        notifications::notify_error(&app, "Recording too short, try again");
        abort_pipeline(&app);
        return;
    }

    if is_cancelled() {
        tracing::info!("pipeline cancelled before STT");
        return;
    }

    // Step 4: STT (state is already Transcribing)
    let step = Instant::now();
    let language = match config.language.as_str() {
        "auto" => None,
        lang => Some(lang),
    };

    let stt_client = match OpenAiSttClient::from_config(&config, &api_key) {
        Ok(c) => c,
        Err(e) => {
            handle_pipeline_error(&app, &format!("STT client error: {e}"));
            return;
        }
    };

    let raw_text = match stt::transcribe_audio(
        &stt_client,
        &trimmed,
        TARGET_SAMPLE_RATE,
        language,
        None,
    )
    .await
    {
        Ok(text) => text,
        Err(e) => {
            handle_pipeline_error(&app, &format!("Transcription failed: {e}"));
            return;
        }
    };

    tracing::info!(
        ms = step.elapsed().as_millis() as u64,
        chars = raw_text.len(),
        "STT complete"
    );

    if raw_text.trim().is_empty() {
        tracing::info!("STT returned empty text");
        notifications::notify_error(&app, "No speech detected");
        abort_pipeline(&app);
        return;
    }

    if is_cancelled() {
        tracing::info!("pipeline cancelled after STT");
        return;
    }

    // Transition: Transcribing -> Enhancing
    dispatch_pipeline_event(&app, AppEvent::TranscriptionDone);

    // Step 5: Enhance (if enabled)
    let text = if config.enhance_enabled {
        let step = Instant::now();
        match enhance_text(&config, &api_key, &raw_text, language).await {
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
        return;
    }

    // Transition: Enhancing -> Pasting
    dispatch_pipeline_event(&app, AppEvent::EnhancementDone);

    // Step 6: Paste (on dedicated thread for clean Win32 state)
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

    // Transition: Pasting -> Idle
    dispatch_pipeline_event(&app, AppEvent::PasteDone);

    tracing::info!(
        total_ms = pipeline_start.elapsed().as_millis() as u64,
        "pipeline completed"
    );
}

// --- Helpers ---

/// Enhances text via OpenAI Responses API.
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

/// Dispatches pipeline event: state transition + tray + notification.
fn dispatch_pipeline_event<R: Runtime>(app: &AppHandle<R>, event: AppEvent) {
    let shared = app.state::<SharedAppState>();
    let (old, new) = shared.dispatch_with_old(&event);
    if old != new {
        tray::update_tray(app, new);
        notifications::notify_state_change(app, old, new);
    }
}

/// Handles pipeline error: specific notification, Error state, auto-recover to Idle.
fn handle_pipeline_error<R: Runtime>(app: &AppHandle<R>, message: &str) {
    tracing::error!("pipeline error: {message}");
    notifications::notify_error(app, message);

    let shared = app.state::<SharedAppState>();

    let (_, error_state) = shared.dispatch_with_old(&AppEvent::Failed(message.to_string()));
    tray::update_tray(app, error_state);

    // Auto-recover to Idle
    let (_, idle_state) = shared.dispatch_with_old(&AppEvent::ErrorAcknowledged);
    tray::update_tray(app, idle_state);
}

/// Aborts pipeline gracefully -> Idle (too short, no speech, etc.).
fn abort_pipeline<R: Runtime>(app: &AppHandle<R>) {
    let shared = app.state::<SharedAppState>();
    let (old, new) = shared.dispatch_with_old(&AppEvent::Cancel);
    if old != new {
        tray::update_tray(app, new);
    }
}

/// Stores text and opens/updates the result window.
fn show_result_window<R: Runtime>(app: &AppHandle<R>, text: &str) {
    let result = app.state::<ResultText>();
    *result.0.lock().expect("result mutex poisoned") = Some(text.to_string());

    if let Some(window) = app.get_webview_window("result") {
        let _ = app.emit("result-text-updated", ());
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
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

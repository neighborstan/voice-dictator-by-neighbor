use std::sync::Mutex;

use crate::config::schema::RecordingMode;

/// Состояния конечного автомата приложения.
///
/// Определяет жизненный цикл диктовки: от ожидания до вставки текста.
/// Переходы управляются функцией `transition`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum AppState {
    Idle,
    Recording,
    Transcribing,
    Enhancing,
    Pasting,
    Error,
}

/// События, вызывающие переходы между состояниями.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum AppEvent {
    /// Toggle: начать/остановить запись
    HotkeyPressed,
    /// PTT: клавиша нажата
    HotkeyDown,
    /// PTT: клавиша отпущена
    HotkeyUp,
    /// VAD: тишина превысила порог (только toggle)
    SilenceTimeout,
    /// Достигнут лимит длительности записи
    MaxDurationTimeout,
    /// STT вернул результат
    TranscriptionDone,
    /// Улучшение текста завершено
    EnhancementDone,
    /// Вставка текста завершена
    PasteDone,
    /// Отмена из tray-меню
    Cancel,
    /// Ошибка в любом модуле
    Failed(String),
    /// Пользователь подтвердил ошибку
    ErrorAcknowledged,
}

/// Чистая функция перехода состояний.
///
/// Возвращает новое состояние. При невалидном переходе логирует
/// warning и возвращает текущее состояние без изменений.
#[allow(dead_code)]
pub fn transition(state: AppState, event: &AppEvent, mode: &RecordingMode) -> AppState {
    if let AppEvent::Failed(reason) = event {
        tracing::warn!(state = ?state, reason = %reason, "transition to Error");
        return AppState::Error;
    }

    let new_state = match (state, event) {
        // Toggle: начать запись
        (AppState::Idle, AppEvent::HotkeyPressed) if *mode == RecordingMode::Toggle => {
            AppState::Recording
        }
        // Toggle: остановить запись
        (AppState::Recording, AppEvent::HotkeyPressed) if *mode == RecordingMode::Toggle => {
            AppState::Transcribing
        }

        // PTT: нажатие - начать запись
        (AppState::Idle, AppEvent::HotkeyDown) if *mode == RecordingMode::PushToTalk => {
            AppState::Recording
        }
        // PTT: отпускание - остановить запись
        (AppState::Recording, AppEvent::HotkeyUp) if *mode == RecordingMode::PushToTalk => {
            AppState::Transcribing
        }

        // VAD авто-стоп по тишине (только toggle)
        (AppState::Recording, AppEvent::SilenceTimeout) if *mode == RecordingMode::Toggle => {
            AppState::Transcribing
        }
        // Safety timeout (оба режима)
        (AppState::Recording, AppEvent::MaxDurationTimeout) => AppState::Transcribing,

        // Pipeline: последовательная обработка
        (AppState::Transcribing, AppEvent::TranscriptionDone) => AppState::Enhancing,
        (AppState::Enhancing, AppEvent::EnhancementDone) => AppState::Pasting,
        (AppState::Pasting, AppEvent::PasteDone) => AppState::Idle,

        // Cancel из processing-состояний (hotkey или tray-меню)
        (
            AppState::Transcribing | AppState::Enhancing | AppState::Pasting,
            AppEvent::HotkeyPressed,
        ) => AppState::Idle,
        (AppState::Transcribing | AppState::Enhancing | AppState::Pasting, AppEvent::Cancel) => {
            AppState::Idle
        }

        // Error recovery
        (AppState::Error, AppEvent::ErrorAcknowledged) => AppState::Idle,

        // Невалидный переход - остаемся в текущем состоянии
        _ => {
            tracing::debug!(
                state = ?state,
                event = ?event,
                mode = ?mode,
                "ignored state transition, staying in current state"
            );
            return state;
        }
    };

    new_state
}

/// Потокобезопасное состояние приложения для Tauri.
///
/// Оборачивает текущее состояние и режим записи в Mutex
/// для безопасного доступа из разных потоков.
#[allow(dead_code)]
pub struct SharedAppState {
    state: Mutex<AppState>,
    recording_mode: Mutex<RecordingMode>,
}

#[allow(dead_code)]
impl SharedAppState {
    /// Создает SharedAppState с начальным состоянием Idle.
    pub fn new(mode: RecordingMode) -> Self {
        Self {
            state: Mutex::new(AppState::Idle),
            recording_mode: Mutex::new(mode),
        }
    }

    /// Возвращает текущее состояние.
    pub fn current_state(&self) -> AppState {
        *self.state.lock().expect("state mutex poisoned")
    }

    /// Применяет событие и возвращает новое состояние.
    ///
    /// Атомарно читает текущее состояние, вычисляет переход
    /// и записывает результат.
    pub fn dispatch(&self, event: &AppEvent) -> AppState {
        let mut state = self.state.lock().expect("state mutex poisoned");
        let mode = self.recording_mode.lock().expect("mode mutex poisoned");
        let old = *state;
        let new = transition(old, event, &mode);
        if old != new {
            tracing::info!(from = ?old, to = ?new, event = ?event, "state transition");
        }
        *state = new;
        new
    }

    /// Применяет событие и возвращает (old, new) атомарно.
    ///
    /// В отличие от `dispatch`, гарантирует что `old` прочитано
    /// в том же lock-е, что и запись `new` - без гонки.
    pub fn dispatch_with_old(&self, event: &AppEvent) -> (AppState, AppState) {
        let mut state = self.state.lock().expect("state mutex poisoned");
        let mode = self.recording_mode.lock().expect("mode mutex poisoned");
        let old = *state;
        let new = transition(old, event, &mode);
        if old != new {
            tracing::info!(from = ?old, to = ?new, event = ?event, "state transition");
        }
        *state = new;
        (old, new)
    }

    /// Возвращает текущий режим записи.
    pub fn recording_mode(&self) -> RecordingMode {
        self.recording_mode
            .lock()
            .expect("mode mutex poisoned")
            .clone()
    }

    /// Устанавливает режим записи.
    pub fn set_recording_mode(&self, mode: RecordingMode) {
        *self.recording_mode.lock().expect("mode mutex poisoned") = mode;
    }
}

impl Default for SharedAppState {
    fn default() -> Self {
        Self::new(RecordingMode::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Toggle mode ---

    #[test]
    fn idle_should_start_recording_when_hotkey_pressed_in_toggle() {
        // Given
        let state = AppState::Idle;
        let mode = RecordingMode::Toggle;

        // When
        let new = transition(state, &AppEvent::HotkeyPressed, &mode);

        // Then
        assert_eq!(new, AppState::Recording);
    }

    #[test]
    fn recording_should_start_transcribing_when_hotkey_pressed_in_toggle() {
        // Given
        let state = AppState::Recording;
        let mode = RecordingMode::Toggle;

        // When
        let new = transition(state, &AppEvent::HotkeyPressed, &mode);

        // Then
        assert_eq!(new, AppState::Transcribing);
    }

    #[test]
    fn recording_should_start_transcribing_when_silence_timeout_in_toggle() {
        // Given
        let state = AppState::Recording;
        let mode = RecordingMode::Toggle;

        // When
        let new = transition(state, &AppEvent::SilenceTimeout, &mode);

        // Then
        assert_eq!(new, AppState::Transcribing);
    }

    #[test]
    fn recording_should_start_transcribing_when_max_duration_timeout() {
        // Given
        let state = AppState::Recording;
        let mode = RecordingMode::Toggle;

        // When
        let new = transition(state, &AppEvent::MaxDurationTimeout, &mode);

        // Then
        assert_eq!(new, AppState::Transcribing);
    }

    // --- PTT mode ---

    #[test]
    fn idle_should_start_recording_when_hotkey_down_in_ptt() {
        // Given
        let state = AppState::Idle;
        let mode = RecordingMode::PushToTalk;

        // When
        let new = transition(state, &AppEvent::HotkeyDown, &mode);

        // Then
        assert_eq!(new, AppState::Recording);
    }

    #[test]
    fn recording_should_start_transcribing_when_hotkey_up_in_ptt() {
        // Given
        let state = AppState::Recording;
        let mode = RecordingMode::PushToTalk;

        // When
        let new = transition(state, &AppEvent::HotkeyUp, &mode);

        // Then
        assert_eq!(new, AppState::Transcribing);
    }

    #[test]
    fn recording_should_start_transcribing_when_max_duration_timeout_in_ptt() {
        // Given
        let state = AppState::Recording;
        let mode = RecordingMode::PushToTalk;

        // When
        let new = transition(state, &AppEvent::MaxDurationTimeout, &mode);

        // Then
        assert_eq!(new, AppState::Transcribing);
    }

    // --- Pipeline progression ---

    #[test]
    fn transcribing_should_move_to_enhancing_when_done() {
        // Given
        let state = AppState::Transcribing;
        let mode = RecordingMode::Toggle;

        // When
        let new = transition(state, &AppEvent::TranscriptionDone, &mode);

        // Then
        assert_eq!(new, AppState::Enhancing);
    }

    #[test]
    fn enhancing_should_move_to_pasting_when_done() {
        // Given
        let state = AppState::Enhancing;
        let mode = RecordingMode::Toggle;

        // When
        let new = transition(state, &AppEvent::EnhancementDone, &mode);

        // Then
        assert_eq!(new, AppState::Pasting);
    }

    #[test]
    fn pasting_should_move_to_idle_when_done() {
        // Given
        let state = AppState::Pasting;
        let mode = RecordingMode::Toggle;

        // When
        let new = transition(state, &AppEvent::PasteDone, &mode);

        // Then
        assert_eq!(new, AppState::Idle);
    }

    // --- Cancel ---

    #[test]
    fn transcribing_should_cancel_on_hotkey_press() {
        // Given
        let state = AppState::Transcribing;
        let mode = RecordingMode::Toggle;

        // When
        let new = transition(state, &AppEvent::HotkeyPressed, &mode);

        // Then
        assert_eq!(new, AppState::Idle);
    }

    #[test]
    fn enhancing_should_cancel_on_hotkey_press() {
        // Given
        let state = AppState::Enhancing;
        let mode = RecordingMode::Toggle;

        // When
        let new = transition(state, &AppEvent::HotkeyPressed, &mode);

        // Then
        assert_eq!(new, AppState::Idle);
    }

    #[test]
    fn pasting_should_cancel_on_hotkey_press() {
        // Given
        let state = AppState::Pasting;
        let mode = RecordingMode::Toggle;

        // When
        let new = transition(state, &AppEvent::HotkeyPressed, &mode);

        // Then
        assert_eq!(new, AppState::Idle);
    }

    #[test]
    fn transcribing_should_cancel_on_cancel_event() {
        // Given
        let state = AppState::Transcribing;
        let mode = RecordingMode::Toggle;

        // When
        let new = transition(state, &AppEvent::Cancel, &mode);

        // Then
        assert_eq!(new, AppState::Idle);
    }

    #[test]
    fn enhancing_should_cancel_on_cancel_event() {
        // Given
        let state = AppState::Enhancing;
        let mode = RecordingMode::Toggle;

        // When
        let new = transition(state, &AppEvent::Cancel, &mode);

        // Then
        assert_eq!(new, AppState::Idle);
    }

    #[test]
    fn pasting_should_cancel_on_cancel_event() {
        // Given
        let state = AppState::Pasting;
        let mode = RecordingMode::Toggle;

        // When
        let new = transition(state, &AppEvent::Cancel, &mode);

        // Then
        assert_eq!(new, AppState::Idle);
    }

    // --- Error ---

    #[test]
    fn any_state_should_move_to_error_on_failure() {
        // Given
        let states = [
            AppState::Idle,
            AppState::Recording,
            AppState::Transcribing,
            AppState::Enhancing,
            AppState::Pasting,
        ];
        let mode = RecordingMode::Toggle;
        let event = AppEvent::Failed("test error".to_string());

        for state in states {
            // When
            let new = transition(state, &event, &mode);

            // Then
            assert_eq!(
                new,
                AppState::Error,
                "state {:?} should move to Error",
                state
            );
        }
    }

    #[test]
    fn error_should_move_to_idle_on_acknowledge() {
        // Given
        let state = AppState::Error;
        let mode = RecordingMode::Toggle;

        // When
        let new = transition(state, &AppEvent::ErrorAcknowledged, &mode);

        // Then
        assert_eq!(new, AppState::Idle);
    }

    // --- Invalid transitions ---

    #[test]
    fn idle_should_ignore_hotkey_up_in_toggle() {
        // Given
        let state = AppState::Idle;
        let mode = RecordingMode::Toggle;

        // When
        let new = transition(state, &AppEvent::HotkeyUp, &mode);

        // Then
        assert_eq!(new, AppState::Idle);
    }

    #[test]
    fn recording_should_ignore_hotkey_down_in_toggle() {
        // Given
        let state = AppState::Recording;
        let mode = RecordingMode::Toggle;

        // When
        let new = transition(state, &AppEvent::HotkeyDown, &mode);

        // Then
        assert_eq!(new, AppState::Recording);
    }

    #[test]
    fn idle_should_ignore_hotkey_pressed_in_ptt() {
        // Given
        let state = AppState::Idle;
        let mode = RecordingMode::PushToTalk;

        // When
        let new = transition(state, &AppEvent::HotkeyPressed, &mode);

        // Then
        assert_eq!(new, AppState::Idle);
    }

    #[test]
    fn recording_should_ignore_silence_timeout_in_ptt() {
        // Given
        let state = AppState::Recording;
        let mode = RecordingMode::PushToTalk;

        // When
        let new = transition(state, &AppEvent::SilenceTimeout, &mode);

        // Then
        assert_eq!(new, AppState::Recording);
    }

    #[test]
    fn idle_should_ignore_transcription_done() {
        // Given
        let state = AppState::Idle;
        let mode = RecordingMode::Toggle;

        // When
        let new = transition(state, &AppEvent::TranscriptionDone, &mode);

        // Then
        assert_eq!(new, AppState::Idle);
    }

    #[test]
    fn error_should_ignore_hotkey_pressed() {
        // Given
        let state = AppState::Error;
        let mode = RecordingMode::Toggle;

        // When
        let new = transition(state, &AppEvent::HotkeyPressed, &mode);

        // Then
        assert_eq!(new, AppState::Error);
    }

    // --- SharedAppState ---

    #[test]
    fn shared_state_should_start_idle() {
        // Given / When
        let shared = SharedAppState::default();

        // Then
        assert_eq!(shared.current_state(), AppState::Idle);
    }

    #[test]
    fn shared_state_dispatch_should_transition_correctly() {
        // Given
        let shared = SharedAppState::default();

        // When
        let new = shared.dispatch(&AppEvent::HotkeyPressed);

        // Then
        assert_eq!(new, AppState::Recording);
        assert_eq!(shared.current_state(), AppState::Recording);
    }

    #[test]
    fn shared_state_should_handle_full_pipeline() {
        // Given
        let shared = SharedAppState::default();

        // When / Then
        assert_eq!(
            shared.dispatch(&AppEvent::HotkeyPressed),
            AppState::Recording
        );
        assert_eq!(
            shared.dispatch(&AppEvent::HotkeyPressed),
            AppState::Transcribing
        );
        assert_eq!(
            shared.dispatch(&AppEvent::TranscriptionDone),
            AppState::Enhancing
        );
        assert_eq!(
            shared.dispatch(&AppEvent::EnhancementDone),
            AppState::Pasting
        );
        assert_eq!(shared.dispatch(&AppEvent::PasteDone), AppState::Idle);
    }

    #[test]
    fn shared_state_should_switch_recording_mode() {
        // Given
        let shared = SharedAppState::default();

        // When
        shared.set_recording_mode(RecordingMode::PushToTalk);
        let result = shared.dispatch(&AppEvent::HotkeyDown);

        // Then
        assert_eq!(result, AppState::Recording);
    }

    #[test]
    fn shared_state_should_handle_cancel_during_processing() {
        // Given
        let shared = SharedAppState::default();
        shared.dispatch(&AppEvent::HotkeyPressed); // -> Recording
        shared.dispatch(&AppEvent::HotkeyPressed); // -> Transcribing

        // When
        let new = shared.dispatch(&AppEvent::Cancel);

        // Then
        assert_eq!(new, AppState::Idle);
    }

    #[test]
    fn shared_state_should_handle_error_and_recovery() {
        // Given
        let shared = SharedAppState::default();
        shared.dispatch(&AppEvent::HotkeyPressed); // -> Recording

        // When
        shared.dispatch(&AppEvent::Failed("mic disconnected".to_string()));
        assert_eq!(shared.current_state(), AppState::Error);

        let new = shared.dispatch(&AppEvent::ErrorAcknowledged);

        // Then
        assert_eq!(new, AppState::Idle);
    }
}

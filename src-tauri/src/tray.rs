use std::sync::LazyLock;

use tauri::image::Image;
use tauri::menu::{MenuBuilder, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};

use crate::config::schema::RecordingMode;
use crate::state::{AppEvent, AppState, SharedAppState};

const TRAY_ID: &str = "main";
const ICON_SIZE: u32 = 32;

// Кэшированные RGBA-данные иконок (генерируются один раз при первом доступе)
static ICON_IDLE: LazyLock<Vec<u8>> = LazyLock::new(|| generate_circle_rgba(128, 128, 128));
static ICON_RECORDING: LazyLock<Vec<u8>> = LazyLock::new(|| generate_circle_rgba(220, 50, 50));
static ICON_PROCESSING: LazyLock<Vec<u8>> = LazyLock::new(|| generate_circle_rgba(50, 120, 220));
static ICON_ERROR: LazyLock<Vec<u8>> = LazyLock::new(|| generate_circle_rgba(200, 30, 30));

/// Создает tray-иконку с начальным меню для состояния Idle.
pub fn create_tray<R: Runtime>(
    app: &impl Manager<R>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let menu = build_menu(app, AppState::Idle)?;
    let icon = icon_for_state(AppState::Idle);

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip(tooltip_for_state(AppState::Idle))
        .menu(&menu)
        .on_menu_event(|app, event| {
            handle_menu_event(app, event.id().as_ref());
        })
        .build(app)?;

    tracing::info!("tray icon created");
    Ok(())
}

/// Обновляет tray (меню, иконку, tooltip) по текущему состоянию.
pub fn update_tray<R: Runtime>(app: &AppHandle<R>, state: AppState) {
    if let Err(e) = try_update_tray(app, state) {
        tracing::error!(error = %e, "failed to update tray");
    }
}

fn try_update_tray<R: Runtime>(
    app: &AppHandle<R>,
    state: AppState,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let tray = app.tray_by_id(TRAY_ID).ok_or("tray icon not found")?;

    let menu = build_menu(app, state)?;
    tray.set_menu(Some(menu))?;
    tray.set_icon(Some(icon_for_state(state)))?;
    tray.set_tooltip(Some(tooltip_for_state(state)))?;

    Ok(())
}

/// Формирует контекстное меню трея в зависимости от состояния.
fn build_menu<R: Runtime>(
    app: &impl Manager<R>,
    state: AppState,
) -> std::result::Result<tauri::menu::Menu<R>, Box<dyn std::error::Error>> {
    let mut builder = MenuBuilder::new(app);

    // Action items (контекстные по состоянию)
    let has_action = !matches!(state, AppState::Error);
    match state {
        AppState::Idle => {
            let start = MenuItem::with_id(
                app,
                "start_recording",
                "Start Recording",
                true,
                None::<&str>,
            )?;
            builder = builder.item(&start);
        }
        AppState::Recording => {
            let stop =
                MenuItem::with_id(app, "stop_recording", "Stop Recording", true, None::<&str>)?;
            builder = builder.item(&stop);
        }
        AppState::Transcribing | AppState::Enhancing | AppState::Pasting => {
            let cancel = MenuItem::with_id(app, "cancel", "Cancel Processing", true, None::<&str>)?;
            builder = builder.item(&cancel);
        }
        AppState::Error => {}
    }

    // Settings только для Idle и Error (как в плане задачи 8.2)
    let show_settings = matches!(state, AppState::Idle | AppState::Error);

    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    if show_settings {
        if has_action {
            builder = builder.separator();
        }
        let settings = MenuItem::with_id(app, "settings", "Settings...", true, None::<&str>)?;
        builder = builder.item(&settings).separator().item(&quit);
    } else {
        builder = builder.separator().item(&quit);
    }

    Ok(builder.build()?)
}

/// Обработчик кликов по пунктам tray-меню.
fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, menu_id: &str) {
    match menu_id {
        "start_recording" => {
            let shared = app.state::<SharedAppState>();
            let event = match shared.recording_mode() {
                RecordingMode::Toggle => AppEvent::HotkeyPressed,
                RecordingMode::PushToTalk => AppEvent::HotkeyDown,
            };
            crate::dispatch_and_update(app, event);
        }
        "stop_recording" => {
            let shared = app.state::<SharedAppState>();
            let event = match shared.recording_mode() {
                RecordingMode::Toggle => AppEvent::HotkeyPressed,
                RecordingMode::PushToTalk => AppEvent::HotkeyUp,
            };
            crate::dispatch_and_update(app, event);
        }
        "cancel" => crate::dispatch_and_update(app, AppEvent::Cancel),
        "settings" => {
            tracing::info!("settings requested (not implemented yet)");
        }
        "quit" => {
            tracing::info!("quit requested from tray");
            app.exit(0);
        }
        other => {
            tracing::warn!(id = %other, "unknown tray menu event");
        }
    }
}

/// Возвращает иконку для указанного состояния.
fn icon_for_state(state: AppState) -> Image<'static> {
    let data: &[u8] = match state {
        AppState::Idle => &ICON_IDLE,
        AppState::Recording => &ICON_RECORDING,
        AppState::Transcribing | AppState::Enhancing | AppState::Pasting => &ICON_PROCESSING,
        AppState::Error => &ICON_ERROR,
    };
    Image::new(data, ICON_SIZE, ICON_SIZE)
}

/// Возвращает текст tooltip для указанного состояния.
fn tooltip_for_state(state: AppState) -> &'static str {
    match state {
        AppState::Idle => "VoiceDictator - Idle",
        AppState::Recording => "VoiceDictator - Recording",
        AppState::Transcribing => "VoiceDictator - Transcribing",
        AppState::Enhancing => "VoiceDictator - Enhancing",
        AppState::Pasting => "VoiceDictator - Pasting",
        AppState::Error => "VoiceDictator - Error",
    }
}

/// Генерирует RGBA-данные круглой иконки заданного цвета (32x32, anti-aliased).
fn generate_circle_rgba(r: u8, g: u8, b: u8) -> Vec<u8> {
    let size = ICON_SIZE;
    let center = size as f64 / 2.0;
    let radius = center - 2.0;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);

    for y in 0..size {
        for x in 0..size {
            let dx = x as f64 - center + 0.5;
            let dy = y as f64 - center + 0.5;
            let dist = (dx * dx + dy * dy).sqrt();

            if dist <= radius - 0.5 {
                rgba.extend_from_slice(&[r, g, b, 255]);
            } else if dist <= radius + 0.5 {
                let alpha = ((radius + 0.5 - dist) * 255.0) as u8;
                rgba.extend_from_slice(&[r, g, b, alpha]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }

    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_should_include_app_name_and_state() {
        assert_eq!(tooltip_for_state(AppState::Idle), "VoiceDictator - Idle");
        assert_eq!(
            tooltip_for_state(AppState::Recording),
            "VoiceDictator - Recording"
        );
        assert_eq!(
            tooltip_for_state(AppState::Transcribing),
            "VoiceDictator - Transcribing"
        );
        assert_eq!(
            tooltip_for_state(AppState::Enhancing),
            "VoiceDictator - Enhancing"
        );
        assert_eq!(
            tooltip_for_state(AppState::Pasting),
            "VoiceDictator - Pasting"
        );
        assert_eq!(tooltip_for_state(AppState::Error), "VoiceDictator - Error");
    }

    #[test]
    fn generate_circle_rgba_should_produce_correct_size() {
        // Given / When
        let rgba = generate_circle_rgba(128, 128, 128);

        // Then
        let expected = (ICON_SIZE * ICON_SIZE * 4) as usize;
        assert_eq!(rgba.len(), expected);
    }

    #[test]
    fn generate_circle_rgba_should_have_transparent_corners() {
        // Given / When
        let rgba = generate_circle_rgba(255, 0, 0);

        // Then - top-left pixel (0,0) should be transparent
        assert_eq!(rgba[3], 0, "corner pixel alpha should be 0");
    }

    #[test]
    fn generate_circle_rgba_should_have_opaque_center() {
        // Given / When
        let rgba = generate_circle_rgba(255, 0, 0);

        // Then - center pixel (16,16) should be opaque red
        let center_offset = (16 * ICON_SIZE as usize + 16) * 4;
        assert_eq!(rgba[center_offset], 255, "center R");
        assert_eq!(rgba[center_offset + 1], 0, "center G");
        assert_eq!(rgba[center_offset + 2], 0, "center B");
        assert_eq!(rgba[center_offset + 3], 255, "center A");
    }

    #[test]
    fn icons_for_different_states_should_differ() {
        // Given
        let idle = generate_circle_rgba(128, 128, 128);
        let recording = generate_circle_rgba(220, 50, 50);

        // When / Then
        assert_ne!(idle, recording);
    }

    #[test]
    fn icon_for_state_should_return_correct_dimensions() {
        // Given / When
        let icon = icon_for_state(AppState::Idle);

        // Then
        assert_eq!(icon.width(), ICON_SIZE);
        assert_eq!(icon.height(), ICON_SIZE);
    }

    #[test]
    fn icon_for_state_should_return_distinct_icons_for_key_states() {
        // Given / When
        let idle = icon_for_state(AppState::Idle);
        let recording = icon_for_state(AppState::Recording);
        let processing = icon_for_state(AppState::Transcribing);
        let error = icon_for_state(AppState::Error);

        // Then
        assert_ne!(idle.rgba(), recording.rgba());
        assert_ne!(recording.rgba(), processing.rgba());
        assert_ne!(idle.rgba(), error.rgba());
    }

    #[test]
    fn processing_states_should_share_same_icon() {
        // Given / When
        let transcribing = icon_for_state(AppState::Transcribing);
        let enhancing = icon_for_state(AppState::Enhancing);
        let pasting = icon_for_state(AppState::Pasting);

        // Then
        assert_eq!(transcribing.rgba(), enhancing.rgba());
        assert_eq!(enhancing.rgba(), pasting.rgba());
    }

    #[test]
    fn all_states_should_have_non_empty_tooltip() {
        let states = [
            AppState::Idle,
            AppState::Recording,
            AppState::Transcribing,
            AppState::Enhancing,
            AppState::Pasting,
            AppState::Error,
        ];

        for state in states {
            let tooltip = tooltip_for_state(state);
            assert!(
                tooltip.starts_with("VoiceDictator"),
                "tooltip for {:?} should start with app name",
                state
            );
            assert!(
                tooltip.len() > "VoiceDictator - ".len(),
                "tooltip for {:?} should include state name",
                state
            );
        }
    }
}

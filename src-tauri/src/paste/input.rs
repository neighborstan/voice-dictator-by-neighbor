use std::thread;
use std::time::Duration;

use enigo::{Direction, Enigo, Key, Keyboard, Settings};

/// Задержка между нажатием модификатора и клавиши (мс).
const KEY_DELAY_MS: u64 = 50;

/// Симулирует Ctrl+V (Windows/Linux) или Cmd+V (macOS).
///
/// Использует enigo для программного нажатия клавиш.
/// На macOS вместо Control используется Meta (Command).
pub fn simulate_paste() -> super::Result<()> {
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| super::PasteError::InputSimulation(e.to_string()))?;

    let modifier = paste_modifier_key();
    tracing::debug!("Simulating paste with modifier {:?}", modifier);

    enigo
        .key(modifier, Direction::Press)
        .map_err(|e| super::PasteError::InputSimulation(e.to_string()))?;

    thread::sleep(Duration::from_millis(KEY_DELAY_MS));

    enigo
        .key(Key::Unicode('v'), Direction::Click)
        .map_err(|e| super::PasteError::InputSimulation(e.to_string()))?;

    thread::sleep(Duration::from_millis(KEY_DELAY_MS));

    enigo
        .key(modifier, Direction::Release)
        .map_err(|e| super::PasteError::InputSimulation(e.to_string()))?;

    tracing::debug!("Paste key simulation completed");
    Ok(())
}

/// Возвращает клавишу-модификатор для вставки в зависимости от ОС.
fn paste_modifier_key() -> Key {
    if cfg!(target_os = "macos") {
        Key::Meta
    } else {
        Key::Control
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_modifier_key_should_return_control_on_windows() {
        // Given / When
        let key = paste_modifier_key();

        // Then
        if cfg!(target_os = "macos") {
            assert_eq!(key, Key::Meta);
        } else {
            assert_eq!(key, Key::Control);
        }
    }
}

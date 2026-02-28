use std::thread;
use std::time::Duration;

use enigo::{Direction, Enigo, Key, Keyboard, Settings};

/// Задержка между нажатием модификатора и клавиши (мс).
const KEY_DELAY_MS: u64 = 50;

/// Guard, гарантирующий отпускание клавиши-модификатора при Drop.
///
/// Если ошибка произойдет после Press, но до Release, guard отпустит
/// модификатор автоматически, предотвращая "залипание" Ctrl/Cmd.
struct ModifierGuard<'a> {
    enigo: &'a mut Enigo,
    key: Key,
    pressed: bool,
}

impl<'a> ModifierGuard<'a> {
    fn new(enigo: &'a mut Enigo, key: Key) -> Self {
        Self {
            enigo,
            key,
            pressed: false,
        }
    }

    fn press(&mut self) -> super::Result<()> {
        self.enigo
            .key(self.key, Direction::Press)
            .map_err(|e| super::PasteError::InputSimulation(e.to_string()))?;
        self.pressed = true;
        Ok(())
    }

    fn release(&mut self) -> super::Result<()> {
        self.enigo
            .key(self.key, Direction::Release)
            .map_err(|e| super::PasteError::InputSimulation(e.to_string()))?;
        self.pressed = false;
        Ok(())
    }
}

impl Drop for ModifierGuard<'_> {
    fn drop(&mut self) {
        if self.pressed {
            if let Err(e) = self.enigo.key(self.key, Direction::Release) {
                tracing::error!("Failed to release modifier key in guard: {e}");
            }
        }
    }
}

/// Симулирует Ctrl+V (Windows/Linux) или Cmd+V (macOS).
///
/// Использует enigo для программного нажатия клавиш.
/// На macOS вместо Control используется Meta (Command).
/// Модификатор гарантированно отпускается даже при ошибках (через guard).
pub fn simulate_paste() -> super::Result<()> {
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| super::PasteError::InputSimulation(e.to_string()))?;

    let modifier = paste_modifier_key();
    tracing::debug!("Simulating paste with modifier {:?}", modifier);

    let mut guard = ModifierGuard::new(&mut enigo, modifier);

    guard.press()?;
    thread::sleep(Duration::from_millis(KEY_DELAY_MS));

    guard
        .enigo
        .key(paste_v_key(), Direction::Click)
        .map_err(|e| super::PasteError::InputSimulation(e.to_string()))?;

    thread::sleep(Duration::from_millis(KEY_DELAY_MS));
    guard.release()?;

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

/// Возвращает клавишу V для вставки.
///
/// На Windows используем `Key::Unicode('v')` как наиболее совместимый вариант
/// (enigo на Windows корректно маппит Unicode 'v' через виртуальный key code).
fn paste_v_key() -> Key {
    Key::Unicode('v')
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

    #[test]
    fn paste_v_key_should_return_unicode_v() {
        // Given / When
        let key = paste_v_key();

        // Then
        assert_eq!(key, Key::Unicode('v'));
    }
}

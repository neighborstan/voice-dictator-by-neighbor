#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;

use tracing::{info, warn};

use crate::config::schema::AppConfig;
use crate::error::{AppError, Result};

/// Имя файла конфигурации.
const CONFIG_FILE_NAME: &str = "config.json";

/// Имя бэкапа поврежденного конфига.
const CONFIG_BACKUP_NAME: &str = "config.json.bak";

/// Идентификатор приложения (совпадает с tauri.conf.json -> identifier).
const APP_IDENTIFIER: &str = "com.voicedictator.app";

/// Возвращает путь к каталогу конфигурации приложения.
///
/// Windows: `%APPDATA%/com.voicedictator.app/`
/// macOS: `~/Library/Application Support/com.voicedictator.app/`
/// Linux: `~/.config/com.voicedictator.app/`
pub fn config_dir() -> Result<PathBuf> {
    let base = dirs::config_dir()
        .ok_or_else(|| AppError::Config("failed to determine OS config directory".to_string()))?;
    Ok(base.join(APP_IDENTIFIER))
}

/// Возвращает полный путь к файлу конфигурации.
fn config_file_path() -> Result<PathBuf> {
    Ok(config_dir()?.join(CONFIG_FILE_NAME))
}

/// Загружает конфиг из JSON-файла.
///
/// - Если файл не существует - возвращает дефолтный конфиг и сохраняет его.
/// - Если файл поврежден - логирует ошибку, создает бэкап, возвращает дефолтный.
pub fn load_config() -> Result<AppConfig> {
    let path = config_file_path()?;

    if !path.exists() {
        info!("Config file not found, creating default at {:?}", path);
        let config = AppConfig::default();
        save_config(&config)?;
        return Ok(config);
    }

    let content = fs::read_to_string(&path)
        .map_err(|e| AppError::Config(format!("failed to read config file {:?}: {}", path, e)))?;

    match serde_json::from_str::<AppConfig>(&content) {
        Ok(config) => {
            info!("Config loaded from {:?}", path);
            Ok(config)
        }
        Err(e) => {
            warn!(
                "Config file corrupted: {}. Backing up and using defaults.",
                e
            );
            let backup_path = config_dir()?.join(CONFIG_BACKUP_NAME);
            if let Err(backup_err) = fs::copy(&path, &backup_path) {
                warn!("Failed to create config backup: {}", backup_err);
            }
            let config = AppConfig::default();
            save_config(&config)?;
            Ok(config)
        }
    }
}

/// Сохраняет конфиг в JSON-файл.
///
/// Создает каталог если не существует. Использует атомарную запись
/// (запись во временный файл + переименование).
pub fn save_config(config: &AppConfig) -> Result<()> {
    let path = config_file_path()?;
    let dir = config_dir()?;

    fs::create_dir_all(&dir).map_err(|e| {
        AppError::Config(format!(
            "failed to create config directory {:?}: {}",
            dir, e
        ))
    })?;

    let json = serde_json::to_string_pretty(config)
        .map_err(|e| AppError::Config(format!("failed to serialize config: {}", e)))?;

    // Атомарная запись: write to temp + rename
    let tmp_path = dir.join("config.json.tmp");
    fs::write(&tmp_path, &json).map_err(|e| {
        AppError::Config(format!(
            "failed to write temp config file {:?}: {}",
            tmp_path, e
        ))
    })?;

    fs::rename(&tmp_path, &path).map_err(|e| {
        AppError::Config(format!("failed to rename temp config to {:?}: {}", path, e))
    })?;

    info!("Config saved to {:?}", path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // Вспомогательные функции для тестов с изолированным каталогом

    fn save_config_to(dir: &std::path::Path, config: &AppConfig) -> Result<()> {
        fs::create_dir_all(dir)
            .map_err(|e| AppError::Config(format!("failed to create dir: {}", e)))?;
        let path = dir.join(CONFIG_FILE_NAME);
        let json = serde_json::to_string_pretty(config)
            .map_err(|e| AppError::Config(format!("failed to serialize: {}", e)))?;
        let tmp_path = dir.join("config.json.tmp");
        fs::write(&tmp_path, &json)
            .map_err(|e| AppError::Config(format!("failed to write: {}", e)))?;
        fs::rename(&tmp_path, &path)
            .map_err(|e| AppError::Config(format!("failed to rename: {}", e)))?;
        Ok(())
    }

    fn load_config_from(dir: &std::path::Path) -> Result<AppConfig> {
        let path = dir.join(CONFIG_FILE_NAME);
        if !path.exists() {
            let config = AppConfig::default();
            save_config_to(dir, &config)?;
            return Ok(config);
        }
        let content = fs::read_to_string(&path)
            .map_err(|e| AppError::Config(format!("read error: {}", e)))?;
        match serde_json::from_str::<AppConfig>(&content) {
            Ok(config) => Ok(config),
            Err(_) => {
                let backup = dir.join(CONFIG_BACKUP_NAME);
                if let Err(e) = fs::copy(&path, &backup) {
                    warn!("Failed to create config backup at {:?}: {}", backup, e);
                }
                let config = AppConfig::default();
                save_config_to(dir, &config)?;
                Ok(config)
            }
        }
    }

    #[test]
    fn load_should_create_default_when_file_missing() {
        // Given
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("voicedictator");

        // When
        let config = load_config_from(&dir).unwrap();

        // Then
        assert_eq!(config.config_version, 1);
        assert_eq!(config.hotkey, "Ctrl+Shift+S");
        assert!(dir.join(CONFIG_FILE_NAME).exists());
    }

    #[test]
    fn save_and_load_should_roundtrip() {
        // Given
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("voicedictator");
        let config = AppConfig {
            hotkey: "Alt+R".to_string(),
            language: "ru".to_string(),
            max_recording_duration_sec: 120,
            ..Default::default()
        };

        // When
        save_config_to(&dir, &config).unwrap();
        let loaded = load_config_from(&dir).unwrap();

        // Then
        assert_eq!(loaded.hotkey, "Alt+R");
        assert_eq!(loaded.language, "ru");
        assert_eq!(loaded.max_recording_duration_sec, 120);
    }

    #[test]
    fn load_should_fallback_to_default_when_corrupted() {
        // Given
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("voicedictator");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(CONFIG_FILE_NAME), "{ invalid json !!!").unwrap();

        // When
        let config = load_config_from(&dir).unwrap();

        // Then - должен вернуть дефолтный конфиг
        assert_eq!(config.config_version, 1);
        assert_eq!(config.hotkey, "Ctrl+Shift+S");
        // Бэкап должен быть создан
        assert!(dir.join(CONFIG_BACKUP_NAME).exists());
    }

    #[test]
    fn save_should_create_directory_if_not_exists() {
        // Given
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("deep").join("nested").join("config");
        let config = AppConfig::default();

        // When
        save_config_to(&dir, &config).unwrap();

        // Then
        assert!(dir.join(CONFIG_FILE_NAME).exists());
    }

    #[test]
    fn save_should_produce_pretty_json() {
        // Given
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("voicedictator");
        let config = AppConfig::default();

        // When
        save_config_to(&dir, &config).unwrap();
        let content = fs::read_to_string(dir.join(CONFIG_FILE_NAME)).unwrap();

        // Then - pretty JSON содержит переносы строк и пробелы
        assert!(content.contains('\n'));
        assert!(content.contains("  "));
        assert!(content.contains("\"config_version\""));
    }
}

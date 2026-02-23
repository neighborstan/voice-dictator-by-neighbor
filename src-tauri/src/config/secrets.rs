#![allow(dead_code)]

use tracing::{info, warn};

use crate::error::{AppError, Result};

/// Имя сервиса в OS keychain.
const SERVICE_NAME: &str = "voicedictator";

/// Имя пользователя (ключ) в OS keychain.
const USERNAME: &str = "openai-api-key";

/// Сохраняет API-ключ в OS keychain.
pub fn store_api_key(key: &str) -> Result<()> {
    let entry = keyring::Entry::new(SERVICE_NAME, USERNAME)
        .map_err(|e| AppError::Config(format!("failed to create keyring entry: {}", e)))?;
    entry
        .set_password(key)
        .map_err(|e| AppError::Config(format!("failed to store API key in keychain: {}", e)))?;
    info!("API key stored in OS keychain");
    Ok(())
}

/// Загружает API-ключ из OS keychain. Возвращает `None` если ключ не сохранен.
pub fn load_api_key() -> Result<Option<String>> {
    let entry = keyring::Entry::new(SERVICE_NAME, USERNAME)
        .map_err(|e| AppError::Config(format!("failed to create keyring entry: {}", e)))?;
    match entry.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => {
            warn!("Failed to load API key from keychain: {}", e);
            Err(AppError::Config(format!(
                "failed to load API key from keychain: {}",
                e
            )))
        }
    }
}

/// Удаляет API-ключ из OS keychain.
pub fn delete_api_key() -> Result<()> {
    let entry = keyring::Entry::new(SERVICE_NAME, USERNAME)
        .map_err(|e| AppError::Config(format!("failed to create keyring entry: {}", e)))?;
    match entry.delete_credential() {
        Ok(()) => {
            info!("API key deleted from OS keychain");
            Ok(())
        }
        Err(keyring::Error::NoEntry) => {
            info!("No API key to delete from OS keychain");
            Ok(())
        }
        Err(e) => Err(AppError::Config(format!(
            "failed to delete API key from keychain: {}",
            e
        ))),
    }
}

/// Проверяет наличие API-ключа в OS keychain.
pub fn has_api_key() -> bool {
    keyring::Entry::new(SERVICE_NAME, USERNAME)
        .map(|entry| entry.get_password().is_ok())
        .unwrap_or(false)
}

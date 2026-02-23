use tracing_appender::rolling;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

/// Инициализирует систему логирования.
///
/// Настраивает tracing-subscriber с выводом в файл (ротация по дням)
/// и в stdout (только в debug-сборке).
/// Уровень по умолчанию: info, переопределяется через RUST_LOG.
pub fn init_logging() {
    let app_name = "voicedictator";

    let log_dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(app_name)
        .join("logs");

    let file_appender = rolling::daily(&log_dir, "voicedictator.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // _guard must be kept alive for the lifetime of the application.
    // We leak it intentionally to avoid dropping the writer.
    std::mem::forget(_guard);

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let file_layer = fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(false);

    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer);

    #[cfg(debug_assertions)]
    {
        let stdout_layer = fmt::layer().with_target(true).with_thread_ids(false);
        registry.with(stdout_layer).init();
    }

    #[cfg(not(debug_assertions))]
    {
        registry.init();
    }
}

mod audio;
mod config;
mod enhance;
mod error;
mod logging;
mod paste;
mod state;
#[allow(dead_code, unused_imports)]
mod stt;
#[allow(dead_code, unused_imports)]
mod vad;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    logging::init_logging();

    tracing::info!("VoiceDictator starting");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(state::SharedAppState::default())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

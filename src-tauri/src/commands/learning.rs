use crate::learning::{self, LearningSample, LearningSettings};
use crate::settings::{get_settings, write_settings};
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
#[specta::specta]
pub fn configure_learning(
    app: AppHandle,
    enabled: bool,
    save_audio: bool,
    directory: String,
) -> Result<(), String> {
    let directory = directory.trim().to_owned();
    if !std::path::Path::new(&directory).is_absolute() {
        return Err("Choose an absolute learning folder path".into());
    }
    if enabled {
        learning::prepare(&directory)?;
    }
    let mut settings = get_settings(&app);
    settings.learning = LearningSettings {
        enabled,
        save_audio,
        directory: Some(directory),
    };
    write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn get_learning_samples(app: AppHandle) -> Result<Vec<LearningSample>, String> {
    match get_settings(&app).learning.directory {
        Some(path) => learning::list(&path),
        None => Ok(Vec::new()),
    }
}

#[tauri::command]
#[specta::specta]
pub fn save_learning_correction(
    app: AppHandle,
    id: String,
    correction: String,
) -> Result<(), String> {
    let path = get_settings(&app)
        .learning
        .directory
        .ok_or("Choose a learning folder first")?;
    learning::save_correction(&path, &id, &correction)
}

#[tauri::command]
#[specta::specta]
pub fn open_learning_folder(app: AppHandle) -> Result<(), String> {
    let path = get_settings(&app)
        .learning
        .directory
        .ok_or("Choose a learning folder first")?;
    let root = learning::prepare(&path)?;
    app.opener()
        .open_path(root.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|e| e.to_string())
}

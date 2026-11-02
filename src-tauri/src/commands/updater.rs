use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub release_url: String,
    pub release_notes: String,
}

#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    if cfg!(debug_assertions) {
        return Ok(None);
    }

    let current = app.package_info().version.to_string();
    if current == "0.0.0" {
        return Ok(None);
    }

    let update = app
        .updater_builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;

    Ok(update.map(|update| {
        let latest_version = update.version.to_string();
        UpdateInfo {
            current_version: current,
            release_url: format!(
                "https://github.com/church-helpers-hungary/metocast/releases/tag/v{}",
                latest_version
            ),
            latest_version,
            release_notes: update.body.unwrap_or_default(),
        }
    }))
}

#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    if cfg!(debug_assertions) {
        return Err("Updates can only be installed from a packaged application.".to_string());
    }

    let update = app
        .updater_builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No update is available.".to_string())?;

    let mut downloaded = 0;
    update
        .download_and_install(
            |chunk_length, content_length| {
                downloaded += chunk_length;
                tracing::info!(downloaded, content_length, "Downloading application update");
            },
            || {
                tracing::info!("Application update download finished");
            },
        )
        .await
        .map_err(|e| e.to_string())?;

    tracing::info!("Application update installed; restarting");
    app.restart();
}

pub mod facebook;
pub mod youtube;

use metocast_core::best_effort::LogFailed;
use std::collections::HashMap;
use std::sync::Arc;

use sqlx::PgPool;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::connectors::{
    ConnectorStatus, FacebookConfig, facebook::FacebookConnector, obs::ObsConnector,
    youtube::YouTubeConnector,
};
use crate::server::websocket::broadcast_upload_paused;

/// Pending upload row joined with recording metadata.
#[derive(sqlx::FromRow)]
struct PendingUpload {
    recording_id: Uuid,
    platform: String,
    state: String,
    progress_bytes: i64,
    upload_uri: Option<String>,
    upload_session_id: Option<String>,
    visibility: String,
    file_path: String,
    file_size: i64,
    /// The recording's own title, else its event's.
    title: Option<String>,
    /// The recording's own description, else its event's.
    description: Option<String>,
}

struct UploadSource<'a> {
    file_path: &'a str,
    file_size: i64,
    title: &'a str,
    description: &'a str,
    visibility: &'a str,
}

struct UploadResume {
    locator: Option<String>,
    progress_bytes: i64,
}

pub struct UploadService {
    pool: PgPool,
    youtube_connector: Arc<YouTubeConnector>,
    facebook_connector: Arc<FacebookConnector>,
    obs_connector: Arc<ObsConnector>,
    facebook_config: Arc<RwLock<FacebookConfig>>,
    ws_clients: Arc<RwLock<HashMap<Uuid, crate::server::ws_client::ClientTx>>>,
}

impl UploadService {
    pub fn new(
        pool: PgPool,
        youtube_connector: Arc<YouTubeConnector>,
        facebook_connector: Arc<FacebookConnector>,
        obs_connector: Arc<ObsConnector>,
        facebook_config: Arc<RwLock<FacebookConfig>>,
        ws_clients: Arc<RwLock<HashMap<Uuid, crate::server::ws_client::ClientTx>>>,
    ) -> Self {
        Self {
            pool,
            youtube_connector,
            facebook_connector,
            obs_connector,
            facebook_config,
            ws_clients,
        }
    }

    /// Returns true if OBS is currently streaming.
    async fn is_streaming(&self) -> bool {
        if let Some(state) = self.obs_connector.get_output_state().await {
            return state.is_streaming;
        }
        false
    }

    /// Process all pending/paused/uploading upload rows.
    /// Pauses any uploading rows if OBS is currently streaming.
    pub async fn run_cycle(&self) {
        tracing::info!("UploadService: starting cycle");

        let rows = sqlx::query_as::<_, PendingUpload>(
            r#"SELECT
                ru.recording_id,
                ru.platform,
                ru.state,
                ru.progress_bytes,
                ru.upload_uri,
                ru.upload_session_id,
                ru.visibility,
                r.file_path,
                r.file_size,
                COALESCE(r.custom_title, NULLIF(e.computed_title, ''), e.title) AS title,
                COALESCE(r.custom_description, e.description) AS description
               FROM recording_uploads ru
               JOIN recordings r ON r.id = ru.recording_id
               LEFT JOIN events e ON e.id = r.event_id
               WHERE ru.state IN ('pending', 'paused', 'uploading')
               ORDER BY r.detected_at ASC"#,
        )
        .fetch_all(&self.pool)
        .await;

        let rows = match rows {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("UploadService: failed to load pending uploads: {e}");
                return;
            }
        };

        if rows.is_empty() {
            tracing::info!("UploadService: no pending uploads");
            return;
        }

        // Pause if OBS is streaming
        if self.is_streaming().await {
            tracing::info!("UploadService: OBS is streaming — pausing uploads");
            for row in &rows {
                if row.state == "uploading" {
                    sqlx::query(
                        "UPDATE recording_uploads SET state = 'paused', updated_at = NOW() \
                         WHERE recording_id = $1 AND platform = $2",
                    )
                    .bind(row.recording_id)
                    .bind(&row.platform)
                    .execute(&self.pool)
                    .await
                    .log_failed();

                    broadcast_upload_paused(&self.ws_clients, row.recording_id, "OBS is streaming")
                        .await;
                }
            }
            return;
        }

        // Process each pending/paused upload
        for row in rows {
            if let Err(e) = self.process_upload(&row).await {
                tracing::error!(
                    "UploadService: upload failed for {} on {}: {e}",
                    row.recording_id,
                    row.platform
                );
            }
        }

        tracing::info!("UploadService: cycle complete");
    }

    async fn process_upload(&self, row: &PendingUpload) -> anyhow::Result<()> {
        let title = row
            .title
            .as_deref()
            .unwrap_or("Untitled Recording")
            .to_string();
        let description = row.description.as_deref().unwrap_or("").to_string();

        match row.platform.as_str() {
            "youtube" => {
                let yt_status = self.youtube_connector.get_status().await;
                if !matches!(yt_status, ConnectorStatus::Connected) {
                    tracing::warn!(
                        "UploadService: YouTube not connected — skipping {}",
                        row.recording_id
                    );
                    return Ok(());
                }

                let token = match crate::connectors::youtube::load_tokens(&self.pool).await {
                    Some(t) => t,
                    None => {
                        tracing::warn!(
                            "UploadService: no YouTube token — skipping {}",
                            row.recording_id
                        );
                        return Ok(());
                    }
                };

                youtube::run_upload(
                    &self.pool,
                    &self.ws_clients,
                    row.recording_id,
                    UploadSource {
                        file_path: &row.file_path,
                        file_size: row.file_size,
                        title: &title,
                        description: &description,
                        visibility: &row.visibility,
                    },
                    UploadResume {
                        locator: row.upload_uri.clone(),
                        progress_bytes: row.progress_bytes,
                    },
                    &token.access_token,
                )
                .await?;
            }
            "facebook" => {
                let fb_status = self.facebook_connector.get_status().await;
                if !matches!(fb_status, ConnectorStatus::Connected) {
                    tracing::warn!(
                        "UploadService: Facebook not connected — skipping {}",
                        row.recording_id
                    );
                    return Ok(());
                }

                let token = match crate::connectors::facebook::load_tokens(&self.pool).await {
                    Some(t) => t,
                    None => {
                        tracing::warn!(
                            "UploadService: no Facebook token — skipping {}",
                            row.recording_id
                        );
                        return Ok(());
                    }
                };

                let page_id = {
                    let cfg = self.facebook_config.read().await;
                    cfg.page_id.clone()
                };

                if page_id.is_empty() {
                    tracing::warn!(
                        "UploadService: Facebook page_id not configured — skipping {}",
                        row.recording_id
                    );
                    return Ok(());
                }

                facebook::run_upload(
                    &self.pool,
                    &self.ws_clients,
                    row.recording_id,
                    UploadSource {
                        file_path: &row.file_path,
                        file_size: row.file_size,
                        title: &title,
                        description: &description,
                        visibility: &row.visibility,
                    },
                    UploadResume {
                        locator: row.upload_session_id.clone(),
                        progress_bytes: row.progress_bytes,
                    },
                    &token.access_token,
                    &page_id,
                )
                .await?;
            }
            other => {
                tracing::warn!("UploadService: unknown platform '{other}' — skipping");
            }
        }

        Ok(())
    }
}

use metocast_core::best_effort::LogGone;
use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::ws::Message;
use serde_json::json;
use sqlx::PgPool;
use tokio::sync::RwLock;
use tokio_cron_scheduler::{Job, JobScheduler};
use uuid::Uuid;

use crate::connectors::youtube;
use crate::models::cron_job;
use crate::models::event::{Event, fetch_event};
use crate::uploader::UploadService;

pub struct CronScheduler {
    scheduler: Arc<RwLock<Option<JobScheduler>>>,
}

impl CronScheduler {
    pub fn new() -> Self {
        Self {
            scheduler: Arc::new(RwLock::new(None)),
        }
    }

    /// Re-read all enabled jobs from the DB and rebuild the scheduler.
    /// Call this on startup and after any CRUD mutation.
    pub async fn reload(
        &self,
        pool: PgPool,
        ws_clients: Arc<RwLock<HashMap<Uuid, crate::server::ws_client::ClientTx>>>,
        youtube_connector: Arc<crate::connectors::youtube::YouTubeConnector>,
        upload_service: Arc<UploadService>,
    ) {
        // Shut down the existing scheduler if any.
        {
            let mut guard = self.scheduler.write().await;
            if let Some(mut sched) = guard.take()
                && let Err(e) = sched.shutdown().await
            {
                tracing::warn!("CronScheduler shutdown error: {e}");
            }
        }

        let jobs = match cron_job::list_all(&pool).await {
            Ok(j) => j,
            Err(e) => {
                tracing::error!("CronScheduler: failed to load jobs: {e}");
                return;
            }
        };

        let enabled: Vec<_> = jobs.into_iter().filter(|j| j.enabled).collect();
        if enabled.is_empty() {
            return;
        }

        let sched = match JobScheduler::new().await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("CronScheduler: failed to create scheduler: {e}");
                return;
            }
        };

        for job in enabled {
            let pool_c = pool.clone();
            let clients_c = Arc::clone(&ws_clients);
            let yt_c = Arc::clone(&youtube_connector);
            let us_c = Arc::clone(&upload_service);
            let expr = job.cron_expression.clone();
            let job_name = job.name.clone();

            let task = match Job::new_async(expr.as_str(), move |_id, _sched| {
                let pool_i = pool_c.clone();
                let clients_i = Arc::clone(&clients_c);
                let yt_i = Arc::clone(&yt_c);
                let us_i = Arc::clone(&us_c);
                let job_i = job.clone();
                Box::pin(async move {
                    tracing::info!("Cron job '{}' fired", job_i.name);
                    run_job(job_i, pool_i, clients_i, yt_i, us_i).await;
                })
            }) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(
                        "CronScheduler: invalid expression '{}' for job '{}': {e}",
                        expr,
                        job_name
                    );
                    continue;
                }
            };

            if let Err(e) = sched.add(task).await {
                tracing::warn!("CronScheduler: failed to add job '{}': {e}", job_name);
            }
        }

        if let Err(e) = sched.start().await {
            tracing::error!("CronScheduler: failed to start: {e}");
            return;
        }

        *self.scheduler.write().await = Some(sched);
        tracing::info!("CronScheduler reloaded");
    }
}

impl Default for CronScheduler {
    fn default() -> Self {
        Self::new()
    }
}

async fn run_job(
    job: cron_job::CronJob,
    pool: PgPool,
    ws_clients: Arc<RwLock<HashMap<Uuid, crate::server::ws_client::ClientTx>>>,
    youtube_connector: Arc<crate::connectors::youtube::YouTubeConnector>,
    upload_service: Arc<UploadService>,
) {
    if job.pull_youtube {
        pull_youtube_live(pool, ws_clients, youtube_connector).await;
    }
    if job.auto_upload {
        upload_service.run_cycle().await;
    }
}

/// Fetch live/upcoming broadcasts from YouTube, then upsert them into the DB:
/// - Existing event (matched by youtube external_id in event_connections): update schedule_status.
/// - No matching event: create one with youtube + facebook connection rows (skipped for
///   already-completed broadcasts).
///
/// Uses `skip_sync_notify` so the PostgreSQL trigger does not re-fire the
/// YouTube auto-scheduler. Each upsert emits an `event.changed` WS message
/// directly so connected clients get real-time updates.
async fn pull_youtube_live(
    pool: PgPool,
    ws_clients: Arc<RwLock<HashMap<Uuid, crate::server::ws_client::ClientTx>>>,
    _youtube_connector: Arc<crate::connectors::youtube::YouTubeConnector>,
) {
    let token = match youtube::load_tokens(&pool).await {
        Some(t) => t,
        None => {
            tracing::warn!("pull_youtube_live: no YouTube token stored");
            return;
        }
    };

    let client = reqwest::Client::new();
    let resp = client
        .get("https://www.googleapis.com/youtube/v3/liveBroadcasts")
        .query(&[
            ("part", "id,status,snippet"),
            ("broadcastStatus", "all"),
            ("maxResults", "50"),
        ])
        .bearer_auth(&token.access_token)
        .send()
        .await;

    let body = match resp {
        Ok(r) => match r.json::<serde_json::Value>().await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("pull_youtube_live: JSON parse error: {e}");
                return;
            }
        },
        Err(e) => {
            tracing::error!("pull_youtube_live: HTTP error: {e}");
            return;
        }
    };

    let items = match body.get("items").and_then(|i| i.as_array()) {
        Some(arr) if !arr.is_empty() => arr.clone(),
        _ => {
            tracing::info!("pull_youtube_live: no broadcasts returned");
            broadcast_cron_status(false, &ws_clients).await;
            return;
        }
    };

    let mut has_live = false;

    for item in &items {
        let broadcast_id = match item.get("id").and_then(|v| v.as_str()) {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => continue,
        };

        let Some(status) = item
            .pointer("/status/lifeCycleStatus")
            .and_then(|v| v.as_str())
            .and_then(|raw| crate::connectors::broadcast_status("youtube", raw))
        else {
            continue;
        };

        if status == "live" {
            has_live = true;
        }

        let title = item
            .pointer("/snippet/title")
            .and_then(|v| v.as_str())
            .unwrap_or("Untitled Broadcast")
            .to_string();

        let privacy_status = item
            .pointer("/status/privacyStatus")
            .and_then(|v| v.as_str())
            .unwrap_or("private")
            .to_string();

        let date_time: chrono::DateTime<chrono::Utc> = item
            .pointer("/snippet/scheduledStartTime")
            .or_else(|| item.pointer("/snippet/actualStartTime"))
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&chrono::Utc))
            .unwrap_or_else(chrono::Utc::now);

        let watch_url = format!("https://www.youtube.com/watch?v={broadcast_id}");

        // Check whether we already have an event for this broadcast.
        #[derive(sqlx::FromRow)]
        struct EventIdRow {
            id: Uuid,
        }

        let existing = sqlx::query_as::<_, EventIdRow>(
            "SELECT e.id FROM events e \
             JOIN event_connections c ON c.event_id = e.id \
             WHERE c.platform = 'youtube' AND c.external_id = $1",
        )
        .bind(&broadcast_id)
        .fetch_optional(&pool)
        .await;

        match existing {
            Ok(Some(row)) => {
                let event_id = row.id;

                // Only update the DB and notify clients if something actually changed.
                #[derive(sqlx::FromRow)]
                struct ConnRow {
                    schedule_status: String,
                    stream_url: Option<String>,
                    external_id: Option<String>,
                }
                let current = sqlx::query_as::<_, ConnRow>(
                    "SELECT schedule_status, stream_url, external_id \
                     FROM event_connections \
                     WHERE event_id = $1 AND platform = 'youtube'",
                )
                .bind(event_id)
                .fetch_optional(&pool)
                .await;

                let needs_update = match &current {
                    Ok(Some(c)) => {
                        c.schedule_status != status
                            || c.stream_url.as_deref() != Some(watch_url.as_str())
                            || c.external_id.as_deref() != Some(broadcast_id.as_str())
                    }
                    _ => true,
                };

                if !needs_update {
                    continue;
                }

                // Update the schedule_status and stream_url to reflect what YouTube reports.
                let result = async {
                    let mut tx = pool.begin().await?;
                    sqlx::query("SET LOCAL app.skip_sync_notify = 'true'")
                        .execute(&mut *tx)
                        .await?;
                    sqlx::query(
                        r#"INSERT INTO event_connections (event_id, platform, external_id, stream_url, schedule_status)
                           VALUES ($1, 'youtube', $2, $3, $4)
                           ON CONFLICT (event_id, platform) DO UPDATE SET
                               external_id     = EXCLUDED.external_id,
                               stream_url      = EXCLUDED.stream_url,
                               schedule_status = EXCLUDED.schedule_status,
                               updated_at      = NOW()"#,
                    )
                    .bind(event_id)
                    .bind(&broadcast_id)
                    .bind(&watch_url)
                    .bind(status)
                    .execute(&mut *tx)
                    .await?;
                    sqlx::query("UPDATE events SET updated_at = NOW() WHERE id = $1")
                        .bind(event_id)
                        .execute(&mut *tx)
                        .await?;
                    tx.commit().await?;
                    anyhow::Ok(event_id)
                }
                .await;

                match result {
                    Ok(eid) => match fetch_event(eid, &pool).await {
                        Ok(Some(event)) => {
                            emit_event_changed("UPDATE", event, &ws_clients).await;
                        }
                        Ok(None) => {
                            tracing::warn!("pull_youtube_live: event {eid} not found after update")
                        }
                        Err(e) => tracing::error!(
                            "pull_youtube_live: fetch_event after update for {eid}: {e}"
                        ),
                    },
                    Err(e) => tracing::error!(
                        "pull_youtube_live: update event for broadcast {broadcast_id}: {e}"
                    ),
                }
            }
            Ok(None) => {
                // Don't create events for already-completed broadcasts we've
                // never seen before — they're historical and wouldn't be useful.
                if status == "ended" {
                    continue;
                }

                let result = async {
                    let mut tx = pool.begin().await?;
                    sqlx::query("SET LOCAL app.skip_sync_notify = 'true'")
                        .execute(&mut *tx)
                        .await?;
                    let event_id: Uuid = sqlx::query_scalar(
                        r#"INSERT INTO events (title, date_time, speaker, description, auto_upload_enabled)
                        VALUES ($1, $2, '', '', false)
                        RETURNING id"#,
                    )
                    .bind(&title)
                    .bind(date_time)
                    .fetch_one(&mut *tx)
                    .await?;

                    sqlx::query(
                        r#"INSERT INTO event_connections
                           (event_id, platform, external_id, stream_url, schedule_status, privacy_status)
                           VALUES ($1, 'youtube', $2, $3, $4, $5)"#,
                    )
                    .bind(event_id)
                    .bind(&broadcast_id)
                    .bind(&watch_url)
                    .bind(status)
                    .bind(&privacy_status)
                    .execute(&mut *tx)
                    .await?;

                    sqlx::query(
                        "INSERT INTO event_connections (event_id, platform, privacy_status) \
                         VALUES ($1, 'facebook', 'EVERYONE')",
                    )
                    .bind(event_id)
                    .execute(&mut *tx)
                    .await?;

                    tx.commit().await?;
                    anyhow::Ok(event_id)
                }
                .await;

                match result {
                    Ok(eid) => match fetch_event(eid, &pool).await {
                        Ok(Some(event)) => {
                            tracing::info!(
                                "pull_youtube_live: created event '{}' for broadcast {broadcast_id}",
                                event.title
                            );
                            emit_event_changed("INSERT", event, &ws_clients).await;
                        }
                        Ok(None) => {
                            tracing::warn!("pull_youtube_live: event {eid} not found after insert")
                        }
                        Err(e) => tracing::error!(
                            "pull_youtube_live: fetch_event after insert for {eid}: {e}"
                        ),
                    },
                    Err(e) => tracing::error!(
                        "pull_youtube_live: create event for broadcast {broadcast_id}: {e}"
                    ),
                }
            }
            Err(e) => {
                tracing::error!("pull_youtube_live: DB lookup for broadcast {broadcast_id}: {e}");
            }
        }
    }

    tracing::info!(
        "pull_youtube_live: processed {} broadcasts, hasLive={}",
        items.len(),
        has_live
    );
    broadcast_cron_status(has_live, &ws_clients).await;
}

async fn emit_event_changed(
    operation: &str,
    event: Event,
    ws_clients: &Arc<RwLock<HashMap<Uuid, crate::server::ws_client::ClientTx>>>,
) {
    let msg = json!({
        "type": "event.changed",
        "data": { "operation": operation, "record": event }
    })
    .to_string();
    let guard = ws_clients.read().await;
    for tx in guard.values() {
        tx.send(Message::Text(msg.clone().into())).log_gone();
    }
}

async fn broadcast_cron_status(
    has_live: bool,
    ws_clients: &Arc<RwLock<HashMap<Uuid, crate::server::ws_client::ClientTx>>>,
) {
    let msg = json!({ "type": "cron.youtube_pull", "hasLive": has_live }).to_string();
    let guard = ws_clients.read().await;
    for tx in guard.values() {
        tx.send(Message::Text(msg.clone().into())).log_gone();
    }
}

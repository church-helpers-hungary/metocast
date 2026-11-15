use metocast_core::best_effort::{LogFailed, LogGone};
use std::sync::Arc;

use crate::HostHandle;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::sync::{Mutex, RwLock, broadcast, watch};
use tokio::time::Duration;

use super::{ConnectorConfig, ConnectorStatus, YouTubeConfig};

// ── Token types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct StoredToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

// ── Error types ───────────────────────────────────────────────────────────────

/// Returned when Google rejects the refresh token (HTTP 400) — the user must re-authenticate.
#[derive(Debug, thiserror::Error)]
#[error("YouTube authentication required — please re-login")]
pub struct AuthRequired;

// ── API response types ────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BroadcastResult {
    pub broadcast_id: String,
    pub watch_url: String,
}

#[derive(Debug, Deserialize)]
struct YtBroadcastInsertResponse {
    id: String,
}

// ── Connector ────────────────────────────────────────────────────────────────

pub struct YouTubeConnector {
    pub status: Arc<RwLock<ConnectorStatus>>,
    pub status_tx: broadcast::Sender<ConnectorStatus>,
    stop_tx: Mutex<Option<watch::Sender<bool>>>,
    host: Mutex<Option<HostHandle>>,
}

impl YouTubeConnector {
    pub fn new() -> Self {
        let (status_tx, _) = broadcast::channel(16);
        Self {
            status: Arc::new(RwLock::new(ConnectorStatus::Disconnected)),
            status_tx,
            stop_tx: Mutex::new(None),
            host: Mutex::new(None),
        }
    }

    pub async fn start(&self, pool: PgPool, config: YouTubeConfig, host: Option<HostHandle>) {
        *self.host.lock().await = host.clone();
        self.stop_internal().await;

        let (stop_tx, stop_rx) = watch::channel(false);
        *self.stop_tx.lock().await = Some(stop_tx);

        let status = Arc::clone(&self.status);
        let status_tx = self.status_tx.clone();
        tokio::spawn(async move {
            run_token_loop(pool, config, status, status_tx, stop_rx, host).await;
        });
    }

    pub async fn stop(&self) {
        self.stop_internal().await;
        let guard = self.host.lock().await;
        set_status(
            &self.status,
            &self.status_tx,
            guard.as_ref(),
            ConnectorStatus::Disconnected,
        )
        .await;
    }

    async fn stop_internal(&self) {
        let mut guard = self.stop_tx.lock().await;
        if let Some(tx) = guard.take() {
            tx.send_replace(true);
        }
    }

    pub async fn get_status(&self) -> ConnectorStatus {
        self.status.read().await.clone()
    }
}

impl Default for YouTubeConnector {
    fn default() -> Self {
        Self::new()
    }
}

// ── Status helpers ────────────────────────────────────────────────────────────

async fn set_status(
    status: &Arc<RwLock<ConnectorStatus>>,
    status_tx: &broadcast::Sender<ConnectorStatus>,
    host: Option<&HostHandle>,
    new_status: ConnectorStatus,
) {
    tracing::info!(connector = "youtube", status = ?new_status, "connector status");
    *status.write().await = new_status;
    let current = status.read().await.clone();
    status_tx.send(current.clone()).log_gone();
    if let Some(host) = host
        && let Err(e) = crate::emit(host, "connector://youtube-status", &current)
    {
        tracing::warn!("Failed to emit YouTube status: {e}");
    }
}

// ── Token management ──────────────────────────────────────────────────────────

pub async fn load_tokens(pool: &PgPool) -> Option<StoredToken> {
    #[derive(sqlx::FromRow)]
    struct Row {
        access_token: String,
        refresh_token: Option<String>,
        expires_at: Option<DateTime<Utc>>,
    }

    let row = sqlx::query_as::<_, Row>(
        "SELECT access_token, refresh_token, expires_at FROM connector_tokens WHERE connector = 'youtube'"
    )
    .fetch_optional(pool)
    .await
    .ok()??;

    Some(StoredToken {
        access_token: row.access_token,
        refresh_token: row.refresh_token,
        expires_at: row.expires_at,
    })
}

pub async fn save_tokens(pool: &PgPool, token: &StoredToken) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO connector_tokens (connector, access_token, refresh_token, expires_at, updated_at)
        VALUES ('youtube', $1, $2, $3, NOW())
        ON CONFLICT (connector) DO UPDATE SET
            access_token  = EXCLUDED.access_token,
            refresh_token = EXCLUDED.refresh_token,
            expires_at    = EXCLUDED.expires_at,
            updated_at    = NOW()
        "#,
    )
    .bind(&token.access_token)
    .bind(&token.refresh_token)
    .bind(token.expires_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_tokens(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM connector_tokens WHERE connector = 'youtube'")
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn refresh_tokens(
    pool: &PgPool,
    config: &YouTubeConfig,
    stored: &StoredToken,
) -> anyhow::Result<StoredToken> {
    let refresh_token = stored
        .refresh_token
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("No refresh token available"))?;

    let client = reqwest::Client::new();
    let raw = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", config.client_id.as_str()),
            ("client_secret", config.client_secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await?;

    // A 400 from Google's token endpoint means the refresh token is invalid or
    // revoked — re-authentication is required (not just a transient error).
    if raw.status() == reqwest::StatusCode::BAD_REQUEST {
        return Err(AuthRequired.into());
    }

    let resp = raw.error_for_status()?.json::<TokenResponse>().await?;

    let expires_at = resp
        .expires_in
        .map(|secs| Utc::now() + chrono::Duration::seconds(secs as i64));

    let new_token = StoredToken {
        access_token: resp.access_token,
        refresh_token: resp.refresh_token.or_else(|| stored.refresh_token.clone()),
        expires_at,
    };
    save_tokens(pool, &new_token).await?;
    Ok(new_token)
}

/// Exchange an OAuth code for tokens and persist them.
pub async fn exchange_code(
    pool: &PgPool,
    config: &YouTubeConfig,
    code: &str,
    redirect_uri: &str,
) -> anyhow::Result<StoredToken> {
    let client = reqwest::Client::new();
    let resp = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("code", code),
            ("client_id", config.client_id.as_str()),
            ("client_secret", config.client_secret.as_str()),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await?
        .error_for_status()?
        .json::<TokenResponse>()
        .await?;

    let expires_at = resp
        .expires_in
        .map(|secs| Utc::now() + chrono::Duration::seconds(secs as i64));

    let token = StoredToken {
        access_token: resp.access_token,
        refresh_token: resp.refresh_token,
        expires_at,
    };
    save_tokens(pool, &token).await?;
    Ok(token)
}

// ── Background token-refresh loop ─────────────────────────────────────────────

async fn run_token_loop(
    pool: PgPool,
    config: YouTubeConfig,
    status: Arc<RwLock<ConnectorStatus>>,
    status_tx: broadcast::Sender<ConnectorStatus>,
    mut stop_rx: watch::Receiver<bool>,
    host: Option<HostHandle>,
) {
    // An enabled connector that cannot work is an error, not a quiet absence: the
    // UI only raises a notification on `Error`, and the operator has to be told.
    if !config.is_configured() {
        set_status(
            &status,
            &status_tx,
            host.as_ref(),
            ConnectorStatus::Error {
                message: "credentials_required".to_string(),
            },
        )
        .await;
        return;
    }

    let mut token = match load_tokens(&pool).await {
        Some(t) => t,
        None => {
            set_status(
                &status,
                &status_tx,
                host.as_ref(),
                ConnectorStatus::Error {
                    message: "login_required".to_string(),
                },
            )
            .await;
            return;
        }
    };

    // Eagerly refresh on startup so we detect an invalid token before ever
    // setting the status to Connected. Without this, the page could load while
    // the connector appears connected, then fail with a 500 when the actual
    // refresh happens inside fetch_channel_content.
    let needs_initial_refresh = token
        .expires_at
        .is_some_and(|exp| exp - Utc::now() < chrono::Duration::minutes(10));

    if needs_initial_refresh {
        match refresh_tokens(&pool, &config, &token).await {
            Ok(new_token) => {
                tracing::info!("YouTube token refreshed on startup");
                token = new_token;
            }
            Err(e) => {
                tracing::error!("YouTube startup token refresh failed: {e}");
                if e.is::<AuthRequired>() {
                    delete_tokens(&pool).await.log_failed();
                }
                set_status(
                    &status,
                    &status_tx,
                    host.as_ref(),
                    ConnectorStatus::Error {
                        message: "relogin_required".to_string(),
                    },
                )
                .await;
                return;
            }
        }
    }

    set_status(
        &status,
        &status_tx,
        host.as_ref(),
        ConnectorStatus::Connected,
    )
    .await;

    loop {
        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(300)) => {}
            _ = stop_rx.changed() => {
                set_status(&status, &status_tx, host.as_ref(), ConnectorStatus::Disconnected).await;
                return;
            }
        }

        if *stop_rx.borrow() {
            set_status(
                &status,
                &status_tx,
                host.as_ref(),
                ConnectorStatus::Disconnected,
            )
            .await;
            return;
        }

        let needs_refresh = token
            .expires_at
            .is_some_and(|exp| exp - Utc::now() < chrono::Duration::minutes(10));

        if needs_refresh {
            match refresh_tokens(&pool, &config, &token).await {
                Ok(new_token) => {
                    tracing::info!("YouTube token refreshed successfully");
                    token = new_token;
                }
                Err(e) => {
                    tracing::error!("YouTube token refresh failed: {e}");
                    if e.is::<AuthRequired>() {
                        delete_tokens(&pool).await.log_failed();
                    }
                    set_status(
                        &status,
                        &status_tx,
                        host.as_ref(),
                        ConnectorStatus::Error {
                            message: "relogin_required".to_string(),
                        },
                    )
                    .await;
                    return;
                }
            }
        }
    }
}

// ── Event scheduling ──────────────────────────────────────────────────────────

/// Create or update a YouTube live broadcast for the given event.
/// Returns `(broadcast_id, watch_url)` on success.
pub async fn schedule_event(
    event_title: &str,
    event_time: &DateTime<Utc>,
    access_token: &str,
    existing_broadcast_id: Option<&str>,
    privacy_status: &str,
    closed_captions: bool,
) -> anyhow::Result<BroadcastResult> {
    let client = reqwest::Client::new();

    let scheduled_start = event_time.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    // HTTP-POST captions are fed by the live transcript (`transcription::youtube_captions`).
    let content_details = serde_json::json!({
        "enableAutoStart": false,
        "enableAutoStop": false,
        "recordFromStart": true,
        "enableDvr": true,
        "closedCaptionsType": if closed_captions { "closedCaptionsHttpPost" } else { "closedCaptionsDisabled" },
        "monitorStream": { "enableMonitorStream": true, "broadcastStreamDelayMs": 0 },
    });

    if let Some(bid) = existing_broadcast_id {
        // Update existing broadcast; contentDetails only when captions need switching on, since
        // a contentDetails update replaces all of it.
        let mut body = serde_json::json!({
            "id": bid,
            "snippet": {
                "title": event_title,
                "scheduledStartTime": scheduled_start,
            },
            "status": {
                "privacyStatus": privacy_status
            }
        });
        let parts = if closed_captions {
            body["contentDetails"] = content_details;
            "snippet,status,contentDetails"
        } else {
            "snippet,status"
        };

        let resp = client
            .put("https://www.googleapis.com/youtube/v3/liveBroadcasts")
            .query(&[("part", parts)])
            .bearer_auth(access_token)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let detail = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "YouTube API {} updating broadcast: {}",
                status,
                detail
            ));
        }

        return Ok(BroadcastResult {
            broadcast_id: bid.to_string(),
            watch_url: format!("https://www.youtube.com/watch?v={bid}"),
        });
    }

    // Create new broadcast
    let body = serde_json::json!({
        "snippet": {
            "title": event_title,
            "scheduledStartTime": scheduled_start,
            "description": ""
        },
        "status": {
            "privacyStatus": privacy_status
        },
        "contentDetails": content_details,
    });

    let resp = client
        .post("https://www.googleapis.com/youtube/v3/liveBroadcasts")
        .query(&[("part", "snippet,status,contentDetails")])
        .bearer_auth(access_token)
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "YouTube API {} creating broadcast: {}",
            status,
            detail
        ));
    }

    let resp = resp.json::<YtBroadcastInsertResponse>().await?;

    Ok(BroadcastResult {
        watch_url: format!("https://www.youtube.com/watch?v={}", resp.id),
        broadcast_id: resp.id,
    })
}

/// Delete a live broadcast. A 404 counts as success — it is already gone.
pub async fn delete_broadcast(broadcast_id: &str, access_token: &str) -> anyhow::Result<()> {
    let resp = reqwest::Client::new()
        .delete("https://www.googleapis.com/youtube/v3/liveBroadcasts")
        .query(&[("id", broadcast_id)])
        .bearer_auth(access_token)
        .send()
        .await?;

    if resp.status().is_success() || resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(());
    }
    let status = resp.status();
    let detail = resp.text().await.unwrap_or_default();
    Err(anyhow::anyhow!(
        "YouTube API {} deleting broadcast: {}",
        status,
        detail
    ))
}

// ── Channel content (Live Events & Videos page) ───────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelVideoItem {
    pub id: String,
    pub title: String,
    pub thumbnail_url: String,
    pub published_at: Option<String>,
    pub view_count: Option<u64>,
    pub like_count: Option<u64>,
    pub duration: Option<String>,
    pub live_status: String, // "none" | "upcoming" | "live" | "completed"
    pub scheduled_start_time: Option<String>,
    pub watch_url: String,
    pub privacy_status: String, // "public" | "private" | "unlisted"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelContent {
    pub live_broadcasts: Vec<ChannelVideoItem>,
    pub videos: Vec<ChannelVideoItem>,
}

// Private serde types for YouTube Data API v3

#[derive(Deserialize)]
struct YtList<T> {
    items: Option<Vec<T>>,
}

#[derive(Deserialize)]
struct ChannelItem {
    #[serde(rename = "contentDetails")]
    content_details: ChannelContentDetails,
}

#[derive(Deserialize)]
struct ChannelContentDetails {
    #[serde(rename = "relatedPlaylists")]
    related_playlists: RelatedPlaylists,
}

#[derive(Deserialize)]
struct RelatedPlaylists {
    uploads: String,
}

#[derive(Deserialize)]
struct PlaylistItem {
    #[serde(rename = "contentDetails")]
    content_details: PlaylistItemContentDetails,
}

#[derive(Deserialize)]
struct PlaylistItemContentDetails {
    #[serde(rename = "videoId")]
    video_id: String,
}

#[derive(Deserialize)]
struct VideoItem {
    id: String,
    snippet: Option<VideoSnippet>,
    statistics: Option<VideoStatistics>,
    #[serde(rename = "contentDetails")]
    content_details: Option<VideoContentDetails>,
    status: Option<VideoStatus>,
    /// Present only for videos that were live broadcasts (past, current, or scheduled).
    #[serde(rename = "liveStreamingDetails")]
    live_streaming_details: Option<LiveStreamingDetails>,
}

#[derive(Deserialize)]
struct VideoStatus {
    #[serde(rename = "privacyStatus")]
    privacy_status: String,
}

#[derive(Deserialize)]
struct VideoSnippet {
    title: String,
    thumbnails: Option<Thumbnails>,
    #[serde(rename = "publishedAt")]
    published_at: Option<String>,
    /// "live" | "upcoming" | "none" — only non-"none" for currently live/upcoming videos.
    #[serde(rename = "liveBroadcastContent")]
    live_broadcast_content: Option<String>,
}

#[derive(Deserialize)]
struct VideoStatistics {
    #[serde(rename = "viewCount")]
    view_count: Option<String>,
    #[serde(rename = "likeCount")]
    like_count: Option<String>,
}

#[derive(Deserialize)]
struct VideoContentDetails {
    duration: Option<String>,
}

#[derive(Deserialize)]
struct LiveStreamingDetails {
    #[serde(rename = "scheduledStartTime")]
    scheduled_start_time: Option<String>,
    #[serde(rename = "actualStartTime")]
    actual_start_time: Option<String>,
    #[serde(rename = "actualEndTime")]
    actual_end_time: Option<String>,
}

#[derive(Deserialize)]
struct Thumbnails {
    medium: Option<Thumb>,
    high: Option<Thumb>,
    standard: Option<Thumb>,
    maxres: Option<Thumb>,
}

#[derive(Deserialize)]
struct Thumb {
    url: String,
}

impl Thumbnails {
    fn best_url(&self) -> Option<&str> {
        self.maxres
            .as_ref()
            .or(self.standard.as_ref())
            .or(self.high.as_ref())
            .or(self.medium.as_ref())
            .map(|t| t.url.as_str())
    }
}

fn parse_duration(iso: &str) -> String {
    let s = iso.trim_start_matches("PT");
    let mut hours: u64 = 0;
    let mut minutes: u64 = 0;
    let mut seconds: u64 = 0;
    let mut current = String::new();
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            current.push(ch);
        } else {
            let n: u64 = current.parse().unwrap_or(0);
            current.clear();
            match ch {
                'H' => hours = n,
                'M' => minutes = n,
                'S' => seconds = n,
                _ => {}
            }
        }
    }
    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, minutes, seconds)
    } else {
        format!("{}:{:02}", minutes, seconds)
    }
}

/// Fetch the channel's uploads and split them into live broadcasts vs regular videos.
///
/// Uses `liveStreamingDetails` from `videos.list` to reliably detect live events,
/// including completed ones (where `liveBroadcastContent` would be "none").
pub async fn fetch_channel_content(
    pool: &PgPool,
    config: &YouTubeConfig,
) -> anyhow::Result<ChannelContent> {
    let mut token = load_tokens(pool)
        .await
        .ok_or_else(|| anyhow::anyhow!("No YouTube token stored"))?;

    let needs_refresh = token
        .expires_at
        .is_some_and(|exp| exp - Utc::now() < chrono::Duration::minutes(5));

    if needs_refresh {
        token = match refresh_tokens(pool, config, &token).await {
            Ok(t) => t,
            Err(e) => {
                // Tokens are now invalid — remove them so the connector won't
                // appear connected on the next startup.
                if e.is::<AuthRequired>() {
                    delete_tokens(pool).await.log_failed();
                }
                return Err(e);
            }
        };
    }

    let client = reqwest::Client::new();
    let all_items = fetch_all_uploads(&client, &token.access_token).await?;

    let mut live_broadcasts = Vec::new();
    let mut videos = Vec::new();

    for item in all_items {
        if item.live_streaming_details.is_some() {
            live_broadcasts.push(to_channel_video_item(item));
        } else {
            videos.push(to_channel_video_item(item));
        }
    }

    Ok(ChannelContent {
        live_broadcasts,
        videos,
    })
}

/// Fetch all uploads for the authenticated channel, including `liveStreamingDetails`
/// so we can tell completed live streams apart from regular uploads.
async fn fetch_all_uploads(
    client: &reqwest::Client,
    access_token: &str,
) -> anyhow::Result<Vec<VideoItem>> {
    // 1. Get the uploads playlist ID for this channel.
    let channel_resp = client
        .get("https://www.googleapis.com/youtube/v3/channels")
        .query(&[("part", "contentDetails"), ("mine", "true")])
        .bearer_auth(access_token)
        .send()
        .await?
        .error_for_status()?
        .json::<YtList<ChannelItem>>()
        .await?;

    let uploads_playlist_id = channel_resp
        .items
        .and_then(|items| items.into_iter().next())
        .map(|c| c.content_details.related_playlists.uploads)
        .ok_or_else(|| anyhow::anyhow!("No YouTube channel found for this account"))?;

    // 2. Get video IDs from the uploads playlist (latest 50).
    let playlist_resp = client
        .get("https://www.googleapis.com/youtube/v3/playlistItems")
        .query(&[
            ("part", "contentDetails"),
            ("playlistId", &uploads_playlist_id),
            ("maxResults", "50"),
        ])
        .bearer_auth(access_token)
        .send()
        .await?
        .error_for_status()?
        .json::<YtList<PlaylistItem>>()
        .await?;

    let video_ids: Vec<String> = playlist_resp
        .items
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.content_details.video_id)
        .collect();

    if video_ids.is_empty() {
        return Ok(vec![]);
    }

    // 3. Fetch full details including liveStreamingDetails to detect live events.
    let ids_str = video_ids.join(",");
    let videos_resp = client
        .get("https://www.googleapis.com/youtube/v3/videos")
        .query(&[
            (
                "part",
                "snippet,statistics,contentDetails,status,liveStreamingDetails",
            ),
            ("id", &ids_str),
            ("maxResults", "50"),
        ])
        .bearer_auth(access_token)
        .send()
        .await?
        .error_for_status()?
        .json::<YtList<VideoItem>>()
        .await?;

    Ok(videos_resp.items.unwrap_or_default())
}

fn to_channel_video_item(v: VideoItem) -> ChannelVideoItem {
    let snippet = v.snippet.as_ref();
    let title = snippet.map(|s| s.title.clone()).unwrap_or_default();
    let thumbnail_url = snippet
        .and_then(|s| s.thumbnails.as_ref())
        .and_then(|t| t.best_url())
        .map(String::from)
        .unwrap_or_default();
    let published_at = snippet.and_then(|s| s.published_at.clone());

    // Determine live_status from liveStreamingDetails (reliable for completed streams)
    // then fall back to liveBroadcastContent for currently live/upcoming.
    let live_status = if let Some(ref lsd) = v.live_streaming_details {
        if lsd.actual_end_time.is_some() {
            "completed"
        } else if lsd.actual_start_time.is_some() {
            "live"
        } else {
            "upcoming"
        }
    } else {
        snippet
            .and_then(|s| s.live_broadcast_content.as_deref())
            .filter(|s| *s != "none")
            .unwrap_or("none")
    }
    .to_string();

    let scheduled_start_time = v
        .live_streaming_details
        .as_ref()
        .and_then(|lsd| lsd.scheduled_start_time.clone());

    let view_count = v
        .statistics
        .as_ref()
        .and_then(|s| s.view_count.as_ref())
        .and_then(|c| c.parse::<u64>().ok());
    let like_count = v
        .statistics
        .as_ref()
        .and_then(|s| s.like_count.as_ref())
        .and_then(|c| c.parse::<u64>().ok());
    let duration = v
        .content_details
        .as_ref()
        .and_then(|d| d.duration.as_deref())
        .map(parse_duration);

    let privacy_status = v
        .status
        .map(|s| s.privacy_status)
        .unwrap_or_else(|| "public".to_string());

    ChannelVideoItem {
        watch_url: format!("https://www.youtube.com/watch?v={}", v.id),
        id: v.id,
        title,
        thumbnail_url,
        published_at,
        view_count,
        like_count,
        duration,
        live_status,
        scheduled_start_time,
        privacy_status,
    }
}

/// The RTMP ingestion endpoint of the channel's first live stream: the address and
/// the stream key OBS, a camera or any encoder needs to push to.
#[derive(Debug, Clone)]
pub struct Ingestion {
    pub address: String,
    pub key: String,
}

impl Ingestion {
    /// The single RTMP URL form, for encoders that take address and key together.
    pub fn rtmp_url(&self) -> String {
        format!("{}/{}", self.address.trim_end_matches('/'), self.key)
    }
}

/// Where live captions go: a stream's signed `closedCaptionsIngestionUrl` (never log it) and the
/// stream's id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptionIngestion {
    pub url: String,
    pub stream_id: String,
    /// The stream bound to the event's broadcast, rather than the channel's first stream.
    pub bound: bool,
}

/// The caption ingestion of the stream bound to `broadcast_id`, or of the channel's first stream
/// when there is no broadcast or it is not bound yet (`go_live` binds it). 2 quota units.
pub async fn caption_ingestion(
    access_token: &str,
    broadcast_id: Option<&str>,
) -> anyhow::Result<Option<CaptionIngestion>> {
    let client = reqwest::Client::new();
    let bound = match broadcast_id {
        Some(id) => bound_stream_id(
            &youtube_call(
                client
                    .get(BROADCASTS)
                    .query(&[("part", "contentDetails"), ("id", id)])
                    .bearer_auth(access_token),
            )
            .await?,
        ),
        None => None,
    };
    let filter = match &bound {
        Some(id) => ("id", id.as_str()),
        None => ("mine", "true"),
    };
    let streams = youtube_call(
        client
            .get(STREAMS)
            .query(&[("part", "contentDetails"), filter])
            .bearer_auth(access_token),
    )
    .await?;
    Ok(pick_caption_stream(&streams, bound.as_deref()))
}

/// `contentDetails.boundStreamId` of the first broadcast in a `liveBroadcasts.list` reply.
fn bound_stream_id(broadcasts: &serde_json::Value) -> Option<String> {
    broadcasts["items"][0]["contentDetails"]["boundStreamId"]
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// From a `liveStreams.list` reply: the `bound` stream, or else the first one with a caption URL.
fn pick_caption_stream(
    streams: &serde_json::Value,
    bound: Option<&str>,
) -> Option<CaptionIngestion> {
    streams["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| bound.is_none_or(|id| item["id"].as_str() == Some(id)))
        .find_map(|item| {
            let url = item["contentDetails"]["closedCaptionsIngestionUrl"]
                .as_str()
                .filter(|url| !url.is_empty())?;
            Some(CaptionIngestion {
                url: url.to_string(),
                stream_id: item["id"].as_str().unwrap_or_default().to_string(),
                bound: bound.is_some(),
            })
        })
}

/// Uploads a WebVTT track in `language` (ISO 639-1) on `video_id`, replacing `existing` when it
/// is still there. Returns the caption id.
pub async fn upload_caption(
    access_token: &str,
    video_id: &str,
    existing: Option<&str>,
    vtt: &str,
    language: &str,
) -> anyhow::Result<String> {
    const URL: &str = "https://www.googleapis.com/upload/youtube/v3/captions";
    let client = reqwest::Client::new();
    let send = |metadata: serde_json::Value, update: bool| {
        let boundary = format!("metocast-{}", uuid::Uuid::new_v4().simple());
        let body = format!(
            "--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{metadata}\r\n\
             --{boundary}\r\nContent-Type: text/vtt\r\n\r\n{vtt}\r\n--{boundary}--\r\n"
        );
        let request = if update {
            client.put(URL)
        } else {
            client.post(URL)
        };
        request
            .query(&[("part", "snippet"), ("uploadType", "multipart")])
            .bearer_auth(access_token)
            .header(
                reqwest::header::CONTENT_TYPE,
                format!("multipart/related; boundary={boundary}"),
            )
            .body(body)
            .send()
    };
    let name = match language {
        "en" => "English",
        "hu" => "Magyar",
        other => other,
    };
    let insert = serde_json::json!({
        "snippet": { "videoId": video_id, "language": language, "name": name, "isDraft": false }
    });
    let response = match existing {
        Some(id) => {
            let update = serde_json::json!({ "id": id, "snippet": { "isDraft": false } });
            let response = send(update, true).await?;
            // Deleted in YouTube Studio since: upload it again.
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                send(insert, false).await?
            } else {
                response
            }
        }
        None => send(insert, false).await?,
    };
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or_default();
    if !status.is_success() {
        let reason = body["error"]["errors"][0]["reason"].as_str().unwrap_or("");
        if reason == "insufficientPermissions"
            || body
                .to_string()
                .contains("insufficient authentication scopes")
        {
            anyhow::bail!("Reconnect YouTube to allow caption uploads");
        }
        anyhow::bail!(
            "YouTube API {status} uploading captions: {}",
            body["error"]["message"]
        );
    }
    body["id"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("YouTube returned no caption id"))
}

pub async fn live_stream_ingestion(access_token: &str) -> anyhow::Result<Ingestion> {
    #[derive(Deserialize)]
    struct IngestionInfo {
        #[serde(rename = "ingestionAddress")]
        ingestion_address: String,
        #[serde(rename = "streamName")]
        stream_name: String,
    }
    #[derive(Deserialize)]
    struct Cdn {
        #[serde(rename = "ingestionInfo")]
        ingestion_info: IngestionInfo,
    }
    #[derive(Deserialize)]
    struct StreamItem {
        cdn: Cdn,
    }
    #[derive(Deserialize)]
    struct StreamList {
        items: Option<Vec<StreamItem>>,
    }

    let response = reqwest::Client::new()
        .get("https://www.googleapis.com/youtube/v3/liveStreams")
        .query(&[("part", "cdn"), ("mine", "true")])
        .bearer_auth(access_token)
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("youtube_api_{}", response.status());
    }

    let list = response.json::<StreamList>().await?;
    let item = list
        .items
        .and_then(|items| items.into_iter().next())
        .ok_or_else(|| anyhow::anyhow!("no_stream_found"))?;
    Ok(Ingestion {
        address: item.cdn.ingestion_info.ingestion_address,
        key: item.cdn.ingestion_info.stream_name,
    })
}

// ── Going live and ending ────────────────────────────────────────────────────

const BROADCASTS: &str = "https://www.googleapis.com/youtube/v3/liveBroadcasts";
const STREAMS: &str = "https://www.googleapis.com/youtube/v3/liveStreams";

/// Sends a YouTube Data API request and returns its JSON body. An error carries YouTube's reason
/// code (`errorStreamInactive`, `redundantTransition`, …) so callers can match on it.
async fn youtube_call(request: reqwest::RequestBuilder) -> anyhow::Result<serde_json::Value> {
    let response = request.send().await?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or_default();
    if status.is_success() {
        return Ok(body);
    }
    let reason = body["error"]["errors"][0]["reason"].as_str().unwrap_or("");
    anyhow::bail!(
        "youtube_api_{}: {reason} {}",
        status.as_u16(),
        body["error"]["message"]
    )
}

/// The broadcasts' states, normalized by [`super::broadcast_status`]; one call (1 quota unit)
/// for up to 50 ids. A broadcast YouTube no longer knows is missing from the map.
pub async fn broadcast_statuses(
    ids: &[String],
    access_token: &str,
) -> anyhow::Result<std::collections::HashMap<String, &'static str>> {
    let list = youtube_call(
        reqwest::Client::new()
            .get(BROADCASTS)
            .query(&[("part", "status"), ("id", ids.join(",").as_str())])
            .bearer_auth(access_token),
    )
    .await?;
    Ok(list["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let raw = item["status"]["lifeCycleStatus"].as_str()?;
            Some((
                item["id"].as_str()?.to_string(),
                super::broadcast_status("youtube", raw)?,
            ))
        })
        .collect())
}

async fn transition(broadcast_id: &str, to: &str, access_token: &str) -> anyhow::Result<()> {
    match youtube_call(
        reqwest::Client::new()
            .post(format!("{BROADCASTS}/transition"))
            .query(&[
                ("broadcastStatus", to),
                ("id", broadcast_id),
                ("part", "status"),
            ])
            .bearer_auth(access_token),
    )
    .await
    {
        Err(error) if error.to_string().contains("redundantTransition") => Ok(()),
        Err(error) if error.to_string().contains("errorStreamInactive") => {
            anyhow::bail!("encoder_not_streaming")
        }
        other => other.map(drop),
    }
}

/// Takes a scheduled broadcast live: binds it to the channel's stream if nothing is bound yet,
/// refuses with `encoder_not_streaming` while no video reaches that stream, then goes through
/// `testing` (our broadcasts have a monitor stream, so YouTube requires it) to `live`.
/// About 150 quota units.
pub async fn go_live(broadcast_id: &str, access_token: &str) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let list = youtube_call(
        client
            .get(BROADCASTS)
            .query(&[("part", "status,contentDetails"), ("id", broadcast_id)])
            .bearer_auth(access_token),
    )
    .await?;
    let broadcast = &list["items"][0];
    let raw = broadcast["status"]["lifeCycleStatus"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("broadcast_not_found"))?;
    match super::broadcast_status("youtube", raw) {
        Some("live") => anyhow::bail!("already_live"),
        Some("ended") => anyhow::bail!("broadcast_ended"),
        _ => {}
    }

    let stream_id = match broadcast["contentDetails"]["boundStreamId"].as_str() {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => {
            let streams = youtube_call(
                client
                    .get("https://www.googleapis.com/youtube/v3/liveStreams")
                    .query(&[("part", "id"), ("mine", "true")])
                    .bearer_auth(access_token),
            )
            .await?;
            let id = streams["items"][0]["id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("no_stream_found"))?
                .to_string();
            youtube_call(
                client
                    .post(format!("{BROADCASTS}/bind"))
                    .query(&[
                        ("id", broadcast_id),
                        ("part", "id"),
                        ("streamId", id.as_str()),
                    ])
                    .bearer_auth(access_token),
            )
            .await?;
            id
        }
    };

    let stream = youtube_call(
        client
            .get("https://www.googleapis.com/youtube/v3/liveStreams")
            .query(&[("part", "status"), ("id", stream_id.as_str())])
            .bearer_auth(access_token),
    )
    .await?;
    if stream["items"][0]["status"]["streamStatus"].as_str() != Some("active") {
        anyhow::bail!("encoder_not_streaming");
    }

    if raw != "testing" {
        transition(broadcast_id, "testing", access_token).await?;
        // YouTube takes a few seconds to reach `testing`; `live` is refused until it has.
        let mut tested = false;
        for _ in 0..15 {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let list = youtube_call(
                client
                    .get(BROADCASTS)
                    .query(&[("part", "status"), ("id", broadcast_id)])
                    .bearer_auth(access_token),
            )
            .await?;
            if list["items"][0]["status"]["lifeCycleStatus"].as_str() == Some("testing") {
                tested = true;
                break;
            }
        }
        if !tested {
            anyhow::bail!("youtube_testing_timeout");
        }
    }
    transition(broadcast_id, "live", access_token).await
}

/// Ends a live broadcast. It can't be restarted afterwards.
pub async fn end_broadcast(broadcast_id: &str, access_token: &str) -> anyhow::Result<()> {
    transition(broadcast_id, "complete", access_token).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn captions_go_to_the_stream_bound_to_the_broadcast() {
        let broadcasts = json!({ "items": [{ "id": "gIGntP_kidc",
            "contentDetails": { "boundStreamId": "stream-b" } }] });
        let streams = json!({ "items": [
            { "id": "stream-a", "contentDetails": { "closedCaptionsIngestionUrl": "https://a?cid=1" } },
            { "id": "stream-b", "contentDetails": { "closedCaptionsIngestionUrl": "https://b?cid=2" } },
        ] });
        let bound = bound_stream_id(&broadcasts);
        assert_eq!(bound.as_deref(), Some("stream-b"));
        assert_eq!(
            pick_caption_stream(&streams, bound.as_deref()),
            Some(CaptionIngestion {
                url: "https://b?cid=2".to_string(),
                stream_id: "stream-b".to_string(),
                bound: true,
            })
        );
    }

    #[test]
    fn an_unbound_broadcast_falls_back_to_the_first_stream_with_captions() {
        let unbound =
            json!({ "items": [{ "id": "x", "contentDetails": { "boundStreamId": "" } }] });
        assert_eq!(bound_stream_id(&unbound), None);
        assert_eq!(bound_stream_id(&json!({ "items": [] })), None);
        let streams = json!({ "items": [
            { "id": "rtmp-only", "contentDetails": {} },
            { "id": "stream-a", "contentDetails": { "closedCaptionsIngestionUrl": "https://a?cid=1" } },
        ] });
        let picked = pick_caption_stream(&streams, None).unwrap();
        assert_eq!(
            (picked.stream_id.as_str(), picked.bound),
            ("stream-a", false)
        );
        assert_eq!(pick_caption_stream(&streams, Some("gone")), None);
    }
}

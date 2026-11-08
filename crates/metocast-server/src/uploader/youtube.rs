use std::collections::HashMap;
use std::sync::Arc;

use tokio::io::AsyncReadExt;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::{UploadResume, UploadSource};
use crate::server::websocket::{
    broadcast_upload_completed, broadcast_upload_failed, broadcast_upload_progress,
};

const CHUNK_SIZE: u64 = 8 * 1024 * 1024; // 8 MB

pub struct UploadChunkResult {
    pub bytes_uploaded: u64,
    pub done: bool,
    pub video_id: Option<String>,
}

/// Initiate a new YouTube resumable upload session.
/// Returns the `upload_uri` (Location header) to use for subsequent PUT requests.
pub async fn initiate_resumable_upload(
    client: &reqwest::Client,
    token: &str,
    title: &str,
    description: &str,
    visibility: &str,
    file_size: u64,
) -> anyhow::Result<String> {
    let body = serde_json::json!({
        "snippet": {
            "title": title,
            "description": description,
        },
        "status": {
            "privacyStatus": visibility,
        }
    });

    let resp = client
        .post("https://www.googleapis.com/upload/youtube/v3/videos?uploadType=resumable&part=snippet,status")
        .bearer_auth(token)
        .header("X-Upload-Content-Type", "video/*")
        .header("X-Upload-Content-Length", file_size.to_string())
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() && resp.status().as_u16() != 200 {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "YouTube initiate upload failed ({}): {}",
            status,
            text
        ));
    }

    let upload_uri = resp
        .headers()
        .get("Location")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| anyhow::anyhow!("YouTube initiate upload: missing Location header"))?
        .to_string();

    Ok(upload_uri)
}

/// Query the current upload offset for a resumable upload (for crash recovery).
/// Returns the number of bytes already uploaded.
pub async fn query_upload_offset(
    client: &reqwest::Client,
    upload_uri: &str,
    file_size: u64,
) -> anyhow::Result<u64> {
    let resp = client
        .put(upload_uri)
        .header("Content-Length", "0")
        .header("Content-Range", format!("bytes */{file_size}"))
        .send()
        .await?;

    let status = resp.status().as_u16();
    if status == 308 {
        // Range header gives the bytes already received: "bytes=0-{last}"
        if let Some(range_val) = resp.headers().get("Range").and_then(|v| v.to_str().ok())
            && let Some(last) = range_val.strip_prefix("bytes=0-")
            && let Ok(n) = last.parse::<u64>()
        {
            return Ok(n + 1);
        }
        // No Range header means nothing uploaded yet
        return Ok(0);
    }
    if status == 200 || status == 201 {
        // Already complete
        return Ok(file_size);
    }
    Err(anyhow::anyhow!(
        "query_upload_offset: unexpected status {status}"
    ))
}

/// Upload a chunk of the file to YouTube.
/// Returns bytes_uploaded (cumulative), done flag, and video_id on completion.
pub async fn upload_chunk(
    client: &reqwest::Client,
    upload_uri: &str,
    file_path: &str,
    offset: u64,
    file_size: u64,
) -> anyhow::Result<UploadChunkResult> {
    let end = (offset + CHUNK_SIZE).min(file_size);
    let chunk_len = end - offset;

    let mut file = tokio::fs::File::open(file_path).await?;
    tokio::io::AsyncSeekExt::seek(&mut file, std::io::SeekFrom::Start(offset)).await?;

    let mut buf = vec![0u8; chunk_len as usize];
    file.read_exact(&mut buf).await?;

    let content_range = format!("bytes {offset}-{end_byte}/{file_size}", end_byte = end - 1);

    let resp = client
        .put(upload_uri)
        .header("Content-Length", chunk_len.to_string())
        .header("Content-Range", content_range)
        .header("Content-Type", "video/*")
        .body(buf)
        .send()
        .await?;

    let status = resp.status().as_u16();

    if status == 308 {
        // Resume Incomplete — not done yet
        return Ok(UploadChunkResult {
            bytes_uploaded: end,
            done: false,
            video_id: None,
        });
    }

    if status == 200 || status == 201 {
        // Completed
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        let video_id = body.get("id").and_then(|v| v.as_str()).map(String::from);
        return Ok(UploadChunkResult {
            bytes_uploaded: file_size,
            done: true,
            video_id,
        });
    }

    let text = resp.text().await.unwrap_or_default();
    Err(anyhow::anyhow!(
        "upload_chunk: unexpected status {status}: {text}"
    ))
}

/// Run the full YouTube resumable upload for a recording.
/// Handles initiation, chunking, progress broadcasting, and completion.
pub(super) async fn run_upload(
    pool: &sqlx::PgPool,
    ws_clients: &Arc<RwLock<HashMap<Uuid, crate::server::ws_client::ClientTx>>>,
    recording_id: Uuid,
    source: UploadSource<'_>,
    resume: UploadResume,
    token: &str,
) -> anyhow::Result<()> {
    let UploadSource {
        file_path,
        file_size,
        title,
        description,
        visibility,
    } = source;
    let UploadResume {
        locator: existing_uri,
        progress_bytes: _,
    } = resume;
    let client = reqwest::Client::new();
    let total = file_size as u64;

    // Step 1: get or create the upload URI
    let upload_uri = match existing_uri {
        Some(uri) if !uri.is_empty() => uri,
        _ => {
            let uri =
                initiate_resumable_upload(&client, token, title, description, visibility, total)
                    .await?;

            // Persist URI so a crash can resume
            sqlx::query(
                "UPDATE recording_uploads \
                 SET upload_uri = $1, state = 'uploading', started_at = NOW(), updated_at = NOW() \
                 WHERE recording_id = $2 AND platform = 'youtube'",
            )
            .bind(&uri)
            .bind(recording_id)
            .execute(pool)
            .await?;

            uri
        }
    };

    // Step 2: query current offset (handles crash recovery)
    let mut offset = query_upload_offset(&client, &upload_uri, total)
        .await
        .unwrap_or(0);

    if offset >= total {
        // Already complete (e.g. recovered after crash with full upload)
        return finalize_completed(pool, ws_clients, recording_id, "youtube", total, None).await;
    }

    // Step 3: upload in chunks
    loop {
        match upload_chunk(&client, &upload_uri, file_path, offset, total).await {
            Ok(result) => {
                offset = result.bytes_uploaded;

                // Persist progress
                sqlx::query(
                    "UPDATE recording_uploads \
                     SET progress_bytes = $1, total_bytes = $2, updated_at = NOW() \
                     WHERE recording_id = $3 AND platform = 'youtube'",
                )
                .bind(offset as i64)
                .bind(total as i64)
                .bind(recording_id)
                .execute(pool)
                .await?;

                broadcast_upload_progress(
                    ws_clients,
                    recording_id,
                    "youtube",
                    offset as i64,
                    total as i64,
                )
                .await;

                if result.done {
                    let video_id = result.video_id.as_deref();
                    let video_url = result
                        .video_id
                        .as_ref()
                        .map(|id| format!("https://www.youtube.com/watch?v={id}"));
                    finalize_completed(
                        pool,
                        ws_clients,
                        recording_id,
                        "youtube",
                        total,
                        Some((
                            result.video_id.clone().unwrap_or_default(),
                            video_url.clone().unwrap_or_default(),
                        )),
                    )
                    .await?;
                    tracing::info!(
                        "YouTube upload completed for recording {recording_id}: {:?}",
                        video_id
                    );
                    return Ok(());
                }
            }
            Err(e) => {
                tracing::error!("YouTube upload chunk error for {recording_id}: {e}");
                sqlx::query(
                    "UPDATE recording_uploads \
                     SET state = 'failed', error = $1, updated_at = NOW() \
                     WHERE recording_id = $2 AND platform = 'youtube'",
                )
                .bind(e.to_string())
                .bind(recording_id)
                .execute(pool)
                .await?;
                broadcast_upload_failed(ws_clients, recording_id, "youtube", &e.to_string()).await;
                return Err(e);
            }
        }
    }
}

async fn finalize_completed(
    pool: &sqlx::PgPool,
    ws_clients: &Arc<RwLock<HashMap<Uuid, crate::server::ws_client::ClientTx>>>,
    recording_id: Uuid,
    platform: &str,
    total_bytes: u64,
    ids: Option<(String, String)>,
) -> anyhow::Result<()> {
    let (video_id, video_url) = ids.unwrap_or_default();
    sqlx::query(
        "UPDATE recording_uploads \
         SET state = 'completed', progress_bytes = $1, total_bytes = $1, \
             video_id = $2, video_url = $3, completed_at = NOW(), updated_at = NOW() \
         WHERE recording_id = $4 AND platform = $5",
    )
    .bind(total_bytes as i64)
    .bind(&video_id)
    .bind(&video_url)
    .bind(recording_id)
    .bind(platform)
    .execute(pool)
    .await?;
    // The recording row carries the link the Mac app lists next to the file.
    if !video_id.is_empty() {
        sqlx::query(
            "UPDATE recordings SET uploaded = true, uploaded_at = NOW(), \
             video_id = $1, video_url = $2, updated_at = NOW() WHERE id = $3",
        )
        .bind(&video_id)
        .bind(&video_url)
        .bind(recording_id)
        .execute(pool)
        .await?;
    }

    broadcast_upload_completed(ws_clients, recording_id, platform, &video_id, &video_url).await;
    if !video_id.is_empty() {
        crate::transcription::captions_job::after_upload(pool, recording_id).await;
    }
    Ok(())
}

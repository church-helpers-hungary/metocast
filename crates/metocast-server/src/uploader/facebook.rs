use std::collections::HashMap;
use std::sync::Arc;

use tokio::io::AsyncReadExt;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::{UploadResume, UploadSource};
use crate::server::websocket::{
    broadcast_upload_completed, broadcast_upload_failed, broadcast_upload_progress,
};

/// Start a Facebook chunked upload session.
/// Returns `(session_id, start_offset, end_offset)`.
pub async fn start_upload(
    client: &reqwest::Client,
    token: &str,
    page_id: &str,
    file_size: u64,
) -> anyhow::Result<(String, u64, u64)> {
    let resp = client
        .post(format!("https://graph.facebook.com/v19.0/{page_id}/videos"))
        .bearer_auth(token)
        .form(&[
            ("upload_phase", "start"),
            ("file_size", &file_size.to_string()),
        ])
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "Facebook start_upload failed ({}): {}",
            status,
            text
        ));
    }

    #[derive(serde::Deserialize)]
    struct StartResponse {
        upload_session_id: String,
        start_offset: String,
        end_offset: String,
    }

    let body: StartResponse = resp.json().await?;
    let start: u64 = body.start_offset.parse().unwrap_or(0);
    let end: u64 = body.end_offset.parse().unwrap_or(file_size);

    Ok((body.upload_session_id, start, end))
}

/// Transfer a chunk of video data to Facebook.
/// Returns `(next_start_offset, next_end_offset)`.
pub async fn transfer_chunk(
    client: &reqwest::Client,
    token: &str,
    page_id: &str,
    session_id: &str,
    file_path: &str,
    offsets: std::ops::Range<u64>,
    file_size: u64,
) -> anyhow::Result<(u64, u64)> {
    let start_offset = offsets.start;
    let end_offset = offsets.end;
    let chunk_len = end_offset - start_offset;

    let mut file = tokio::fs::File::open(file_path).await?;
    tokio::io::AsyncSeekExt::seek(&mut file, std::io::SeekFrom::Start(start_offset)).await?;

    let mut buf = vec![0u8; chunk_len as usize];
    file.read_exact(&mut buf).await?;

    let part = reqwest::multipart::Part::bytes(buf)
        .file_name("chunk")
        .mime_str("application/octet-stream")?;

    let form = reqwest::multipart::Form::new()
        .text("upload_phase", "transfer")
        .text("upload_session_id", session_id.to_string())
        .text("start_offset", start_offset.to_string())
        .part("video_file_chunk", part);

    let resp = client
        .post(format!("https://graph.facebook.com/v19.0/{page_id}/videos"))
        .bearer_auth(token)
        .multipart(form)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "Facebook transfer_chunk failed ({}): {}",
            status,
            text
        ));
    }

    #[derive(serde::Deserialize)]
    struct TransferResponse {
        start_offset: String,
        end_offset: String,
    }

    let body: TransferResponse = resp.json().await?;
    let next_start: u64 = body.start_offset.parse().unwrap_or(file_size);
    let next_end: u64 = body.end_offset.parse().unwrap_or(file_size);

    Ok((next_start, next_end))
}

/// Finish the Facebook chunked upload and set video metadata.
/// Returns the video_id string.
pub async fn finish_upload(
    client: &reqwest::Client,
    token: &str,
    page_id: &str,
    session_id: &str,
    title: &str,
    description: &str,
    visibility: &str,
) -> anyhow::Result<String> {
    let privacy_json = serde_json::json!({ "value": visibility }).to_string();

    let resp = client
        .post(format!("https://graph.facebook.com/v19.0/{page_id}/videos"))
        .bearer_auth(token)
        .form(&[
            ("upload_phase", "finish"),
            ("upload_session_id", session_id),
            ("title", title),
            ("description", description),
            ("privacy", &privacy_json),
        ])
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "Facebook finish_upload failed ({}): {}",
            status,
            text
        ));
    }

    #[derive(serde::Deserialize)]
    struct FinishResponse {
        video_id: Option<String>,
        id: Option<String>,
    }

    let body: FinishResponse = resp.json().await.unwrap_or(FinishResponse {
        video_id: None,
        id: None,
    });

    Ok(body.video_id.or(body.id).unwrap_or_default())
}

/// Sends the recording's caption tracks (such as a transcript dropped on the Media page) to its
/// Facebook video, one SRT per language. Best effort: the video is up either way.
async fn upload_captions(
    client: &reqwest::Client,
    pool: &sqlx::PgPool,
    token: &str,
    recording_id: Uuid,
    video_id: &str,
) {
    let tracks: Vec<(String, String)> =
        sqlx::query_as("SELECT language, vtt FROM caption_tracks WHERE recording_id = $1")
            .bind(recording_id)
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    for (language, vtt) in tracks {
        // Facebook reads the locale from the file name: `<name>.hu_HU.srt`.
        let locale = format!(
            "{language}_{}",
            if language == "en" {
                "US".to_string()
            } else {
                language.to_uppercase()
            }
        );
        let srt = match metocast_transcription::captions::parse_cues(&vtt) {
            Ok(cues) => metocast_transcription::captions::render_srt(&cues),
            Err(e) => {
                tracing::warn!(recording = %recording_id, %language, "captions: unreadable track: {e}");
                continue;
            }
        };
        let file = reqwest::multipart::Part::text(srt)
            .file_name(format!("captions.{locale}.srt"))
            .mime_str("application/x-subrip");
        let sent = match file {
            Ok(file) => client
                .post(format!(
                    "https://graph.facebook.com/v19.0/{video_id}/captions"
                ))
                .bearer_auth(token)
                .multipart(reqwest::multipart::Form::new().part("captions_file", file))
                .send()
                .await
                .map_err(anyhow::Error::from),
            Err(e) => Err(e.into()),
        };
        match sent {
            Ok(response) if response.status().is_success() => {
                tracing::info!(recording = %recording_id, %locale, "captions: sent to Facebook");
            }
            Ok(response) => tracing::warn!(
                recording = %recording_id,
                %locale,
                "captions: Facebook refused ({}): {}",
                response.status(),
                response.text().await.unwrap_or_default()
            ),
            Err(e) => tracing::warn!(recording = %recording_id, %locale, "captions: Facebook: {e}"),
        }
    }
}

/// Run the full Facebook chunked upload for a recording.
pub(super) async fn run_upload(
    pool: &sqlx::PgPool,
    ws_clients: &Arc<RwLock<HashMap<Uuid, crate::server::ws_client::ClientTx>>>,
    recording_id: Uuid,
    source: UploadSource<'_>,
    resume: UploadResume,
    token: &str,
    page_id: &str,
) -> anyhow::Result<()> {
    let UploadSource {
        file_path,
        file_size,
        title,
        description,
        visibility,
    } = source;
    let UploadResume {
        locator: existing_session_id,
        progress_bytes,
    } = resume;
    let client = reqwest::Client::new();
    let total = file_size as u64;

    // Step 1: get or create upload session
    let (session_id, mut start_offset, mut end_offset) = match existing_session_id {
        Some(sid) if !sid.is_empty() => {
            // Resume from known offset
            let start = progress_bytes as u64;
            // Use a 10MB chunk or rest of file
            let end = (start + 10 * 1024 * 1024).min(total);
            (sid, start, end)
        }
        _ => {
            let (sid, s, e) = start_upload(&client, token, page_id, total).await?;

            sqlx::query(
                    "UPDATE recording_uploads \
                     SET upload_session_id = $1, state = 'uploading', started_at = NOW(), updated_at = NOW() \
                     WHERE recording_id = $2 AND platform = 'facebook'",
                )
                .bind(&sid)
                .bind(recording_id)
                .execute(pool)
                .await?;

            (sid, s, e)
        }
    };

    // Step 2: transfer chunks
    loop {
        if start_offset >= total {
            break;
        }

        match transfer_chunk(
            &client,
            token,
            page_id,
            &session_id,
            file_path,
            start_offset..end_offset,
            total,
        )
        .await
        {
            Ok((next_start, next_end)) => {
                // Persist progress
                sqlx::query(
                    "UPDATE recording_uploads \
                     SET progress_bytes = $1, total_bytes = $2, updated_at = NOW() \
                     WHERE recording_id = $3 AND platform = 'facebook'",
                )
                .bind(end_offset as i64)
                .bind(total as i64)
                .bind(recording_id)
                .execute(pool)
                .await?;

                broadcast_upload_progress(
                    ws_clients,
                    recording_id,
                    "facebook",
                    end_offset as i64,
                    total as i64,
                )
                .await;

                start_offset = next_start;
                end_offset = next_end;

                if start_offset >= total {
                    break;
                }
            }
            Err(e) => {
                tracing::error!("Facebook upload chunk error for {recording_id}: {e}");
                sqlx::query(
                    "UPDATE recording_uploads \
                     SET state = 'failed', error = $1, updated_at = NOW() \
                     WHERE recording_id = $2 AND platform = 'facebook'",
                )
                .bind(e.to_string())
                .bind(recording_id)
                .execute(pool)
                .await?;
                broadcast_upload_failed(ws_clients, recording_id, "facebook", &e.to_string()).await;
                return Err(e);
            }
        }
    }

    // Step 3: finish upload
    match finish_upload(
        &client,
        token,
        page_id,
        &session_id,
        title,
        description,
        visibility,
    )
    .await
    {
        Ok(video_id) => {
            let video_url = if video_id.is_empty() {
                String::new()
            } else {
                format!("https://www.facebook.com/video/{video_id}")
            };

            sqlx::query(
                "UPDATE recording_uploads \
                 SET state = 'completed', progress_bytes = $1, total_bytes = $1, \
                     video_id = $2, video_url = $3, completed_at = NOW(), updated_at = NOW() \
                 WHERE recording_id = $4 AND platform = 'facebook'",
            )
            .bind(total as i64)
            .bind(&video_id)
            .bind(&video_url)
            .bind(recording_id)
            .execute(pool)
            .await?;

            broadcast_upload_completed(ws_clients, recording_id, "facebook", &video_id, &video_url)
                .await;
            tracing::info!("Facebook upload completed for recording {recording_id}: {video_id}");
            if !video_id.is_empty() {
                upload_captions(&client, pool, token, recording_id, &video_id).await;
            }
            Ok(())
        }
        Err(e) => {
            tracing::error!("Facebook finish_upload error for {recording_id}: {e}");
            sqlx::query(
                "UPDATE recording_uploads \
                 SET state = 'failed', error = $1, updated_at = NOW() \
                 WHERE recording_id = $2 AND platform = 'facebook'",
            )
            .bind(e.to_string())
            .bind(recording_id)
            .execute(pool)
            .await?;
            broadcast_upload_failed(ws_clients, recording_id, "facebook", &e.to_string()).await;
            Err(e)
        }
    }
}

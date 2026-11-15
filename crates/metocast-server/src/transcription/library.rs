//! The transcript library: stored live-transcript sessions read back to list, read, download
//! (`.txt`, `.srt`, `.vtt`) and delete, and an OpenAI clean-up pass that keeps a better-reading
//! version of each line next to the raw one. Clean-up progress goes out as
//! `transcript.cleanup.state`.

use metocast_core::best_effort::LogGone;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use metocast_core::connectors::ConnectorConfig as _;
use metocast_core::protocol::ServerEvent;
use metocast_core::transcription::{
    CleanupProgress, CleanupStatus, OPENAI_CONFIG_KEY, OpenAiConfig, StoredSegment,
    StoredTranscript, TranscriptDetail,
};
use metocast_transcription::captions::{CaptionOptions, build_cues, render_srt, render_vtt};
use metocast_transcription::segmenter::Segment;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::database::settings;
use crate::server::AppState;

pub const CLEANUP_MODEL: &str = "gpt-5-mini";
const CHAT_URL: &str = "https://api.openai.com/v1/chat/completions";
/// Lines per request, and cleaned lines sent before them as context.
const CHUNK: usize = 40;
const CONTEXT: usize = 4;

const PROMPT: &str = "You edit the transcript of a Hungarian Reformed church service that was \
machine-translated into English live, sentence by sentence. Rewrite each line into clear, natural \
English: fix grammar, mistranslations, broken words and repeated fragments, and keep the speaker's \
meaning, voice and first person. Keep Bible references, names and quotations. Never add content, \
never summarise, never merge or split lines. You get JSON with `context` (the lines just before, \
already edited, for reference only) and `lines`. Answer with `lines`: exactly one string per input \
line, in the same order. Use an empty string for a line that is only filler or noise.";

// ── Reading ─────────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct SummaryRow {
    id: Uuid,
    event_id: Option<Uuid>,
    event_title: Option<String>,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    status: String,
    cleanup_status: String,
    cleanup_error: Option<String>,
    captions_delivered: i32,
    captions_failed: i32,
    caption_last_error: Option<String>,
    caption_stream_id: Option<String>,
    segment_count: i64,
    last_at: Option<DateTime<Utc>>,
}

/// `seq, started_at, ended_at, text, clean_text`.
type SegmentRow = (i32, DateTime<Utc>, DateTime<Utc>, String, Option<String>);

const SUMMARY_SQL: &str = "\
SELECT s.id, s.event_id, NULLIF(COALESCE(NULLIF(e.computed_title, ''), e.title), '') AS event_title, \
       s.started_at, s.ended_at, s.status, s.cleanup_status, s.cleanup_error, \
       s.captions_delivered, s.captions_failed, s.caption_last_error, s.caption_stream_id, \
       COUNT(g.seq) AS segment_count, MAX(g.ended_at) AS last_at \
FROM transcript_sessions s \
LEFT JOIN events e ON e.id = s.event_id \
LEFT JOIN transcript_segments g ON g.session_id = s.id \
WHERE ($1::uuid IS NULL OR s.event_id = $1) AND ($2::uuid IS NULL OR s.id = $2) \
GROUP BY s.id, e.id \
ORDER BY s.started_at DESC";

fn summary(row: SummaryRow, live: Option<Uuid>) -> StoredTranscript {
    let end = row.ended_at.or(row.last_at).unwrap_or(row.started_at);
    StoredTranscript {
        id: row.id,
        event_id: row.event_id,
        event_title: row.event_title,
        started_at: row.started_at,
        ended_at: row.ended_at,
        status: row.status,
        live: live == Some(row.id),
        duration_seconds: (end - row.started_at).num_milliseconds().max(0) as f64 / 1_000.0,
        segment_count: u32::try_from(row.segment_count).unwrap_or(u32::MAX),
        cleanup_status: CleanupStatus::parse(&row.cleanup_status),
        cleanup_error: row.cleanup_error,
        captions_delivered: u32::try_from(row.captions_delivered).unwrap_or_default(),
        captions_failed: u32::try_from(row.captions_failed).unwrap_or_default(),
        caption_last_error: row.caption_last_error,
        caption_stream_id: row.caption_stream_id,
    }
}

/// Every stored session, newest first, or one event's.
///
/// # Errors
/// Returns the database error.
pub async fn list(
    pool: &PgPool,
    event_id: Option<Uuid>,
    live: Option<Uuid>,
) -> Result<Vec<StoredTranscript>, sqlx::Error> {
    let rows: Vec<SummaryRow> = sqlx::query_as(SUMMARY_SQL)
        .bind(event_id)
        .bind(None::<Uuid>)
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|row| summary(row, live)).collect())
}

/// One session with every line, or `None` when there is no such session.
///
/// # Errors
/// Returns the database error.
pub async fn detail(
    pool: &PgPool,
    id: Uuid,
    live: Option<Uuid>,
) -> Result<Option<TranscriptDetail>, sqlx::Error> {
    let row: Option<SummaryRow> = sqlx::query_as(SUMMARY_SQL)
        .bind(None::<Uuid>)
        .bind(id)
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let segments: Vec<SegmentRow> = sqlx::query_as(
        "SELECT seq, started_at, ended_at, text, clean_text FROM transcript_segments \
             WHERE session_id = $1 ORDER BY seq",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    Ok(Some(TranscriptDetail {
        transcript: summary(row, live),
        segments: segments
            .into_iter()
            .map(
                |(seq, started_at, ended_at, text, clean_text)| StoredSegment {
                    seq: u32::try_from(seq).unwrap_or_default(),
                    started_at,
                    ended_at,
                    text,
                    clean_text,
                },
            )
            .collect(),
    }))
}

// ── Downloads ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Txt,
    Srt,
    Vtt,
}

/// The file's text: the raw lines, or the cleaned ones without the lines the clean-up dropped.
/// Times in `.srt` and `.vtt` count from the session's start.
#[must_use]
pub fn render(detail: &TranscriptDetail, format: Format, clean: bool) -> String {
    let start = detail.transcript.started_at;
    let at = |time: DateTime<Utc>| u64::try_from((time - start).num_milliseconds()).unwrap_or(0);
    let segments: Vec<Segment> = detail
        .segments
        .iter()
        .filter_map(|s| {
            let text = if clean {
                s.clean_text.as_deref().unwrap_or(&s.text)
            } else {
                &s.text
            };
            let text = text.trim();
            (!text.is_empty()).then(|| Segment {
                seq: s.seq,
                start_ms: at(s.started_at),
                end_ms: at(s.ended_at),
                text: text.to_string(),
            })
        })
        .collect();
    match format {
        Format::Txt => segments.iter().map(|s| format!("{}\n", s.text)).collect(),
        Format::Srt => render_srt(&build_cues(&segments, &CaptionOptions::default())),
        Format::Vtt => render_vtt(&build_cues(&segments, &CaptionOptions::default())),
    }
}

// ── Routes ──────────────────────────────────────────────────────────────────

fn live_session(state: &AppState) -> Option<Uuid> {
    state.transcription.state().session_id
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

fn database_error(e: &sqlx::Error) -> Response {
    tracing::error!("transcripts: {e}");
    error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    event_id: Option<Uuid>,
}

/// `GET /api/transcripts`: stored sessions, newest first.
pub async fn list_route(State(state): State<AppState>, Query(query): Query<ListQuery>) -> Response {
    match list(&state.pool, query.event_id, live_session(&state)).await {
        Ok(transcripts) => Json(transcripts).into_response(),
        Err(e) => database_error(&e),
    }
}

/// `GET /api/transcripts/{id}`: one session with every line.
pub async fn get_route(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    match detail(&state.pool, id, live_session(&state)).await {
        Ok(Some(detail)) => Json(detail).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "Transcript not found"),
        Err(e) => database_error(&e),
    }
}

#[derive(Deserialize)]
pub struct DownloadQuery {
    #[serde(default)]
    format: Format,
    #[serde(default)]
    clean: bool,
}

/// `GET /api/transcripts/{id}/download?format=txt|srt|vtt&clean=true`
pub async fn download_route(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<DownloadQuery>,
) -> Response {
    let detail = match detail(&state.pool, id, live_session(&state)).await {
        Ok(Some(detail)) => detail,
        Ok(None) => return error(StatusCode::NOT_FOUND, "Transcript not found"),
        Err(e) => return database_error(&e),
    };
    if query.clean && detail.transcript.cleanup_status != CleanupStatus::Succeeded {
        return error(StatusCode::CONFLICT, "transcript_not_cleaned");
    }
    let (content_type, extension) = match query.format {
        Format::Txt => ("text/plain; charset=utf-8", "txt"),
        Format::Srt => ("application/x-subrip; charset=utf-8", "srt"),
        Format::Vtt => ("text/vtt; charset=utf-8", "vtt"),
    };
    // The library holds English only: `.en` lets the Media page take it as English when dropped.
    let name = format!(
        "transcript-{}{}.en.{extension}",
        detail.transcript.started_at.format("%Y-%m-%d-%H%M"),
        if query.clean { "-clean" } else { "" }
    );
    (
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
        ],
        render(&detail, query.format, query.clean),
    )
        .into_response()
}

/// `DELETE /api/transcripts/{id}`: the live session and one being cleaned up are refused.
pub async fn delete_route(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    if live_session(&state) == Some(id) {
        return error(StatusCode::CONFLICT, "transcript_live");
    }
    let deleted = sqlx::query(
        "DELETE FROM transcript_sessions WHERE id = $1 AND cleanup_status <> 'running'",
    )
    .bind(id)
    .execute(&state.pool)
    .await;
    match deleted {
        Ok(done) if done.rows_affected() == 1 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => refusal(&state.pool, id).await,
        Err(e) => database_error(&e),
    }
}

/// Why a delete or a clean-up touched no row.
async fn refusal(pool: &PgPool, id: Uuid) -> Response {
    let status: Result<Option<String>, sqlx::Error> =
        sqlx::query_scalar("SELECT cleanup_status FROM transcript_sessions WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await;
    match status {
        Ok(Some(status)) if status == "running" => {
            error(StatusCode::CONFLICT, "transcript_cleanup_running")
        }
        Ok(Some(_)) => error(StatusCode::CONFLICT, "transcript_empty"),
        Ok(None) => error(StatusCode::NOT_FOUND, "Transcript not found"),
        Err(e) => database_error(&e),
    }
}

/// `POST /api/transcripts/{id}/cleanup`: starts the AI clean-up of a finished session; a
/// successful pass replaces an earlier clean version.
pub async fn cleanup_route(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    if live_session(&state) == Some(id) {
        return error(StatusCode::CONFLICT, "transcript_live");
    }
    let config: OpenAiConfig = settings::get_json(&state.pool, OPENAI_CONFIG_KEY).await;
    if !config.is_configured() {
        return error(StatusCode::BAD_REQUEST, "transcription_not_configured");
    }
    let claimed = sqlx::query(
        "UPDATE transcript_sessions SET cleanup_status = 'running', cleanup_error = NULL \
         WHERE id = $1 AND cleanup_status <> 'running' \
           AND EXISTS (SELECT 1 FROM transcript_segments WHERE session_id = $1)",
    )
    .bind(id)
    .execute(&state.pool)
    .await;
    match claimed {
        Ok(done) if done.rows_affected() == 1 => {}
        Ok(_) => return refusal(&state.pool, id).await,
        Err(e) => return database_error(&e),
    }
    let progress = progress(id, CleanupStatus::Running, 0.0, None);
    broadcast(&state, &progress).await;
    let job = state.clone();
    tokio::spawn(async move { run_cleanup(&job, id, &config.api_key, CHAT_URL).await });
    (StatusCode::ACCEPTED, Json(progress)).into_response()
}

// ── The clean-up pass ───────────────────────────────────────────────────────

/// A pass running when the server stopped never finishes: mark it failed.
///
/// # Errors
/// Returns the database error.
pub async fn fail_interrupted(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE transcript_sessions SET cleanup_status = 'failed', \
             cleanup_error = 'the server stopped during the clean-up' \
         WHERE cleanup_status = 'running'",
    )
    .execute(pool)
    .await
    .map(drop)
}

fn progress(
    id: Uuid,
    state: CleanupStatus,
    progress: f64,
    error: Option<String>,
) -> CleanupProgress {
    CleanupProgress {
        session_id: id,
        state,
        progress,
        error,
    }
}

async fn broadcast(state: &AppState, progress: &CleanupProgress) {
    let event = ServerEvent::TranscriptCleanupState {
        progress: progress.clone(),
    };
    let text = match serde_json::to_string(&event) {
        Ok(text) => text,
        Err(e) => return tracing::error!("transcripts: cannot encode progress: {e}"),
    };
    for tx in state.ws_clients.read().await.values() {
        tx.send(axum::extract::ws::Message::Text(text.clone().into()))
            .log_gone();
    }
}

async fn run_cleanup(state: &AppState, id: Uuid, api_key: &str, url: &str) {
    let result = clean_session(state, id, api_key, url).await;
    let (status, error) = match result {
        Ok(()) => (CleanupStatus::Succeeded, None),
        Err(e) => {
            tracing::warn!(session = %id, "transcripts: clean-up failed: {e}");
            (CleanupStatus::Failed, Some(e))
        }
    };
    let saved = sqlx::query(
        "UPDATE transcript_sessions SET cleanup_status = $1, cleanup_error = $2 WHERE id = $3",
    )
    .bind(status.as_str())
    .bind(&error)
    .bind(id)
    .execute(&state.pool)
    .await;
    if let Err(e) = saved {
        tracing::error!(session = %id, "transcripts: cannot record the clean-up result: {e}");
    }
    let done = if status == CleanupStatus::Succeeded {
        1.0
    } else {
        0.0
    };
    broadcast(state, &progress(id, status, done, error)).await;
}

/// Stores the cleaned lines only once every chunk is done, so a failed pass leaves an earlier
/// clean version as it was.
async fn clean_session(state: &AppState, id: Uuid, api_key: &str, url: &str) -> Result<(), String> {
    let rows: Vec<(i32, String)> = sqlx::query_as(
        "SELECT seq, text FROM transcript_segments WHERE session_id = $1 ORDER BY seq",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| e.to_string())?;
    let (seqs, texts): (Vec<i32>, Vec<String>) = rows.into_iter().unzip();
    let http = reqwest::Client::new();
    let chunks = texts.len().div_ceil(CHUNK);
    let mut cleaned: Vec<String> = Vec::with_capacity(texts.len());
    for (index, chunk) in texts.chunks(CHUNK).enumerate() {
        let context = &cleaned[cleaned.len().saturating_sub(CONTEXT)..];
        let lines = clean_chunk(&http, url, api_key, context, chunk).await?;
        cleaned.extend(lines);
        let done = (index + 1) as f64 / chunks as f64 * 0.99;
        broadcast(state, &progress(id, CleanupStatus::Running, done, None)).await;
    }
    save_clean(&state.pool, id, &seqs, &cleaned)
        .await
        .map_err(|e| e.to_string())
}

async fn save_clean(
    pool: &PgPool,
    id: Uuid,
    seqs: &[i32],
    cleaned: &[String],
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE transcript_segments g SET clean_text = c.text \
         FROM UNNEST($2::int[], $3::text[]) AS c(seq, text) \
         WHERE g.session_id = $1 AND g.seq = c.seq",
    )
    .bind(id)
    .bind(seqs)
    .bind(cleaned)
    .execute(pool)
    .await
    .map(drop)
}

/// One chunk of lines through the chat model, answered line for line.
// ponytail: no retry; a wrong line count fails the pass and the operator runs it again.
async fn clean_chunk(
    http: &reqwest::Client,
    url: &str,
    api_key: &str,
    context: &[String],
    lines: &[String],
) -> Result<Vec<String>, String> {
    let body = json!({
        "model": CLEANUP_MODEL,
        "reasoning_effort": "low",
        "messages": [
            { "role": "system", "content": PROMPT },
            { "role": "user", "content": json!({ "context": context, "lines": lines }).to_string() },
        ],
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "cleaned_lines",
                "strict": true,
                "schema": {
                    "type": "object",
                    "properties": { "lines": { "type": "array", "items": { "type": "string" } } },
                    "required": ["lines"],
                    "additionalProperties": false,
                },
            },
        },
    });
    let response = http
        .post(url)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("OpenAI request failed: {}", e.without_url()))?;
    let status = response.status();
    let answer: Value = response
        .json()
        .await
        .map_err(|e| format!("OpenAI response unreadable: {}", e.without_url()))?;
    if !status.is_success() {
        let message = answer["error"]["message"].as_str().unwrap_or_default();
        return Err(format!("OpenAI {status}: {message}"));
    }
    let content = answer["choices"][0]["message"]["content"]
        .as_str()
        .ok_or("the model sent no text")?;
    #[derive(Deserialize)]
    struct Answer {
        lines: Vec<String>,
    }
    let cleaned = serde_json::from_str::<Answer>(content)
        .map_err(|e| format!("the model's answer is not the expected JSON: {e}"))?
        .lines;
    if cleaned.len() != lines.len() {
        return Err(format!(
            "the model answered {} lines for {} sent; run the clean-up again",
            cleaned.len(),
            lines.len()
        ));
    }
    Ok(cleaned
        .into_iter()
        .map(|line| line.trim().to_string())
        .collect())
}

#[cfg(test)]
mod tests;

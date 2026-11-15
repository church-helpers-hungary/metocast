//! Posts committed segments to a YouTube broadcast's caption ingestion URL
//! (`closedCaptionsHttpPost`). Each POST carries `seq`, which must keep rising for the stream,
//! and a body of `YYYY-MM-DDTHH:MM:SS.mmm` (UTC) plus the caption, lines joined by `<br>`.

use metocast_core::best_effort::LogFailed;
use std::time::{Duration, Instant};

use chrono::{DateTime, NaiveDateTime, TimeDelta, Utc};
use metocast_core::transcription::LiveCaptionStatus;
use sqlx::PgPool;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::connectors::youtube;

const MAX_AGE: TimeDelta = TimeDelta::seconds(60);
const ATTEMPTS: u32 = 3;
const LINE_CHARS: usize = 42;
const LOOKUP_EVERY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct Caption {
    /// When the speech started.
    pub at: DateTime<Utc>,
    pub text: String,
}

/// Where a session's captions go.
#[derive(Debug, Clone)]
pub enum Target {
    /// The ingestion URL the operator pasted in the OpenAI settings.
    Url(String),
    /// Looked up on YouTube: the stream bound to this broadcast, or the channel's first stream
    /// while there is no broadcast or it is not bound yet.
    Youtube { broadcast_id: Option<String> },
}

/// Where captions are posted right now, and whether that can still change.
struct Destination {
    url: String,
    stream_id: Option<String>,
    settled: bool,
}

const NO_URL: &str = "No caption ingestion URL: set the broadcast to HTTP captions or paste the URL in the OpenAI settings";

/// Starts the sender for one session. It ends when the returned sender is dropped. Each caption's
/// outcome is counted on the session row (`captions_delivered`, `captions_failed`,
/// `caption_last_error`, `caption_stream_id`).
pub fn spawn(
    pool: PgPool,
    session_id: Uuid,
    target: Target,
    report: impl Fn(LiveCaptionStatus) + Send + 'static,
) -> mpsc::UnboundedSender<Caption> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Caption>();
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        let mut seq: u64 = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(MAX(caption_seq), 0) FROM transcript_sessions",
        )
        .fetch_one(&pool)
        .await
        .map_or(0, |n| u64::try_from(n).unwrap_or(0));
        let mut skew = TimeDelta::zero();
        let mut destination = match &target {
            Target::Url(url) => Some(Destination {
                url: url.clone(),
                stream_id: None,
                settled: true,
            }),
            Target::Youtube { .. } => None,
        };
        let mut looked_up: Option<Instant> = None;
        refresh(&pool, &target, &mut destination, &mut looked_up).await;
        let mut status = LiveCaptionStatus {
            ok: destination.is_some(),
            last_seq: seq,
            last_error: destination.is_none().then(|| NO_URL.to_string()),
        };
        report(status.clone());
        while let Some(caption) = rx.recv().await {
            refresh(&pool, &target, &mut destination, &mut looked_up).await;
            seq += 1;
            let outcome = match &destination {
                Some(destination) => deliver(&client, &destination.url, seq, &caption, skew).await,
                None => Err(NO_URL.to_string()),
            };
            let error = match outcome {
                Ok(server_time) => {
                    if let Some(server_time) = server_time {
                        skew = server_time - Utc::now();
                    }
                    status = LiveCaptionStatus {
                        ok: true,
                        last_seq: seq,
                        last_error: None,
                    };
                    None
                }
                Err(error) => {
                    tracing::warn!("transcription: YouTube caption {seq} not delivered: {error}");
                    status.ok = false;
                    status.last_error = Some(error.clone());
                    Some(error)
                }
            };
            sqlx::query(
                "UPDATE transcript_sessions SET caption_seq = $1, \
                 captions_delivered = captions_delivered + $2, \
                 captions_failed = captions_failed + $3, \
                 caption_last_error = COALESCE($4, caption_last_error), \
                 caption_stream_id = COALESCE($5, caption_stream_id) WHERE id = $6",
            )
            .bind(i64::try_from(seq).unwrap_or(i64::MAX))
            .bind(i32::from(error.is_none()))
            .bind(i32::from(error.is_some()))
            .bind(error)
            .bind(destination.as_ref().and_then(|d| d.stream_id.clone()))
            .bind(session_id)
            .execute(&pool)
            .await
            .log_failed();
            report(status.clone());
        }
    });
    tx
}

/// Looks the destination up again until it is the broadcast's bound stream (the transcript often
/// starts before the broadcast goes live and gets bound), at most every [`LOOKUP_EVERY`].
async fn refresh(
    pool: &PgPool,
    target: &Target,
    destination: &mut Option<Destination>,
    looked_up: &mut Option<Instant>,
) {
    let Target::Youtube { broadcast_id } = target else {
        return;
    };
    if destination.as_ref().is_some_and(|d| d.settled)
        || looked_up.is_some_and(|at| at.elapsed() < LOOKUP_EVERY)
    {
        return;
    }
    *looked_up = Some(Instant::now());
    let Some(token) = youtube::load_tokens(pool).await else {
        return;
    };
    match youtube::caption_ingestion(&token.access_token, broadcast_id.as_deref()).await {
        Ok(Some(found)) => {
            if found.bound {
                tracing::info!(stream = %found.stream_id, "transcription: captions go to the broadcast's bound stream");
            }
            *destination = Some(Destination {
                url: found.url,
                stream_id: Some(found.stream_id),
                // Without a broadcast the channel's first stream is all there is to find.
                settled: found.bound || broadcast_id.is_none(),
            });
        }
        Ok(None) => {}
        Err(e) => tracing::warn!("transcription: cannot read the caption ingestion URL: {e}"),
    }
}

/// Posts one caption, retrying a failed POST twice while it is under a minute old.
async fn deliver(
    client: &reqwest::Client,
    url: &str,
    seq: u64,
    caption: &Caption,
    skew: TimeDelta,
) -> Result<Option<DateTime<Utc>>, String> {
    let mut delay = Duration::from_millis(500);
    let mut outcome = Err("not sent".to_string());
    for _ in 0..ATTEMPTS {
        if Utc::now() - caption.at > MAX_AGE {
            return Err("dropped a caption older than 60 s".to_string());
        }
        outcome = post(client, url, seq, &body(caption.at + skew, &caption.text)).await;
        if outcome.is_ok() {
            break;
        }
        tokio::time::sleep(delay).await;
        delay *= 2;
    }
    outcome
}

/// The POST body: timestamp line, then the caption wrapped to at most two lines.
fn body(at: DateTime<Utc>, text: &str) -> String {
    format!(
        "{}\n{}\n",
        at.format("%Y-%m-%dT%H:%M:%S%.3f"),
        two_lines(text)
    )
}

fn two_lines(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= LINE_CHARS {
        return text;
    }
    let middle = text.len() / 2;
    let cut = text
        .match_indices(' ')
        .map(|(i, _)| i)
        .min_by_key(|i| i.abs_diff(middle))
        .unwrap_or(text.len());
    let (first, rest) = text.split_at(cut);
    format!("{first}<br>{}", rest.trim_start())
}

/// Returns YouTube's clock from the response body, when it sent one.
async fn post(
    client: &reqwest::Client,
    url: &str,
    seq: u64,
    body: &str,
) -> Result<Option<DateTime<Utc>>, String> {
    let response = client
        .post(format!("{url}&seq={seq}"))
        .header(reqwest::header::CONTENT_TYPE, "text/plain")
        .body(body.to_string())
        .timeout(Duration::from_secs(10))
        .send()
        .await
        // The URL carries the stream's signed caption id: never log it.
        .map_err(|e| e.without_url().to_string())?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("HTTP {status}: {}", text.trim()));
    }
    Ok(
        NaiveDateTime::parse_from_str(text.trim(), "%Y-%m-%dT%H:%M:%S%.f")
            .ok()
            .map(|t| t.and_utc()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    #[test]
    fn body_is_timestamp_then_two_lines() {
        let at = DateTime::parse_from_rfc3339("2026-10-04T09:41:07.25Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(body(at, "Amen."), "2026-10-04T09:41:07.250\nAmen.\n");
        let long = "Grace to you and peace from God our Father and the Lord Jesus Christ";
        let lines = two_lines(long);
        let (a, b) = lines.split_once("<br>").unwrap();
        assert!(a.len().abs_diff(b.len()) < 12, "{lines}");
        assert_eq!(format!("{a} {b}"), long);
    }

    /// One request against a local mock: path carries `seq`, the reply's clock is parsed.
    #[tokio::test]
    async fn posts_with_seq_and_reads_server_clock() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let n = socket.read(&mut request).await.unwrap();
            let reply = "2026-10-04T09:41:09.000";
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{reply}",
                        reply.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            String::from_utf8_lossy(&request[..n]).into_owned()
        });
        let url = format!("http://{address}/closedcaption?cid=abc");
        let clock = post(
            &reqwest::Client::new(),
            &url,
            7,
            "2026-10-04T09:41:07.250\nHi\n",
        )
        .await
        .unwrap()
        .unwrap();
        let request = server.await.unwrap();
        assert!(
            request.starts_with("POST /closedcaption?cid=abc&seq=7 "),
            "{request}"
        );
        assert!(request.ends_with("2026-10-04T09:41:07.250\nHi\n"));
        assert_eq!(clock.format("%H:%M:%S").to_string(), "09:41:09");
    }
}

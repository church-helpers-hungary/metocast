use super::*;
use axum::routing::post;
use chrono::TimeZone as _;

fn transcript() -> TranscriptDetail {
    let start = Utc.with_ymd_and_hms(2026, 10, 18, 9, 30, 0).unwrap();
    let at = |s: i64| start + chrono::TimeDelta::seconds(s);
    let segment = |seq, from, to, text: &str, clean: Option<&str>| StoredSegment {
        seq,
        started_at: at(from),
        ended_at: at(to),
        text: text.to_string(),
        clean_text: clean.map(str::to_string),
    };
    TranscriptDetail {
        transcript: StoredTranscript {
            id: Uuid::nil(),
            event_id: None,
            event_title: None,
            started_at: start,
            ended_at: Some(at(10)),
            status: "stopped".to_string(),
            live: false,
            duration_seconds: 10.0,
            segment_count: 3,
            cleanup_status: CleanupStatus::Succeeded,
            cleanup_error: None,
            captions_delivered: 0,
            captions_failed: 0,
            caption_last_error: None,
            caption_stream_id: None,
        },
        segments: vec![
            segment(
                1,
                1,
                3,
                "Grace to you and and peace.",
                Some("Grace to you and peace."),
            ),
            segment(2, 4, 5, "Um, so.", Some("")),
            segment(3, 6, 9, "Let us pray.", None),
        ],
    }
}

#[test]
fn downloads_render_raw_and_clean_lines_with_session_times() {
    let detail = transcript();
    assert_eq!(
        render(&detail, Format::Txt, false),
        "Grace to you and and peace.\nUm, so.\nLet us pray.\n"
    );
    // The clean version drops the filler line and keeps a line it never got as it was.
    assert_eq!(
        render(&detail, Format::Txt, true),
        "Grace to you and peace.\nLet us pray.\n"
    );
    let srt = render(&detail, Format::Srt, true);
    assert!(srt.starts_with("1\n00:00:01,000 --> "), "{srt}");
    assert!(srt.contains("Let us pray."));
    assert!(render(&detail, Format::Vtt, false).starts_with("WEBVTT\n\n00:00:01.000 --> "));
}

async fn mock(answer: Value) -> String {
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        post(move || async move { Json(answer) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

#[tokio::test]
async fn a_chunk_comes_back_line_for_line_or_fails() {
    let lines = vec!["Grace to you and and peace.".to_string(), "Um.".to_string()];
    let http = reqwest::Client::new();
    let ok = mock(json!({ "choices": [{ "message": {
        "content": json!({ "lines": [" Grace to you and peace. ", ""] }).to_string()
    } }] }))
    .await;
    assert_eq!(
        clean_chunk(&http, &ok, "key", &[], &lines).await.unwrap(),
        vec!["Grace to you and peace.".to_string(), String::new()]
    );
    let short = mock(json!({ "choices": [{ "message": {
        "content": json!({ "lines": ["Grace to you and peace."] }).to_string()
    } }] }))
    .await;
    let error = clean_chunk(&http, &short, "key", &[], &lines)
        .await
        .unwrap_err();
    assert!(error.contains("1 lines for 2"), "{error}");
}

/// With `METOCAST_TEST_DATABASE_URL` (a scratch database): a stored session lists with its event
/// and duration, reads back with its lines, keeps a clean version next to the raw one, and goes
/// with its lines when deleted.
#[tokio::test]
async fn a_stored_session_lists_reads_cleans_and_deletes() {
    let Ok(url) = std::env::var("METOCAST_TEST_DATABASE_URL") else {
        return;
    };
    let pool = PgPool::connect(&url).await.unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let event: Uuid = sqlx::query_scalar(
        "INSERT INTO events (title, date_time) VALUES ('Library test', now()) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO transcript_sessions (event_id, started_at, ended_at, status, \
         captions_delivered, captions_failed, caption_last_error, caption_stream_id) \
         VALUES ($1, '2026-10-18T09:30:00Z', '2026-10-18T09:31:30Z', 'stopped', \
         40, 2, 'HTTP 403 Forbidden', 'stream-b') RETURNING id",
    )
    .bind(event)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO transcript_segments (session_id, seq, started_at, ended_at, text) VALUES \
         ($1, 1, '2026-10-18T09:30:01Z', '2026-10-18T09:30:03Z', 'Grace to you and and peace.'), \
         ($1, 2, '2026-10-18T09:30:04Z', '2026-10-18T09:30:05Z', 'Um, so.')",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();

    let listed = list(&pool, Some(event), None).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);
    assert!(listed[0].event_title.is_some());
    assert_eq!(listed[0].duration_seconds, 90.0);
    assert_eq!(listed[0].segment_count, 2);
    assert_eq!(listed[0].cleanup_status, CleanupStatus::None);
    assert_eq!(
        (listed[0].captions_delivered, listed[0].captions_failed),
        (40, 2)
    );
    assert_eq!(
        listed[0].caption_last_error.as_deref(),
        Some("HTTP 403 Forbidden")
    );
    assert_eq!(listed[0].caption_stream_id.as_deref(), Some("stream-b"));
    assert!(
        list(&pool, None, Some(id))
            .await
            .unwrap()
            .iter()
            .any(|t| t.id == id && t.live)
    );

    save_clean(
        &pool,
        id,
        &[1, 2],
        &["Grace to you and peace.".to_string(), String::new()],
    )
    .await
    .unwrap();
    let detail = detail(&pool, id, None).await.unwrap().unwrap();
    assert_eq!(detail.segments.len(), 2);
    assert_eq!(detail.segments[0].text, "Grace to you and and peace.");
    assert_eq!(
        render(&detail, Format::Txt, true),
        "Grace to you and peace.\n"
    );

    sqlx::query("DELETE FROM events WHERE id = $1")
        .bind(event)
        .execute(&pool)
        .await
        .unwrap();
    assert!(detail_is_gone(&pool, id).await);
}

async fn detail_is_gone(pool: &PgPool, id: Uuid) -> bool {
    detail(pool, id, None).await.unwrap().is_none()
}

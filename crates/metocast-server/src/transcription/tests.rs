use super::*;
use crate::server::ws_client::Outgoing;
use futures_util::{SinkExt as _, StreamExt as _};
use metocast_core::best_effort::LogFailed;
use metocast_transcription::framer::Frame;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

#[test]
fn idle_status_orders_setup_problems_before_errors() {
    let status = |configured, mixer, error: Option<&str>| {
        idle_status(configured, mixer, error.map(str::to_string))
    };
    assert_eq!(
        status(false, true, None),
        (TranscriptionStatus::NotConfigured, None)
    );
    // The desk drops: unavailable, whatever came before.
    assert_eq!(
        status(true, false, Some("noCredit")),
        (
            TranscriptionStatus::Unavailable,
            Some("mixerOffline".to_string())
        )
    );
    // It comes back: ready again, and nothing starts by itself (only `start` creates a session).
    assert_eq!(status(true, true, None), (TranscriptionStatus::Ready, None));
    assert_eq!(
        status(true, true, Some("noCredit")),
        (TranscriptionStatus::Error, Some("noCredit".to_string()))
    );
}

#[test]
fn fatal_reasons_map_close_codes_and_error_codes() {
    assert_eq!(
        fatal_reason("1013 try again later", None).as_deref(),
        Some("noCredit")
    );
    assert_eq!(
        fatal_reason("3000 invalid", None).as_deref(),
        Some("invalidKey")
    );
    assert_eq!(
        fatal_reason("websocket: handshake rejected: HTTP 401 Unauthorized", None).as_deref(),
        Some("invalidKey")
    );
    assert_eq!(
        fatal_reason("closed", Some("insufficient_quota")).as_deref(),
        Some("noCredit")
    );
    assert_eq!(fatal_reason("connection reset", None), None);
}

fn test_service() -> Arc<TranscriptionService> {
    // No database in unit tests: every query fails fast and is logged.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(50))
        .connect_lazy("postgres://nobody@127.0.0.1:1/none")
        .expect("lazy pool");
    service_on(pool)
}

fn service_on(pool: sqlx::PgPool) -> Arc<TranscriptionService> {
    TranscriptionService::new(
        pool,
        Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        Arc::new(RodecasterConnector::new()),
    )
}

fn test_session(url: String) -> Session {
    let mut realtime = RealtimeConfig::new("sk-test".to_string());
    realtime.url = url;
    realtime.drain_idle = Duration::from_millis(200);
    Session {
        id: Uuid::new_v4(),
        started_at: Utc::now(),
        config: OpenAiConfig::default(),
        realtime,
        rotate_after: Duration::from_millis(300),
        force_rotate_after: Duration::from_secs(60),
    }
}

/// Feeds silent frames every 20 ms until the returned sender is dropped.
fn feed() -> (mpsc::Receiver<FrameResult>, oneshot::Sender<()>) {
    let (tx, rx) = mpsc::channel(64);
    let (stop, mut stopped) = oneshot::channel::<()>();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut stopped => return,
                () = tokio::time::sleep(Duration::from_millis(20)) => {}
            }
            let frame = Frame {
                pcm: vec![0; 2_400],
                onsets_ms: Vec::new(),
            };
            if tx.send(Ok(frame)).await.is_err() {
                return;
            }
        }
    });
    (rx, stop)
}

async fn delta(ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, text: &str) {
    let event = json!({ "type": "session.output_transcript.delta", "delta": text });
    ws.send(WsMessage::Text(event.to_string())).await.unwrap();
}

/// Upstream mock: connection 1 says one sentence and is rotated away; connection 2 says two
/// more and closes with 1013 (no credit). Segments must arrive once each, in order.
#[tokio::test]
async fn rotates_then_stops_on_no_credit_without_duplicates() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let upstream = tokio::spawn(async move {
        for connection in 0..2 {
            let (socket, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(socket).await.unwrap();
            ws.next().await; // session.update
            ws.send(WsMessage::Text(
                json!({ "type": "session.created" }).to_string(),
            ))
            .await
            .unwrap();
            if connection == 0 {
                delta(&mut ws, "Grace to you").await;
                delta(&mut ws, " and peace. ").await;
                // Read audio until the client rotates away and closes.
                while let Some(Ok(message)) = ws.next().await {
                    if message.is_close() {
                        break;
                    }
                }
            } else {
                delta(&mut ws, "Amen. ").await;
                delta(&mut ws, "Thanks be to God.").await;
                tokio::time::sleep(Duration::from_millis(100)).await;
                let close = CloseFrame {
                    code: CloseCode::from(1013),
                    reason: "insufficient credit".into(),
                };
                ws.close(Some(close)).await.unwrap();
                // Hang up only once the client has: dropping the socket with its audio unread
                // resets the connection and its next write fails before it reads the close.
                while ws.next().await.is_some() {}
            }
        }
    });

    // With METOCAST_TEST_DATABASE_URL (a scratch database) the rows are checked too.
    let database = std::env::var("METOCAST_TEST_DATABASE_URL").ok();
    let service = match &database {
        Some(url) => service_on(sqlx::PgPool::connect(url).await.unwrap()),
        None => test_service(),
    };
    let mut session = test_session(url);
    if database.is_some() {
        crate::database::run_migrations(&service.pool)
            .await
            .unwrap();
        let event: Uuid = sqlx::query_scalar(
            "INSERT INTO events (title, date_time) VALUES ('transcript test', now()) RETURNING id",
        )
        .fetch_one(&service.pool)
        .await
        .unwrap();
        session.id = sqlx::query_scalar(
            "INSERT INTO transcript_sessions (event_id) VALUES ($1) RETURNING id",
        )
        .bind(event)
        .fetch_one(&service.pool)
        .await
        .unwrap();
    }
    let session_id = session.id;
    let (client_tx, mut client_rx) = crate::server::ws_client::ClientTx::channel(false);
    service
        .clients
        .write()
        .await
        .insert(Uuid::new_v4(), client_tx);
    let (frames, _feeding) = feed();
    let (_stop, stopped) = oneshot::channel();
    let outcome = tokio::time::timeout(
        Duration::from_secs(20),
        Arc::clone(&service).run(session, frames, stopped),
    )
    .await
    .expect("the session ends");
    upstream.await.unwrap();

    assert_eq!(outcome, Outcome::Failed("noCredit".to_string()));
    let mut segments = Vec::new();
    while let Ok(Outgoing {
        message: Message::Text(text),
        ..
    }) = client_rx.try_recv()
    {
        let message: Value = serde_json::from_str(&text).unwrap();
        if message["type"] == "transcription.segment" {
            let segment = &message["segment"];
            segments.push((
                segment["seq"].as_u64().unwrap(),
                segment["text"].as_str().unwrap().to_string(),
            ));
        }
    }
    assert_eq!(
        segments,
        vec![
            (1, "Grace to you and peace.".to_string()),
            (2, "Amen.".to_string()),
            (3, "Thanks be to God.".to_string()),
        ]
    );
    assert_eq!(
        service.last_error.lock().unwrap().as_deref(),
        Some("noCredit")
    );
    if database.is_some() {
        let rows: Vec<(i32, String)> = sqlx::query_as(
            "SELECT seq, text FROM transcript_segments WHERE session_id = $1 ORDER BY seq",
        )
        .bind(session_id)
        .fetch_all(&service.pool)
        .await
        .unwrap();
        let expected: Vec<(i32, String)> = segments
            .iter()
            .map(|(seq, text)| (i32::try_from(*seq).unwrap(), text.clone()))
            .collect();
        assert_eq!(rows, expected);
        let status: String =
            sqlx::query_scalar("SELECT status FROM transcript_sessions WHERE id = $1")
                .bind(session_id)
                .fetch_one(&service.pool)
                .await
                .unwrap();
        assert_eq!(status, "error");
    }
}

/// With `METOCAST_TEST_DATABASE_URL` (a scratch database): a session may belong to no event, and
/// keeps its segments.
#[tokio::test]
async fn a_session_without_an_event_keeps_its_segments() {
    let Ok(url) = std::env::var("METOCAST_TEST_DATABASE_URL") else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let session: Uuid =
        sqlx::query_scalar("INSERT INTO transcript_sessions (event_id) VALUES ($1) RETURNING id")
            .bind(None::<Uuid>)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO transcript_segments (session_id, seq, started_at, ended_at, text) \
         VALUES ($1, 1, now(), now(), 'Grace to you')",
    )
    .bind(session)
    .execute(&pool)
    .await
    .unwrap();
    let kept: i64 =
        sqlx::query_scalar("SELECT count(*) FROM transcript_segments WHERE session_id = $1")
            .bind(session)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(kept, 1);
    // Segments go with their session.
    sqlx::query("DELETE FROM transcript_sessions WHERE id = $1")
        .bind(session)
        .execute(&pool)
        .await
        .unwrap();
}

/// The desk vanishing mid-run (its audio stops) ends the session as offline.
#[tokio::test]
async fn audio_loss_ends_the_session_offline() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(socket).await.unwrap();
        while ws.next().await.is_some() {}
    });
    let (frames, feeding) = feed();
    let (_stop, stopped) = oneshot::channel();
    let service = test_service();
    let run = tokio::spawn(Arc::clone(&service).run(test_session(url), frames, stopped));
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(feeding);
    let outcome = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("the session ends")
        .unwrap();
    assert_eq!(outcome, Outcome::Offline);
}

/// The server refuses mid-stream (1013) without reading what the client keeps sending, so its
/// close resets the connection and the client's blocked audio write fails. The close frame is
/// still readable and must be what the client reports, not the broken pipe.
#[tokio::test]
async fn a_close_frame_beats_a_failed_audio_write() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = RealtimeConfig::new("sk-test".to_string());
    config.url = format!("ws://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(socket).await.unwrap();
        ws.next().await; // session.update
        // Not reading: the audio piles up in the socket buffers until the client's write blocks.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let close = CloseFrame {
            code: CloseCode::from(1013),
            reason: "insufficient credit".into(),
        };
        ws.close(Some(close)).await.log_failed();
        // Dropped here with that audio unread: the kernel resets the connection.
    });
    // Far more than the socket buffers hold.
    let (audio_tx, audio) = mpsc::channel(2_048);
    for _ in 0..2_048 {
        audio_tx.send(vec![0; 2_400]).await.unwrap();
    }
    let (events, mut events_rx) = mpsc::unbounded_channel();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        realtime::run(config, audio, events),
    )
    .await
    .expect("the client ends");

    assert!(result.is_ok(), "the close frame was lost: {result:?}");
    let mut closed = None;
    while let Ok(event) = events_rx.try_recv() {
        if let RealtimeEvent::Closed { reason, .. } = event {
            closed = Some(reason);
        }
    }
    let reason = closed.expect("a closed event");
    assert_eq!(fatal_reason(&reason, None).as_deref(), Some("noCredit"));
}

/// Needs the network: a real TLS handshake with a throwaway key. The server links two rustls
/// backends (`aws-lc-rs` through reqwest, `ring` through tungstenite), so unless one is installed
/// as the process default the connect panics before any request is sent.
#[tokio::test]
#[ignore = "needs the network"]
async fn the_upstream_tls_handshake_does_not_panic() {
    let (_audio_tx, audio) = mpsc::channel(1);
    let (events, _events_rx) = mpsc::unbounded_channel();
    let config = RealtimeConfig::new("sk-not-a-real-key".to_string());
    let outcome = tokio::spawn(realtime::run(config, audio, events)).await;
    assert!(outcome.is_ok(), "the realtime task panicked: {outcome:?}");
}

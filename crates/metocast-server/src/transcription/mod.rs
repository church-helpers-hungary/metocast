//! Live Hungarian → English transcript. The service listens to one RØDECaster source, streams it
//! to OpenAI's realtime translation model and broadcasts the English text over `/ws`
//! (`transcription.state`, `.partial`, `.segment`, `.snapshot`, `.credit`).
//!
//! It only runs while the desk is connected: a disconnect ends the session (`stoppedOffline`) and
//! the operator starts it again by hand. Upstream sessions expire after about an hour, so the
//! connection is rotated at a pause after [`ROTATE_AFTER`].
pub mod audio;
pub mod captions_job;
mod credit;
pub mod library;
mod youtube_captions;

use metocast_core::best_effort::LogGone;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use axum::extract::ws::Message;
use chrono::{DateTime, TimeDelta, Utc};
use metocast_core::connectors::{ConnectorConfig as _, ConnectorStatus};
use metocast_core::transcription::{
    CreditStatus, LiveCaptionStatus, OPENAI_CONFIG_KEY, OpenAiConfig, TranscriptSegment,
    TranscriptionState, TranscriptionStatus,
};
use metocast_transcription::framer::FRAME_MS;
use metocast_transcription::realtime::{self, RealtimeConfig, RealtimeEvent};
use metocast_transcription::retime::{RetimeOptions, snap_start};
use metocast_transcription::segmenter::{SegmentEvent, Segmenter, SegmenterConfig};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::{Notify, mpsc, oneshot};
use uuid::Uuid;

use crate::connectors::rodecaster::RodecasterConnector;
use crate::connectors::youtube;
use crate::database::settings;
use audio::FrameResult;
use youtube_captions::Caption;

type WsClients = Arc<tokio::sync::RwLock<HashMap<Uuid, crate::server::ws_client::ClientTx>>>;

const ROTATE_AFTER: Duration = Duration::from_secs(55 * 60);
/// Rotate even mid-sentence before the upstream session expires at 60 minutes.
const FORCE_ROTATE_AFTER: Duration = Duration::from_secs(58 * 60);
const PARTIAL_EVERY: Duration = Duration::from_millis(150);
const RECENT_SEGMENTS: usize = 20;
const CREDIT_EVERY: Duration = Duration::from_secs(10 * 60);
/// No audio for this long means the desk is gone even if CoreAudio didn't say so.
const AUDIO_TIMEOUT: Duration = Duration::from_secs(5);
/// Audio kept while reconnecting or rotating, sent first on the next connection.
const PENDING_FRAMES: usize = 300;
const BACKOFF_MAX: Duration = Duration::from_secs(30);
const ONSET_WINDOW_MS: u64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Stopped,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    Stopped,
    Offline,
    Failed(String),
}

enum Step {
    Rotate,
    Retry { error: String, lasted: Duration },
    Done(Outcome),
}

struct Active {
    session_id: Uuid,
    stop: oneshot::Sender<End>,
}

pub struct TranscriptionService {
    pool: PgPool,
    clients: WsClients,
    rodecaster: Arc<RodecasterConnector>,
    state: RwLock<TranscriptionState>,
    /// A fatal upstream error (`noCredit`, `invalidKey`) shown until the next start or save.
    last_error: Mutex<Option<String>>,
    recent: Mutex<VecDeque<TranscriptSegment>>,
    partial: Mutex<String>,
    credit: Mutex<Option<CreditStatus>>,
    active: tokio::sync::Mutex<Option<Active>>,
    credit_wake: Notify,
}

/// One running session's fixed facts.
struct Session {
    id: Uuid,
    started_at: DateTime<Utc>,
    config: OpenAiConfig,
    realtime: RealtimeConfig,
    rotate_after: Duration,
    force_rotate_after: Duration,
}

impl Session {
    fn at(&self, audio_ms: u64) -> DateTime<Utc> {
        self.started_at + TimeDelta::milliseconds(i64::try_from(audio_ms).unwrap_or(i64::MAX))
    }
}

/// What a session accumulates across its upstream connections.
struct Live {
    segmenter: Segmenter,
    /// Session audio position of the next frame, in ms.
    audio_ms: u64,
    onsets: VecDeque<u64>,
    pending: VecDeque<(u64, Vec<i16>)>,
    last_partial: Instant,
    captions: Option<mpsc::UnboundedSender<Caption>>,
}

impl Live {
    /// Takes one frame from the tap; `None` means the audio is gone.
    fn frame(
        &mut self,
        frame: Result<Option<FrameResult>, tokio::time::error::Elapsed>,
    ) -> Option<(u64, Vec<i16>)> {
        let frame = match frame {
            Ok(Some(Ok(frame))) => frame,
            Ok(Some(Err(error))) => {
                tracing::warn!("transcription: {error}");
                return None;
            }
            Ok(None) | Err(_) => return None,
        };
        let at = self.audio_ms;
        self.audio_ms += FRAME_MS;
        self.onsets.extend(frame.onsets_ms);
        while self
            .onsets
            .front()
            .is_some_and(|&o| o + ONSET_WINDOW_MS < self.audio_ms)
        {
            self.onsets.pop_front();
        }
        Some((at, frame.pcm))
    }

    fn hold(&mut self, at: u64, pcm: Vec<i16>) {
        if self.pending.len() == PENDING_FRAMES {
            self.pending.pop_front();
        }
        self.pending.push_back((at, pcm));
    }
}

impl TranscriptionService {
    #[must_use]
    pub fn new(
        pool: PgPool,
        clients: WsClients,
        rodecaster: Arc<RodecasterConnector>,
    ) -> Arc<Self> {
        Arc::new(Self {
            pool,
            clients,
            rodecaster,
            state: RwLock::new(TranscriptionState::idle(
                TranscriptionStatus::NotConfigured,
                None,
            )),
            last_error: Mutex::new(None),
            recent: Mutex::new(VecDeque::new()),
            partial: Mutex::new(String::new()),
            credit: Mutex::new(None),
            active: tokio::sync::Mutex::new(None),
            credit_wake: Notify::new(),
        })
    }

    #[must_use]
    pub fn state(&self) -> TranscriptionState {
        self.state.read().expect("transcription state").clone()
    }

    /// `transcription.snapshot`, sent to every client on connect.
    #[must_use]
    pub fn snapshot_message(&self) -> String {
        json!({
            "type": "transcription.snapshot",
            "state": self.state(),
            "segments": self.recent.lock().expect("recent segments").iter().collect::<Vec<_>>(),
            "partial": *self.partial.lock().expect("partial line"),
            "credit": *self.credit.lock().expect("credit"),
        })
        .to_string()
    }

    /// Follows the desk (a disconnect stops a running session) and refreshes the credit line.
    pub fn spawn_watchers(self: &Arc<Self>) {
        let service = Arc::clone(self);
        tokio::spawn(async move {
            let mut status_rx = service.rodecaster.status_tx.subscribe();
            let mut profile_rx = service.rodecaster.profile_tx.subscribe();
            service.refresh().await;
            loop {
                tokio::select! {
                    status = status_rx.recv() => match status {
                        Ok(ConnectorStatus::Connected) => {}
                        Ok(_) => service.end(End::Offline).await,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    },
                    profile = profile_rx.recv() => if let Err(tokio::sync::broadcast::error::RecvError::Closed) = profile {
                        break;
                    },
                }
                service.refresh().await;
            }
        });
        let service = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                service.refresh_credit().await;
                tokio::select! {
                    () = tokio::time::sleep(CREDIT_EVERY) => {}
                    () = service.credit_wake.notified() => {}
                }
            }
        });
    }

    /// Re-reads the config after a save: clears a sticky error and re-checks the credit.
    pub async fn config_changed(&self) {
        *self.last_error.lock().expect("last error") = None;
        self.refresh().await;
        self.credit_wake.notify_one();
    }

    /// Starts a session for `event_id`, or for no event at all; a running session is returned
    /// unchanged.
    ///
    /// # Errors
    /// `transcription_not_configured`, `transcription_unavailable` (no desk), `event_not_found`,
    /// or why the audio or database refused.
    pub async fn start(
        self: &Arc<Self>,
        event_id: Option<Uuid>,
    ) -> Result<TranscriptionState, String> {
        let mut active = self.active.lock().await;
        if active.is_some() {
            return Ok(self.state());
        }
        let config: OpenAiConfig = settings::get_json(&self.pool, OPENAI_CONFIG_KEY).await;
        if !config.is_configured() {
            return Err("transcription_not_configured".to_string());
        }
        let profile = self
            .mixer_profile()
            .ok_or_else(|| "transcription_unavailable".to_string())?;
        if let Some(event_id) = event_id {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM events WHERE id = $1)")
                    .bind(event_id)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(|e| e.to_string())?;
            if !exists {
                return Err("event_not_found".to_string());
            }
        }
        let source = config.source.clone();
        let frames = tokio::task::spawn_blocking(move || audio::start(&source, &profile))
            .await
            .map_err(|e| format!("transcription_unavailable: {e}"))?
            .map_err(|e| format!("transcription_unavailable: {e}"))?;
        let (id, started_at): (Uuid, DateTime<Utc>) = sqlx::query_as(
            "INSERT INTO transcript_sessions (event_id) VALUES ($1) RETURNING id, started_at",
        )
        .bind(event_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut realtime = RealtimeConfig::new(config.api_key.clone());
        realtime.source_transcription_model = None; // the Hungarian text is never shown
        let session = Session {
            id,
            started_at,
            config,
            realtime,
            rotate_after: ROTATE_AFTER,
            force_rotate_after: FORCE_ROTATE_AFTER,
        };
        *self.last_error.lock().expect("last error") = None;
        self.recent.lock().expect("recent segments").clear();
        self.partial.lock().expect("partial line").clear();
        let state = TranscriptionState {
            status: TranscriptionStatus::Connecting,
            reason: None,
            session_id: Some(id),
            event_id,
            started_at: Some(started_at),
            youtube: None,
        };
        self.set_state(state.clone()).await;
        let (stop, stopped) = oneshot::channel();
        *active = Some(Active {
            session_id: id,
            stop,
        });
        drop(active);
        tracing::info!(session = %id, event = ?event_id, "transcription: started");
        let service = Arc::clone(self);
        tokio::spawn(async move {
            service.run(session, frames, stopped).await;
        });
        Ok(state)
    }

    pub async fn stop(&self) {
        self.end(End::Stopped).await;
    }

    async fn end(&self, end: End) {
        if let Some(active) = self.active.lock().await.take() {
            active.stop.send(end).log_gone();
        }
    }

    fn mixer_profile(&self) -> Option<rodecaster::Profile> {
        (self.rodecaster.get_status() == ConnectorStatus::Connected)
            .then(|| self.rodecaster.get_profile())
            .flatten()
    }

    /// Recomputes the status while nothing is running.
    async fn refresh(&self) {
        if self.active.lock().await.is_some() {
            return;
        }
        let config: OpenAiConfig = settings::get_json(&self.pool, OPENAI_CONFIG_KEY).await;
        let last_error = self.last_error.lock().expect("last error").clone();
        let (status, reason) = idle_status(
            config.is_configured(),
            self.mixer_profile().is_some(),
            last_error,
        );
        self.set_state(TranscriptionState::idle(status, reason))
            .await;
    }

    async fn refresh_credit(&self) {
        let config: OpenAiConfig = settings::get_json(&self.pool, OPENAI_CONFIG_KEY).await;
        match credit::fetch(&config).await {
            Ok(credit) => {
                *self.credit.lock().expect("credit") = credit.clone();
                if let Some(credit) = credit {
                    self.broadcast(&json!({ "type": "transcription.credit", "credit": credit }))
                        .await;
                }
            }
            Err(error) => tracing::warn!("transcription: credit check failed: {error}"),
        }
    }

    async fn set_state(&self, state: TranscriptionState) {
        {
            let mut current = self.state.write().expect("transcription state");
            if *current == state {
                return;
            }
            current.clone_from(&state);
        }
        self.broadcast(&json!({ "type": "transcription.state", "state": state }))
            .await;
    }

    async fn update_state(&self, change: impl FnOnce(&mut TranscriptionState)) {
        let mut state = self.state();
        change(&mut state);
        self.set_state(state).await;
    }

    async fn broadcast(&self, message: &Value) {
        let text = message.to_string();
        for tx in self.clients.read().await.values() {
            tx.send(Message::Text(text.clone().into())).log_gone();
        }
    }

    async fn run(
        self: Arc<Self>,
        session: Session,
        mut frames: mpsc::Receiver<FrameResult>,
        mut stopped: oneshot::Receiver<End>,
    ) -> Outcome {
        let mut live = Live {
            segmenter: Segmenter::new(SegmenterConfig::default()),
            audio_ms: 0,
            onsets: VecDeque::new(),
            pending: VecDeque::new(),
            last_partial: Instant::now(),
            captions: self.live_captions(&session).await,
        };
        let mut backoff = Duration::from_secs(1);
        let outcome = loop {
            match self
                .connection(&session, &mut live, &mut frames, &mut stopped)
                .await
            {
                Step::Rotate => {
                    tracing::info!(session = %session.id, "transcription: rotated the upstream session");
                }
                Step::Retry { error, lasted } => {
                    if lasted > Duration::from_secs(60) {
                        backoff = Duration::from_secs(1);
                    }
                    tracing::warn!("transcription: reconnecting in {backoff:?}: {error}");
                    self.update_state(|s| {
                        s.status = TranscriptionStatus::Connecting;
                        s.reason = Some(error.clone());
                    })
                    .await;
                    let wait = tokio::time::sleep(backoff);
                    tokio::pin!(wait);
                    let ended = loop {
                        tokio::select! {
                            () = &mut wait => break None,
                            end = &mut stopped => break Some(outcome_of(end)),
                            frame = tokio::time::timeout(AUDIO_TIMEOUT, frames.recv()) => match live.frame(frame) {
                                Some((at, pcm)) => live.hold(at, pcm),
                                None => break Some(Outcome::Offline),
                            },
                        }
                    };
                    if let Some(outcome) = ended {
                        break outcome;
                    }
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                }
                Step::Done(outcome) => break outcome,
            }
        };

        let events = live.segmenter.finish(live.audio_ms);
        self.apply(&session, &mut live, events).await;
        let (db_status, error) = match &outcome {
            Outcome::Stopped => ("stopped", None),
            Outcome::Offline => ("stoppedOffline", None),
            Outcome::Failed(reason) => ("error", Some(reason.clone())),
        };
        tracing::info!(session = %session.id, status = db_status, "transcription: ended");
        if let Err(e) = sqlx::query(
            "UPDATE transcript_sessions SET ended_at = now(), status = $1 WHERE id = $2",
        )
        .bind(db_status)
        .bind(session.id)
        .execute(&self.pool)
        .await
        {
            tracing::error!("transcription: cannot close session {}: {e}", session.id);
        }
        *self.last_error.lock().expect("last error") = error;
        {
            let mut active = self.active.lock().await;
            match active.as_ref().map(|a| a.session_id) {
                Some(id) if id != session.id => return outcome, // a newer session owns the state
                Some(_) => *active = None,
                None => {}
            }
        }
        self.refresh().await;
        self.credit_wake.notify_one();
        outcome
    }

    /// One upstream connection, until it closes, is rotated, or the session ends.
    async fn connection(
        &self,
        session: &Session,
        live: &mut Live,
        frames: &mut mpsc::Receiver<FrameResult>,
        stopped: &mut oneshot::Receiver<End>,
    ) -> Step {
        let (audio_tx, audio_rx) = mpsc::channel::<Vec<i16>>(PENDING_FRAMES + 64);
        let (events_tx, mut events) = mpsc::unbounded_channel();
        let client = tokio::spawn(realtime::run(session.realtime.clone(), audio_rx, events_tx));
        let opened = Instant::now();
        // Realtime event times count the audio this connection was sent.
        let offset = live.pending.front().map_or(live.audio_ms, |(at, _)| *at);
        for (_, pcm) in live.pending.drain(..) {
            audio_tx.try_send(pcm).log_gone();
        }
        let mut audio_tx = Some(audio_tx);
        let mut server_error: Option<String> = None;
        let mut close_reason = String::new();
        loop {
            tokio::select! {
                end = &mut *stopped => return Step::Done(outcome_of(end)),
                frame = tokio::time::timeout(AUDIO_TIMEOUT, frames.recv()) => {
                    let Some((at, pcm)) = live.frame(frame) else {
                        return Step::Done(Outcome::Offline);
                    };
                    match &audio_tx {
                        Some(tx) => {
                            tx.try_send(pcm).log_gone();
                        }
                        None => live.hold(at, pcm),
                    }
                    let age = opened.elapsed();
                    if audio_tx.is_some()
                        && (age >= session.force_rotate_after
                            || (age >= session.rotate_after && live.segmenter.is_idle()))
                    {
                        audio_tx = None; // the client drains what is in flight, then closes
                    }
                }
                event = events.recv() => {
                    let Some(event) = event else {
                        let error = match client.await {
                            Ok(Ok(())) => close_reason,
                            Ok(Err(e)) => e.to_string(),
                            Err(e) => e.to_string(),
                        };
                        if let Some(reason) = fatal_reason(&error, server_error.as_deref()) {
                            return Step::Done(Outcome::Failed(reason));
                        }
                        if audio_tx.is_none() {
                            return Step::Rotate;
                        }
                        return Step::Retry { error, lasted: opened.elapsed() };
                    };
                    match event {
                        RealtimeEvent::AudioSent { at_ms } => {
                            let events = live.segmenter.tick(offset + at_ms);
                            self.apply(session, live, events).await;
                        }
                        RealtimeEvent::TargetDelta { at_ms, text } => {
                            self.mark_running().await;
                            let events = live.segmenter.push(offset + at_ms, &text);
                            self.apply(session, live, events).await;
                        }
                        RealtimeEvent::Server { kind, raw, .. } if kind == "error" => {
                            let error = &raw["error"];
                            let code = error["code"].as_str().or_else(|| error["type"].as_str());
                            tracing::warn!("transcription: upstream error: {}", error["message"]);
                            server_error = code.map(str::to_string);
                        }
                        RealtimeEvent::Server { kind, .. } if kind.starts_with("session.") => {
                            self.mark_running().await;
                        }
                        RealtimeEvent::Closed { reason, .. } => close_reason = reason,
                        _ => {}
                    }
                }
            }
        }
    }

    async fn mark_running(&self) {
        if self.state().status != TranscriptionStatus::Running {
            self.update_state(|s| {
                s.status = TranscriptionStatus::Running;
                s.reason = None;
            })
            .await;
        }
    }

    async fn apply(&self, session: &Session, live: &mut Live, events: Vec<SegmentEvent>) {
        for event in events {
            match event {
                SegmentEvent::Partial(text) => {
                    // ponytail: a dropped partial is replaced by the next one or the commit.
                    if !text.is_empty() && live.last_partial.elapsed() < PARTIAL_EVERY {
                        continue;
                    }
                    live.last_partial = Instant::now();
                    self.partial.lock().expect("partial line").clone_from(&text);
                    self.broadcast(&json!({ "type": "transcription.partial", "text": text }))
                        .await;
                }
                SegmentEvent::Committed(segment) => {
                    let committed = TranscriptSegment {
                        seq: segment.seq,
                        started_at: session.at(segment.start_ms),
                        ended_at: session.at(segment.end_ms),
                        text: segment.text.clone(),
                    };
                    if let Err(e) = sqlx::query(
                        "INSERT INTO transcript_segments (session_id, seq, started_at, ended_at, text) \
                         VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
                    )
                    .bind(session.id)
                    .bind(i32::try_from(committed.seq).unwrap_or(i32::MAX))
                    .bind(committed.started_at)
                    .bind(committed.ended_at)
                    .bind(&committed.text)
                    .execute(&self.pool)
                    .await
                    {
                        tracing::error!("transcription: cannot store segment: {e}");
                    }
                    {
                        let mut recent = self.recent.lock().expect("recent segments");
                        if recent.len() == RECENT_SEGMENTS {
                            recent.pop_front();
                        }
                        recent.push_back(committed.clone());
                    }
                    self.broadcast(
                        &json!({ "type": "transcription.segment", "segment": committed }),
                    )
                    .await;
                    if let Some(captions) = &live.captions {
                        let options = RetimeOptions {
                            lag_ms: session.config.lag_ms,
                            ..RetimeOptions::default()
                        };
                        let speech =
                            snap_start(segment.start_ms, live.onsets.make_contiguous(), &options);
                        captions
                            .send(Caption {
                                at: session.at(speech),
                                text: segment.text,
                            })
                            .log_gone();
                    }
                }
            }
        }
    }

    /// The YouTube caption sender, when live captions are on and an ingestion URL is known.
    async fn live_captions(
        self: &Arc<Self>,
        session: &Session,
    ) -> Option<mpsc::UnboundedSender<Caption>> {
        if !session.config.youtube_live_captions {
            return None;
        }
        let mut url = session.config.caption_ingestion_url.trim().to_string();
        if url.is_empty()
            && let Some(token) = youtube::load_tokens(&self.pool).await
        {
            match youtube::caption_ingestion_url(&token.access_token).await {
                Ok(Some(found)) => url = found,
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("transcription: cannot read the caption ingestion URL: {e}")
                }
            }
        }
        if url.is_empty() {
            let failed = LiveCaptionStatus {
                ok: false,
                last_seq: 0,
                last_error: Some("No caption ingestion URL: set the broadcast to HTTP captions or paste the URL in the OpenAI settings".to_string()),
            };
            self.update_state(|s| s.youtube = Some(failed)).await;
            return None;
        }
        let service = Arc::clone(self);
        Some(youtube_captions::spawn(
            self.pool.clone(),
            session.id,
            url,
            move |status| {
                let service = Arc::clone(&service);
                tokio::spawn(async move {
                    service.update_state(|s| s.youtube = Some(status)).await;
                });
            },
        ))
    }
}

/// Whether new YouTube broadcasts should take HTTP-POST captions from the live transcript.
pub async fn live_captions_enabled(pool: &PgPool) -> bool {
    let config: OpenAiConfig = settings::get_json(pool, OPENAI_CONFIG_KEY).await;
    config.is_configured() && config.youtube_live_captions
}

fn outcome_of(end: Result<End, oneshot::error::RecvError>) -> Outcome {
    match end {
        Ok(End::Offline) => Outcome::Offline,
        Ok(End::Stopped) | Err(_) => Outcome::Stopped,
    }
}

/// The status while nothing runs: setup problems first, then a sticky upstream error.
fn idle_status(
    configured: bool,
    mixer_online: bool,
    last_error: Option<String>,
) -> (TranscriptionStatus, Option<String>) {
    if !configured {
        (TranscriptionStatus::NotConfigured, None)
    } else if !mixer_online {
        (
            TranscriptionStatus::Unavailable,
            Some("mixerOffline".to_string()),
        )
    } else if let Some(error) = last_error {
        (TranscriptionStatus::Error, Some(error))
    } else {
        (TranscriptionStatus::Ready, None)
    }
}

/// Upstream failures that retrying cannot fix: close 1013 / `insufficient_quota` (no credit) and
/// close 3000 / HTTP 401 / `invalid_api_key` (bad key).
fn fatal_reason(error: &str, server_code: Option<&str>) -> Option<String> {
    let no_credit = server_code == Some("insufficient_quota")
        || error.starts_with("1013")
        || error.contains("insufficient_quota");
    let invalid_key = server_code == Some("invalid_api_key")
        || error.starts_with("3000")
        || error.contains("HTTP 401")
        || error.contains("invalid_api_key");
    if no_credit {
        Some("noCredit".to_string())
    } else if invalid_key {
        Some("invalidKey".to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests;

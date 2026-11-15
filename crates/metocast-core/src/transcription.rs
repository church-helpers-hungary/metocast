//! Live Hungarian → English transcription: the `openai` connector config and the state the core
//! broadcasts as `transcription.*` WebSocket messages.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::connectors::{ConnectorConfig, RodecasterAudioSource};

/// Below this balance the credit line turns red.
pub const LOW_CREDIT_USD: f64 = 5.0;
pub const OPENAI_CONFIG_KEY: &str = "openai_config";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TopUp {
    pub amount_usd: f64,
    pub date: NaiveDate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct OpenAiConfig {
    pub enabled: bool,
    pub api_key: String,
    /// Organization admin key; only used to read the Costs API for the credit line.
    pub admin_key: String,
    pub top_ups: Vec<TopUp>,
    /// The RØDECaster source the transcriber listens to.
    pub source: RodecasterAudioSource,
    /// How far the translated text trails the speech.
    pub lag_ms: u64,
    /// Sets new YouTube broadcasts to HTTP-POST captions and posts each segment to them.
    pub youtube_live_captions: bool,
    /// Overrides the ingestion URL read from the YouTube stream.
    pub caption_ingestion_url: String,
    /// Captions every finished YouTube upload.
    pub auto_youtube_captions: bool,
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key: String::new(),
            admin_key: String::new(),
            top_ups: Vec::new(),
            source: RodecasterAudioSource::MainMix,
            lag_ms: 3_200,
            youtube_live_captions: false,
            caption_ingestion_url: String::new(),
            auto_youtube_captions: false,
        }
    }
}

impl ConnectorConfig for OpenAiConfig {
    fn is_configured(&self) -> bool {
        self.enabled && !self.api_key.is_empty()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TranscriptionStatus {
    NotConfigured,
    /// `reason` says why, e.g. `mixerOffline`.
    Unavailable,
    Ready,
    Connecting,
    Running,
    /// `reason` is `noCredit`, `invalidKey` or a message.
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LiveCaptionStatus {
    pub ok: bool,
    pub last_seq: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionState {
    pub status: TranscriptionStatus,
    pub reason: Option<String>,
    pub session_id: Option<Uuid>,
    pub event_id: Option<Uuid>,
    pub started_at: Option<DateTime<Utc>>,
    /// Present while live YouTube captions are being posted.
    pub youtube: Option<LiveCaptionStatus>,
}

impl TranscriptionState {
    #[must_use]
    pub fn idle(status: TranscriptionStatus, reason: Option<String>) -> Self {
        Self {
            status,
            reason,
            session_id: None,
            event_id: None,
            started_at: None,
            youtube: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptSegment {
    pub seq: u32,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub text: String,
}

/// Where a stored transcript's AI clean-up pass stands.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum CleanupStatus {
    #[default]
    None,
    Running,
    Succeeded,
    Failed,
}

impl CleanupStatus {
    /// The value kept in `transcript_sessions.cleanup_status`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    /// Reads the stored value; anything unknown is `None`.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "running" => Self::Running,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            _ => Self::None,
        }
    }
}

/// One stored live-transcript session, as the transcript library lists it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StoredTranscript {
    pub id: Uuid,
    pub event_id: Option<Uuid>,
    pub event_title: Option<String>,
    pub started_at: DateTime<Utc>,
    /// `None` while it runs, or when the server stopped without closing it.
    pub ended_at: Option<DateTime<Utc>>,
    /// `running`, `stopped`, `stoppedOffline` or `error`, as the session ended.
    pub status: String,
    /// The session the live transcript is writing to right now.
    pub live: bool,
    /// From the start to the end, or to the last line when it has no end.
    pub duration_seconds: f64,
    pub segment_count: u32,
    pub cleanup_status: CleanupStatus,
    pub cleanup_error: Option<String>,
    /// Live captions YouTube accepted, and those that failed or were dropped, in this session.
    pub captions_delivered: u32,
    pub captions_failed: u32,
    /// Why the latest failed caption was not delivered.
    pub caption_last_error: Option<String>,
    /// The YouTube stream the captions were last posted to; `None` for a pasted ingestion URL.
    pub caption_stream_id: Option<String>,
}

/// One committed line of a stored transcript, with its cleaned-up text when there is one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoredSegment {
    pub seq: u32,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub text: String,
    /// The AI clean-up's version of `text`; empty when it dropped the line as filler.
    pub clean_text: Option<String>,
}

/// A stored transcript with every line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptDetail {
    pub transcript: StoredTranscript,
    pub segments: Vec<StoredSegment>,
}

/// `transcript.cleanup.state`: a clean-up pass ran a step, finished or failed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CleanupProgress {
    pub session_id: Uuid,
    pub state: CleanupStatus,
    /// 0 to 1.
    pub progress: f64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CreditStatus {
    pub balance_usd: f64,
    pub topped_up_usd: f64,
    pub spent_usd: f64,
    pub as_of: DateTime<Utc>,
    pub low: bool,
}

impl CreditStatus {
    #[must_use]
    pub fn new(topped_up_usd: f64, spent_usd: f64, as_of: DateTime<Utc>) -> Self {
        let balance_usd = topped_up_usd - spent_usd;
        Self {
            balance_usd,
            topped_up_usd,
            spent_usd,
            as_of,
            low: balance_usd < LOW_CREDIT_USD,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_fill_missing_fields() {
        let config: OpenAiConfig =
            serde_json::from_str(r#"{"enabled":true,"apiKey":"sk"}"#).unwrap();
        assert!(config.is_configured());
        assert_eq!(config.lag_ms, 3_200);
        assert_eq!(config.source, RodecasterAudioSource::MainMix);
        assert!(!OpenAiConfig::default().is_configured());
    }

    #[test]
    fn credit_is_low_below_five_dollars() {
        let now = Utc::now();
        assert!(!CreditStatus::new(20.0, 15.0, now).low);
        let low = CreditStatus::new(20.0, 15.01, now);
        assert!(low.low);
        assert!((low.balance_usd - 4.99).abs() < 1e-9);
    }
}

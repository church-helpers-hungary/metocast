mod discovery;
use discovery::{DiscoveredDeviceRecord, DiscoveryTool};
use metocast_core::best_effort::LogGone;

use std::sync::{Arc, Mutex};

use metocast_client::{
    ClientConfig, ClientError, ConnectionState, MetocastClient, WebSocketConnection,
};
use metocast_core::{
    automations::{
        ActionOutcome, ActionResult, Automation, AutomationAction, AutomationDraft, AutomationList,
        AutomationRun, AutomationTrigger, ChannelSource, RecordTarget, RecorderRow, StopReason,
    },
    config::ConfigFieldKind,
    connectors::{
        AtemInput, AtemState, CameraDestination, ConnectorState, ConnectorStatus,
        DeskRecorderStatus, LastPreset, MiddlecontrolState, RecordStatus, RecorderStatus,
        RodecasterChannel, RodecasterDeskRecorder, RodecasterProfile, RodecasterRecorderState,
        StreamStatus,
    },
    events::{
        BibleReference, BibleVerse, CreateBibleReference, CreateConnection, CreateEvent, Event,
        EventConnection, EventSummary, OrderItem, OrderItemKind,
    },
    media::{
        AsShot, AudioInfo, AudioProbe, AudioProgress, AudioReport, AudioRequest, AudioSettings,
        AudioSource, AudioStep, ClockEstimate, CropRect, LutMode, MediaKind, MediaProbe,
        OutputAspect, PictureMode, PortraitRequest, PreviewFrame, PreviewRequest, ProgramProbe,
        ProgramProgress, ProgramReport, ProgramRequest, ProgramStep, RawOptions, RenderProgress,
        RenderRequest, RenderSettings, RenderState, SubtitleReport, SubtitleSource, SyncMethod,
        SyncReport, SyncWindow, ValueRange, VideoSound,
    },
    operations::{
        BroadlinkCommand, CronJob, CronJobDraft, DeviceListener, QueueJob, QueueSummary,
        UntrackedRecording,
    },
    protocol::{
        ConnectedClient, DeviceAlertCommand, ParagraphContent, PresentationCommand,
        PresentationStatus, PresenterCommand, PresenterState, PresenterTheme, ProductionCommand,
        ServerEvent, SlideContent, SvgSlideContent,
    },
    recordings::Recording,
    slides::{CreateSongSlides, DeckMatch, HighlightPart, PptFile, SongSlides, WelcomeSettings},
    transcription::{CreditStatus, StoredTranscript, TranscriptionState, TranscriptionStatus},
};
use serde::Serialize;

uniffi::setup_scaffolding!();

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum AppleError {
    #[error("invalid client configuration")]
    InvalidConfiguration,
    #[error("authentication was rejected")]
    Authentication,
    #[error("transport failed")]
    Transport,
    /// The server refused and said why, in its own words.
    #[error("{message}")]
    Server { message: String },
    /// The server refused without saying why, so only its status code is left to report.
    #[error("the server refused with HTTP {status}")]
    Refused { status: u16 },
    /// The server answered, but not with what the Metocast contract says it sends. Reporting
    /// this as a transport failure sends whoever reads it looking at the network instead.
    #[error("the answer did not match the Metocast contract")]
    Contract,
    #[error("identifier is invalid")]
    InvalidIdentifier,
    #[error("event connection is not running")]
    NotConnected,
    #[error("client runtime failed")]
    Runtime,
    #[error("input is invalid")]
    InvalidInput,
}

#[derive(Clone, uniffi::Record)]
pub struct EventSummaryRecord {
    pub id: String,
    pub title: String,
    pub computed_title: String,
    pub date_time: String,
    pub speaker: String,
    pub recording_count: i64,
    pub is_completed: bool,
    pub created_at: String,
    pub updated_at: String,
    pub connections: Vec<EventConnectionRecord>,
}

#[derive(Clone, uniffi::Record)]
pub struct EventConnectionRecord {
    pub platform: String,
    pub external_id: Option<String>,
    pub stream_url: Option<String>,
    pub event_url: Option<String>,
    pub schedule_status: String,
    pub privacy_status: Option<String>,
}

#[derive(Clone, uniffi::Record)]
pub struct BibleVerseRecord {
    pub chapter: i32,
    pub verse: i32,
    pub text: String,
}

#[derive(Clone, uniffi::Record)]
pub struct BibleReferenceRecord {
    pub reference_type: String,
    pub reference: String,
    pub translation: String,
    pub verses: Vec<BibleVerseRecord>,
}

#[derive(Clone, uniffi::Record)]
pub struct EventRecord {
    pub id: String,
    pub title: String,
    pub computed_title: String,
    pub date_time: String,
    pub speaker: String,
    pub description: String,
    pub auto_upload_enabled: bool,
    pub created_at: String,
    pub updated_at: String,
    pub connections: Vec<EventConnectionRecord>,
    pub bible_references: Vec<BibleReferenceRecord>,
}

/// What the event editor saves. An empty `reference` removes that Bible reference.
#[derive(Clone, uniffi::Record)]
pub struct EventDraftRecord {
    pub title: String,
    pub computed_title: String,
    /// RFC 3339.
    pub date_time: String,
    pub speaker: String,
    pub description: String,
    pub auto_upload_enabled: bool,
    pub youtube_privacy: String,
    pub bible_references: Vec<BibleReferenceRecord>,
}

#[derive(Clone, uniffi::Enum)]
pub enum ConnectorStatusCode {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

#[derive(Clone, uniffi::Record)]
pub struct ConnectorStatusRecord {
    pub connector: String,
    pub status: ConnectorStatusCode,
    pub message: Option<String>,
}

#[derive(Clone, uniffi::Record)]
pub struct ParagraphRecord {
    pub lines: Vec<String>,
    pub align: String,
    pub font_size_pt: f32,
}

#[derive(Clone, uniffi::Record)]
pub struct SlideRecord {
    pub index: u32,
    pub paragraphs: Vec<ParagraphRecord>,
}

#[derive(Clone, uniffi::Record)]
pub struct SvgSlideRecord {
    pub index: u32,
    pub svg: String,
    pub width_px: u32,
    pub height_px: u32,
}

#[derive(Clone, uniffi::Record)]
pub struct PresenterStateRecord {
    pub loaded: bool,
    pub file_path: Option<String>,
    pub current_slide: u32,
    pub total_slides: u32,
    pub render_mode: String,
    pub slides: Vec<SlideRecord>,
    pub svg_slides: Vec<SvgSlideRecord>,
    pub muted: bool,
    pub slide_width_emu: u64,
    pub slide_height_emu: u64,
}

#[derive(Clone, uniffi::Enum)]
pub enum PresenterControl {
    Status,
    Next,
    Previous,
    First,
    Last,
    GoTo { slide: u32 },
    Mute,
    Unmute,
}

// Live production state is plain data, so Swift gets the core types as they are.

#[uniffi::remote(Enum)]
pub enum StreamStatus {
    Idle,
    Connecting,
    Streaming,
    Stopping,
}

#[uniffi::remote(Enum)]
pub enum RecordStatus {
    Idle,
    Recording,
    Stopping,
}

#[uniffi::remote(Record)]
pub struct AtemInput {
    pub id: u16,
    pub name: String,
    pub short_name: String,
}

#[uniffi::remote(Record)]
pub struct AtemState {
    pub product: String,
    pub program: Option<u16>,
    pub preview: Option<u16>,
    pub inputs: Vec<AtemInput>,
    pub streaming: Option<StreamStatus>,
    pub recording: Option<RecordStatus>,
    pub stream_service: Option<String>,
}

#[uniffi::remote(Record)]
pub struct CameraDestination {
    /// `youtube`, `facebook`, `discord`, `middlecontrol` or `other`.
    pub service: String,
    /// Scheme and host only; never the stream key.
    pub url: String,
    pub broadcast: Option<String>,
    pub matches_youtube: Option<bool>,
}

#[uniffi::remote(Record)]
pub struct LastPreset {
    pub preset: u8,
    pub camera_id: u8,
    /// RFC 3339.
    pub at: String,
}

#[uniffi::remote(Record)]
pub struct MiddlecontrolState {
    pub selected_camera: Option<u8>,
    pub recording: Option<bool>,
    pub recording_camera_ids: Option<Vec<u8>>,
    pub connected_camera_ids: Option<Vec<u8>>,
    pub connected_apcr_ids: Option<Vec<u8>>,
    pub preset_move_active: Option<bool>,
    /// The preset (1–12) moving the selected camera, `None` at rest.
    pub running_preset: Option<u8>,
    pub last_preset: Option<LastPreset>,
}

/// The RØDECaster's own SD/USB recorder, apart from Metocast's audio recording.
#[uniffi::remote(Enum)]
pub enum DeskRecorderStatus {
    Stopped,
    Paused,
    Recording,
}

#[uniffi::remote(Record)]
pub struct RodecasterDeskRecorder {
    pub status: DeskRecorderStatus,
    /// An SD card or USB drive is there to write to.
    pub media_present: bool,
}

#[uniffi::remote(Record)]
pub struct RodecasterChannel {
    pub channel: u32,
    pub label: String,
    pub mute: bool,
    pub wireless_mute: bool,
}

#[uniffi::remote(Record)]
pub struct RodecasterProfile {
    pub model: String,
    pub channels: Vec<RodecasterChannel>,
}

#[uniffi::remote(Enum)]
pub enum RecorderStatus {
    Idle,
    Recording,
    Failed,
}

#[uniffi::remote(Enum)]
pub enum TranscriptionStatus {
    NotConfigured,
    Unavailable,
    Ready,
    Connecting,
    Running,
    Error,
}

/// The live English transcript.
#[derive(Clone, uniffi::Record)]
pub struct TranscriptionRecord {
    pub status: TranscriptionStatus,
    /// `mixerOffline` while unavailable; `noCredit`, `invalidKey` or a message on error.
    pub reason: Option<String>,
    pub event_id: Option<String>,
    /// RFC 3339.
    pub started_at: Option<String>,
    /// Why live YouTube captions are failing, while they are.
    pub youtube_error: Option<String>,
}

/// One stored live transcript in the library.
#[derive(Clone, uniffi::Record)]
pub struct StoredTranscriptRecord {
    pub id: String,
    pub event_title: Option<String>,
    /// RFC 3339.
    pub started_at: String,
    pub duration_seconds: f64,
    pub segment_count: u32,
    /// Being written by the live transcript now: it cannot be deleted or cleaned up.
    pub live: bool,
    /// `none`, `running`, `succeeded` or `failed`.
    pub cleanup_status: String,
    pub cleanup_error: Option<String>,
}

/// One line of a stored transcript.
#[derive(Clone, uniffi::Record)]
pub struct TranscriptLineRecord {
    pub seq: u32,
    pub text: String,
    /// The AI clean-up's version; empty when it dropped the line as filler.
    pub clean_text: Option<String>,
}

#[derive(Clone, uniffi::Record)]
pub struct TranscriptDetailRecord {
    pub transcript: StoredTranscriptRecord,
    pub lines: Vec<TranscriptLineRecord>,
}

/// Prepaid OpenAI credit left.
#[derive(Clone, uniffi::Record)]
pub struct CreditRecord {
    pub balance_usd: f64,
    /// Below $5.
    pub low: bool,
    /// RFC 3339.
    pub as_of: String,
}

#[derive(Clone, uniffi::Record)]
pub struct RecorderStateRecord {
    pub status: RecorderStatus,
    pub event_id: Option<String>,
    /// RFC 3339.
    pub started_at: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, uniffi::Enum)]
pub enum ProductionControl {
    ObsStreaming {
        on: bool,
    },
    ObsRecording {
        on: bool,
    },
    AtemProgram {
        input: u16,
    },
    AtemPreview {
        input: u16,
    },
    AtemCut,
    AtemAuto,
    AtemStreaming {
        on: bool,
    },
    AtemRecording {
        on: bool,
    },
    MiddlecontrolCamera {
        camera_id: u8,
    },
    MiddlecontrolRecordingAll {
        on: bool,
    },
    /// Answered by `AppleEvent::RodecasterProfile`.
    RodecasterProfile,
    RodecasterMute {
        channel: u32,
        mute: bool,
    },
    /// `None` keeps the recording as untracked files, to assign to an event later.
    RodecasterRecordStart {
        event_id: Option<String>,
    },
    RodecasterRecordStop,
    /// The desk's own SD/USB recorder, not Metocast's audio recording.
    RodecasterDeskRecording {
        on: bool,
    },
    /// `None` transcribes with no event.
    TranscriptionStart {
        event_id: Option<String>,
    },
    TranscriptionStop,
    CameraRecording {
        on: bool,
    },
    CameraStreaming {
        on: bool,
    },
    /// Copies the YouTube ingestion address and key into the camera, without going live.
    CameraPushYoutube,
    /// The same for the ATEM.
    AtemPushYoutube,
    /// Every recorder the Record button is set to; the results arrive as `RecordersState`.
    Recorders {
        on: bool,
    },
}

/// The OBS sources the server watches. Ids cross the bridge as strings.
#[derive(Clone, uniffi::Enum)]
pub enum DeviceAlertControl {
    List,
    Scan,
    Available,
    Create {
        category: String,
        device_item_value: String,
        device_item_name: String,
        friendly_name: String,
    },
    Delete {
        id: String,
    },
}

#[derive(Clone, uniffi::Record)]
pub struct DeviceListenerRecord {
    pub id: String,
    pub category: String,
    pub device_item_name: String,
    pub friendly_name: String,
}

// Slides and the presentation backend: the core types as they are.

#[uniffi::remote(Record)]
pub struct PptFile {
    pub id: String,
    pub name: String,
    pub path: String,
    pub folder_id: String,
    pub title: String,
    pub number: Option<i32>,
    pub matched_in: Option<DeckMatch>,
    pub title_parts: Vec<HighlightPart>,
    pub snippet_parts: Vec<HighlightPart>,
}

#[uniffi::remote(Enum)]
pub enum DeckMatch {
    Number,
    Title,
    Text,
}

#[uniffi::remote(Record)]
pub struct HighlightPart {
    pub text: String,
    pub hit: bool,
}

#[uniffi::remote(Enum)]
pub enum OrderItemKind {
    Song,
    Bible,
    Block,
    Announcements,
    Text,
}

/// The welcome screen's settings; `has_logo` and `replaced_photos` are read-only.
#[derive(Clone, uniffi::Record)]
pub struct WelcomeSettingsRecord {
    pub title: String,
    pub subtitle: String,
    pub church_name: String,
    pub language: String,
    pub has_logo: bool,
    pub replaced_photos: Vec<u32>,
}

impl From<WelcomeSettings> for WelcomeSettingsRecord {
    fn from(s: WelcomeSettings) -> Self {
        Self {
            title: s.title,
            subtitle: s.subtitle,
            church_name: s.church_name,
            language: s.language,
            has_logo: s.has_logo,
            replaced_photos: s.replaced_photos,
        }
    }
}

/// One step of an event's Order of Service. `id` is absent for a step added in this edit.
#[derive(Clone, uniffi::Record)]
pub struct OrderItemRecord {
    pub id: Option<String>,
    pub kind: OrderItemKind,
    pub title: String,
    pub deck_path: Option<String>,
    pub bible_type: Option<String>,
    pub reference: Option<String>,
    pub translation: Option<String>,
    pub notes: String,
}

impl From<OrderItem> for OrderItemRecord {
    fn from(item: OrderItem) -> Self {
        Self {
            id: item.id.map(|id| id.to_string()),
            kind: item.kind,
            title: item.title,
            deck_path: item.deck_path,
            bible_type: item.bible_type,
            reference: item.reference,
            translation: item.translation,
            notes: item.notes,
        }
    }
}

#[derive(Clone, uniffi::Record)]
pub struct PptFolderRecord {
    pub id: String,
    pub name: String,
    pub path: String,
}

#[uniffi::remote(Record)]
pub struct SongSlides {
    pub file_path: String,
    pub slide_count: u32,
}

#[uniffi::remote(Enum)]
pub enum PresenterTheme {
    Classic,
    Editorial,
}

#[uniffi::remote(Record)]
pub struct PresentationStatus {
    pub app_running: bool,
    pub slideshow_active: bool,
    pub current_slide: Option<u32>,
    pub total_slides: Option<u32>,
    pub document_name: Option<String>,
    pub blanked: bool,
}

#[uniffi::remote(Enum)]
pub enum PresentationCommand {
    Open {
        file_path: String,
    },
    Close,
    Start,
    Stop,
    GetSettings,
    Status,
    SetUseWebPresenter {
        enabled: bool,
    },
    SetPresenterTheme {
        theme: PresenterTheme,
    },
    SetDeliveryLog {
        enabled: bool,
    },
    Next,
    Previous,
    First,
    Last,
    GoTo {
        slide: u32,
    },
    Mute,
    Unmute,
    OrderGo {
        event_id: String,
        item_id: String,
    },
    OrderNext {
        event_id: Option<String>,
    },
    OrderPrevious {
        event_id: Option<String>,
    },
    Register {
        label: String,
        hostname: Option<String>,
    },
    Ack {
        slide: u32,
        shown_at: String,
    },
    ClientsList,
    ClientsPing {
        client_id: String,
    },
}

#[derive(Clone, uniffi::Record)]
pub struct CronJobRecord {
    pub id: String,
    pub name: String,
    pub cron_expression: String,
    pub enabled: bool,
    pub pull_youtube: bool,
    pub auto_upload: bool,
}

// Automations: rules that run inside the core. The triggers, actions and results are plain data,
// so Swift gets the core types as they are; a rule's id and a run's times cross as strings.

#[uniffi::remote(Enum)]
pub enum ChannelSource {
    Either,
    Desk,
    Transmitter,
}

#[uniffi::remote(Enum)]
pub enum AutomationTrigger {
    MiddlecontrolPreset {
        preset: u8,
        camera_id: Option<u8>,
    },
    RodecasterChannel {
        channel: u32,
        label: String,
        live: bool,
        source: ChannelSource,
        hold_seconds: f32,
    },
}

#[uniffi::remote(Enum)]
pub enum RecordTarget {
    Obs,
    Atem,
    BlackmagicCamera,
    Middlecontrol { camera_id: Option<u8> },
    Rodecaster,
    RodecasterAudio,
}

#[uniffi::remote(Enum)]
pub enum StopReason {
    Metocast,
    Disconnected,
    Device,
}

/// One recorder as the Record button shows it. `last_start_at` (RFC 3339) and
/// `last_start_error` are the last start by the Record button or an automation.
#[derive(Clone, uniffi::Record)]
pub struct RecorderRecord {
    pub target: RecordTarget,
    pub selected: bool,
    pub available: bool,
    pub recording: bool,
    pub last_start_at: Option<String>,
    pub last_start_error: Option<String>,
}

impl From<RecorderRow> for RecorderRecord {
    fn from(row: RecorderRow) -> Self {
        Self {
            target: row.target,
            selected: row.selected,
            available: row.available,
            recording: row.recording,
            last_start_at: row.last_start.as_ref().map(|start| start.at.to_rfc3339()),
            last_start_error: row.last_start.and_then(|start| start.error),
        }
    }
}

#[uniffi::remote(Enum)]
pub enum AutomationAction {
    RecordStart {
        target: RecordTarget,
    },
    RecordStop {
        target: RecordTarget,
    },
    PresetRecall {
        preset: u8,
        camera_id: Option<u8>,
        wait_for_arrival: bool,
    },
}

#[uniffi::remote(Enum)]
pub enum ActionOutcome {
    Done,
    Skipped,
    Failed,
}

#[uniffi::remote(Record)]
pub struct ActionResult {
    pub action: String,
    pub outcome: ActionOutcome,
    pub message: Option<String>,
}

/// A rule. `id` is empty for one that isn't saved yet.
#[derive(Clone, uniffi::Record)]
pub struct AutomationRecord {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub triggers: Vec<AutomationTrigger>,
    pub actions: Vec<AutomationAction>,
    pub cooldown_seconds: f32,
    pub require_event: bool,
}

#[derive(Clone, uniffi::Record)]
pub struct AutomationListRecord {
    /// The master switch: nothing fires while it is off.
    pub armed: bool,
    pub automations: Vec<AutomationRecord>,
}

#[derive(Clone, uniffi::Record)]
pub struct AutomationRunRecord {
    pub id: String,
    pub automation_id: String,
    pub name: String,
    /// What fired it, in words.
    pub cause: String,
    /// "Run now", which ignores the cooldown, the event check and the master switch.
    pub manual: bool,
    /// RFC 3339.
    pub started_at: String,
    pub results: Vec<ActionResult>,
}

#[derive(Clone, uniffi::Record)]
pub struct QueueSummaryRecord {
    pub queue: String,
    pub pending: i64,
    pub processing: i64,
    pub succeeded: i64,
    pub dead: i64,
}

#[derive(Clone, uniffi::Record)]
pub struct QueueJobRecord {
    pub id: String,
    pub queue: String,
    pub job_type: String,
    pub status: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub last_error: Option<String>,
    /// RFC 3339.
    pub updated_at: String,
}

#[derive(Clone, uniffi::Record)]
pub struct BroadlinkCommandRecord {
    pub id: String,
    pub name: String,
    pub category: String,
}

// Video processing: the media pipeline's records, mostly the core types as they are.

#[uniffi::remote(Enum)]
pub enum LutMode {
    Embedded,
    File,
    Sidecar,
    None,
}

#[uniffi::remote(Enum)]
pub enum OutputAspect {
    Widescreen,
    Native,
}

#[uniffi::remote(Record)]
pub struct PortraitRequest {
    pub shift_x: i32,
}

#[uniffi::remote(Enum)]
pub enum MediaKind {
    Braw,
    Video,
}

#[uniffi::remote(Enum)]
pub enum RenderState {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[uniffi::remote(Record)]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[uniffi::remote(Record)]
pub struct AudioInfo {
    pub codec: String,
    pub channels: u32,
    pub sample_rate: u32,
    pub bits: Option<u32>,
}

#[uniffi::remote(Record)]
pub struct RenderSettings {
    pub gamma: String,
    pub gamut: String,
    pub iso: Option<u32>,
    pub kelvin: Option<u32>,
    pub tint: Option<i32>,
    pub exposure: Option<f32>,
    pub highlight_recovery: bool,
    pub gamut_compression: bool,
    pub lut_mode: LutMode,
    pub lut_path: Option<String>,
    pub aspect: OutputAspect,
    pub crop_shift_y: i32,
    pub bitrate_mbps: u32,
    pub audio_channels: Vec<u32>,
    pub sidecar_path: Option<String>,
}

#[uniffi::remote(Record)]
pub struct ValueRange {
    pub min: f64,
    pub max: f64,
}

#[uniffi::remote(Record)]
pub struct AsShot {
    pub iso: Option<u32>,
    pub kelvin: Option<u32>,
    pub tint: Option<i32>,
    pub exposure: Option<f32>,
}

#[uniffi::remote(Record)]
pub struct RawOptions {
    pub gammas: Vec<String>,
    pub gamuts: Vec<String>,
    pub isos: Vec<u32>,
    pub kelvin: Option<ValueRange>,
    pub tint: Option<ValueRange>,
    pub exposure: Option<ValueRange>,
    pub as_shot: AsShot,
}

/// One frame as a base64 JPEG, and the crop a render keeps, in source pixels.
#[uniffi::remote(Record)]
pub struct PreviewFrame {
    pub image_jpeg: String,
    pub active: CropRect,
    pub output: CropRect,
}

#[uniffi::remote(Record)]
pub struct SubtitleSource {
    pub path: String,
    pub language: String,
}

#[uniffi::remote(Record)]
pub struct SubtitleReport {
    pub language: String,
    pub cues: u32,
    pub shift_seconds: f64,
    pub confident: bool,
    pub srt_path: String,
    pub on_speech: bool,
    pub matched: u32,
}

/// A video file on the server's disk, and the render the stored settings would make of it.
#[derive(Clone, uniffi::Record)]
pub struct MediaProbeRecord {
    pub path: String,
    pub file_name: String,
    pub file_size: u64,
    pub kind: MediaKind,
    pub camera: Option<String>,
    pub clip_name: Option<String>,
    pub width: u32,
    pub height: u32,
    pub active_area: CropRect,
    pub fps_num: u32,
    pub fps_den: u32,
    pub frame_count: u64,
    pub duration_seconds: f64,
    pub start_timecode: Option<String>,
    /// RFC 3339, by the camera's clock.
    pub recorded_at: Option<String>,
    /// RFC 3339, by the server's clock.
    pub host_record_started_at: Option<String>,
    pub audio: Option<AudioInfo>,
    pub gamma: Option<String>,
    pub gamut: Option<String>,
    pub embedded_lut: Option<String>,
    pub defaults: RenderSettings,
    pub output: CropRect,
    /// Tools the server lacks for this file: `ffmpeg`, `braw-decode`, `blackmagic-raw-sdk`.
    pub missing: Vec<String>,
    /// Blackmagic RAW only: what its settings may be set to.
    pub raw_options: Option<RawOptions>,
}

#[derive(Clone, uniffi::Record)]
pub struct RenderProgressRecord {
    pub job_id: String,
    pub state: RenderState,
    pub frame: u64,
    pub total_frames: u64,
    pub fps: f64,
    pub output_path: String,
    pub error: Option<String>,
}

#[uniffi::remote(Enum)]
pub enum AudioStep {
    Download,
    Prepare,
    Clean,
    Align,
    Finish,
}

#[uniffi::remote(Record)]
pub struct AudioSettings {
    pub channels: Vec<u32>,
    pub target_lufs: f32,
}

#[uniffi::remote(Record)]
pub struct AudioReport {
    pub input_lufs: f64,
    pub output_lufs: f64,
    pub output_true_peak_dbtp: f64,
    pub removed_delay_samples: i64,
    pub alignment_confidence: f64,
}

#[derive(Clone, uniffi::Record)]
pub struct AudioSourceRecord {
    pub path: String,
    pub file_name: String,
    pub file_size: u64,
    pub audio: AudioInfo,
    pub duration_seconds: f64,
    /// RFC 3339, by the recorder's clock.
    pub recorded_at: Option<String>,
}

/// Audio files on the server's disk, joined in order, and what processing them needs.
#[derive(Clone, uniffi::Record)]
pub struct AudioProbeRecord {
    pub sources: Vec<AudioSourceRecord>,
    pub duration_seconds: f64,
    pub channels: u32,
    /// RFC 3339, by the recorder's clock.
    pub recorded_at: Option<String>,
    pub defaults: AudioSettings,
    /// Tools the server lacks and cannot fetch.
    pub missing: Vec<String>,
    /// Tools the first job downloads.
    pub downloads: Vec<String>,
}

#[derive(Clone, uniffi::Record)]
pub struct AudioProgressRecord {
    pub job_id: String,
    pub state: RenderState,
    pub step: Option<AudioStep>,
    pub fraction: Option<f64>,
    pub output_path: String,
    pub error: Option<String>,
    pub report: Option<AudioReport>,
}

#[derive(Clone, uniffi::Record)]
pub struct RenderQueuedRecord {
    pub job_id: String,
    pub output_path: String,
}

#[uniffi::remote(Enum)]
pub enum PictureMode {
    Grade,
    Copy,
}

#[uniffi::remote(Enum)]
pub enum SyncMethod {
    None,
    Waveform,
    Lips,
    Clock,
    Manual,
}

#[uniffi::remote(Enum)]
pub enum ProgramStep {
    Download,
    Picture,
    Prepare,
    Sync,
    Clean,
    Merge,
    Portrait,
    Captions,
}

#[uniffi::remote(Record)]
pub struct VideoSound {
    pub has_track: bool,
    pub usable: bool,
    pub level_dbfs: Option<f64>,
}

#[uniffi::remote(Record)]
pub struct ClockEstimate {
    pub offset_seconds: f64,
    pub learned: bool,
}

#[uniffi::remote(Record)]
pub struct SyncWindow {
    pub video_seconds: f64,
    pub offset_seconds: f64,
    pub score: f64,
    pub agrees: bool,
}

#[uniffi::remote(Record)]
pub struct SyncReport {
    pub method: SyncMethod,
    pub offset_seconds: f64,
    pub drift_ppm: f64,
    pub confidence: f64,
    pub confident: bool,
    pub peak_sigma: Option<f64>,
    pub windows: Vec<SyncWindow>,
    pub clock_difference_seconds: Option<f64>,
    pub clock_offset_seconds: Option<f64>,
    pub manual_frames: i32,
    pub start_file: Option<String>,
    pub start_file_seconds: Option<f64>,
}

#[uniffi::remote(Record)]
pub struct ProgramReport {
    pub sync: SyncReport,
    pub audio: AudioReport,
    pub audio_seconds: f64,
    pub coverage: f64,
    pub silent_start_seconds: f64,
    pub silent_end_seconds: f64,
    pub damaged_frames: u64,
    pub audio_output_path: String,
    pub subtitles: Vec<SubtitleReport>,
}

/// A program to queue. Without an event it is listed with the untracked recordings;
/// `start_seconds` skips the search when the operator knows where the video starts in the
/// recording; `subtitles` are moved onto the voice and sent with the uploads.
#[derive(Clone, uniffi::Record)]
pub struct ProgramRequestRecord {
    pub video_path: String,
    pub audio_paths: Vec<String>,
    pub event_id: Option<String>,
    pub picture: Option<PictureMode>,
    pub settings: Option<RenderSettings>,
    pub audio_settings: Option<AudioSettings>,
    pub start_seconds: Option<f64>,
    pub subtitles: Vec<SubtitleSource>,
    /// Also makes a 9:16 copy with the same sound.
    pub portrait: Option<PortraitRequest>,
}

/// A video and its audio on the server's disk, and how a program job would put them together.
#[derive(Clone, uniffi::Record)]
pub struct ProgramProbeRecord {
    pub video: MediaProbeRecord,
    /// `None` when the video's own sound is used.
    pub audio: Option<AudioProbeRecord>,
    pub video_sound: VideoSound,
    /// `none`, `waveform` or `lips`.
    pub sync_method: SyncMethod,
    pub clock: Option<ClockEstimate>,
    pub picture: PictureMode,
    pub warnings: Vec<String>,
    /// Why the job can't run, when it can't.
    pub blocker: Option<String>,
    pub missing: Vec<String>,
    pub downloads: Vec<String>,
}

#[derive(Clone, uniffi::Record)]
pub struct ProgramProgressRecord {
    pub job_id: String,
    pub state: RenderState,
    pub step: Option<ProgramStep>,
    pub fraction: Option<f64>,
    pub output_path: String,
    pub error: Option<String>,
    pub report: Option<ProgramReport>,
}

#[derive(Clone, uniffi::Record)]
pub struct UntrackedRecordingRecord {
    pub id: String,
    pub file_name: String,
    pub file_size: i64,
    pub duration_seconds: f64,
    /// RFC 3339.
    pub detected_at: String,
}

#[derive(Clone, uniffi::Record)]
pub struct BibleSuggestionRecord {
    pub label: String,
    pub category: String,
}

/// One recorded file for an event, as the event screen lists it.
#[derive(Clone, uniffi::Record)]
pub struct RecordingRecord {
    pub id: String,
    pub file_name: String,
    pub file_size: i64,
    pub duration_seconds: f64,
    pub source: String,
    /// RFC 3339.
    pub detected_at: String,
    pub uploadable: bool,
    pub uploaded: bool,
    pub video_url: Option<String>,
    /// "youtube: uploading 12%", one per platform that has started.
    pub upload_states: Vec<String>,
}

/// The part of `active` a portrait copy keeps, for the viewer's frame.
#[uniffi::export]
pub fn portrait_frame(active: CropRect, shift_x: i32) -> CropRect {
    metocast_core::media::portrait_crop(active, shift_x)
}

/// What a portrait copy of `file_name` is called, to find its recording.
#[uniffi::export]
pub fn portrait_name(file_name: String) -> String {
    metocast_core::media::portrait_file_name(&file_name)
}

/// A recording's `source` when it is a portrait copy.
#[uniffi::export]
pub fn is_portrait_source(source: String) -> bool {
    source == metocast_core::media::PORTRAIT_SOURCE
}

#[derive(Clone, uniffi::Record)]
pub struct ServerSettingsRecord {
    pub title_template: String,
    /// Where `Generate Slides` writes an event's Bible decks.
    pub slide_folder: String,
    pub song_slide_folder: String,
}

#[derive(Clone, uniffi::Record)]
pub struct ConnectorNameRecord {
    pub id: String,
    pub name: String,
}

#[uniffi::remote(Enum)]
pub enum ConfigFieldKind {
    Toggle,
    Text,
    Number,
    Secret,
}

#[derive(Clone, uniffi::Record)]
pub struct ConfigFieldRecord {
    pub key: String,
    pub label: String,
    pub kind: ConfigFieldKind,
    pub value: String,
    /// A secret is stored, though its value never leaves the server.
    pub is_set: bool,
}

#[derive(Clone, uniffi::Record)]
pub struct ConfigValueRecord {
    pub key: String,
    pub value: String,
    /// Forget the stored secret instead of keeping it.
    pub clear: bool,
}

/// One device on the server's WebSocket, as the connected-devices list shows it.
#[derive(Clone, uniffi::Record)]
pub struct ConnectedClientRecord {
    pub id: String,
    pub label: String,
    pub hostname: Option<String>,
    pub user_agent: Option<String>,
    /// RFC 3339.
    pub connected_at: String,
    /// RFC 3339.
    pub latency_ms: Option<i64>,
}

#[derive(Clone, uniffi::Enum)]
pub enum AppleConnectionState {
    Connecting,
    Connected,
    Reconnecting,
    AuthenticationFailed,
    Closed,
}

#[derive(Clone, uniffi::Enum)]
pub enum AppleEvent {
    Discovery {
        tool: DiscoveryTool,
        devices: Vec<DiscoveredDeviceRecord>,
        message: Option<String>,
    },
    Connected {
        server_id: String,
    },
    ConnectorStatus {
        status: ConnectorStatusRecord,
    },
    EventsList {
        events: Vec<EventSummaryRecord>,
    },
    Event {
        event: EventRecord,
    },
    PresenterState {
        state: PresenterStateRecord,
    },
    ConnectorStatuses {
        statuses: Vec<ConnectorStatusRecord>,
    },
    PresenterSlideChanged {
        current_slide: u32,
        total_slides: u32,
    },
    EventChanged,
    /// The Order of Service step on screen; both ids are absent when none is.
    OrderState {
        event_id: Option<String>,
        item_id: Option<String>,
    },
    /// An event's Order of Service was replaced; reload it.
    EventOrderChanged {
        event_id: String,
    },
    Notification {
        level: String,
        message: String,
    },
    ObsState {
        streaming: bool,
        recording: bool,
    },
    AtemState {
        state: Option<AtemState>,
    },
    MiddlecontrolState {
        state: Option<MiddlecontrolState>,
    },
    RodecasterProfile {
        profile: RodecasterProfile,
    },
    /// A desk mute, or a Wireless PRO transmitter mute when `remote`.
    RodecasterMute {
        channel: u32,
        label: String,
        muted: bool,
        remote: bool,
        notify: bool,
    },
    RodecasterRecorder {
        state: RecorderStateRecord,
    },
    /// The desk's own SD/USB recorder started, paused or stopped, or a card or drive came or went.
    RodecasterDeskRecorder {
        state: RodecasterDeskRecorder,
    },
    /// The rules or the master switch changed; also sent on connect.
    AutomationsChanged {
        armed: bool,
        automations: Vec<AutomationRecord>,
    },
    /// A rule ran.
    AutomationRun {
        run: AutomationRunRecord,
    },
    /// Every recorder the Record button can drive.
    RecordersState {
        recorders: Vec<RecorderRecord>,
    },
    /// A recording stopped, for whatever reason.
    RecorderStopped {
        target: RecordTarget,
        reason: StopReason,
        error: Option<String>,
    },
    /// A command this client sent failed: a stable code such as `atem_not_connected`, or the
    /// device's own words.
    CommandFailed {
        message: String,
    },
    PresentationSettings {
        use_web_presenter: bool,
        theme: PresenterTheme,
        delivery_log: bool,
    },
    PresentationStatus {
        status: PresentationStatus,
    },
    Clients {
        clients: Vec<ConnectedClientRecord>,
    },
    CameraState {
        streaming: bool,
        recording: bool,
        /// The camera's own word for what streaming is doing.
        stream_status: String,
        /// False with no card or SSD in the camera.
        has_media: bool,
        /// Where the livestream points; `None` from an older server or a camera without one.
        destination: Option<CameraDestination>,
    },
    DeviceAlerts {
        listeners: Vec<DeviceListenerRecord>,
    },
    MediaRenderProgress {
        progress: RenderProgressRecord,
    },
    MediaAudioProgress {
        progress: AudioProgressRecord,
    },
    MediaProgramProgress {
        progress: ProgramProgressRecord,
    },
    Transcription {
        state: TranscriptionRecord,
    },
    /// Sent on connect: replaces the state, the line and the credit.
    TranscriptionSnapshot {
        state: TranscriptionRecord,
        line: String,
        credit: Option<CreditRecord>,
    },
    /// The newest English line, open or committed.
    TranscriptLine {
        text: String,
    },
    TranscriptionCredit {
        credit: CreditRecord,
    },
    /// The server is parsing `pending` decks into the slide search index; 0 when done.
    SlideIndexState {
        pending: u32,
    },
    /// A stored transcript's AI clean-up ran a step, finished or failed.
    TranscriptCleanup {
        session_id: String,
        /// `running`, `succeeded` or `failed`.
        state: String,
        progress: f64,
        error: Option<String>,
    },
    Unknown,
}

#[uniffi::export(foreign)]
pub trait EventListener: Send + Sync {
    fn on_state_changed(&self, state: AppleConnectionState);
    fn on_event(&self, event: AppleEvent);
}

#[derive(uniffi::Object)]
pub struct AppleClient {
    runtime: Arc<tokio::runtime::Runtime>,
    client: MetocastClient,
    connection: Arc<Mutex<Option<WebSocketConnection>>>,
    event_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[uniffi::export]
impl AppleClient {
    #[uniffi::constructor]
    pub fn new(base_url: String, auth_token: String) -> Result<Arc<Self>, AppleError> {
        let config = ClientConfig::new(&base_url, auth_token)
            .map_err(|_| AppleError::InvalidConfiguration)?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|_| AppleError::Runtime)?;
        Ok(Arc::new(Self {
            runtime: Arc::new(runtime),
            client: MetocastClient::new(config),
            connection: Arc::new(Mutex::new(None)),
            event_task: Mutex::new(None),
        }))
    }

    pub async fn health(&self) -> Result<(), AppleError> {
        let client = self.client.clone();
        self.run(async move { client.health().await }).await
    }

    pub async fn list_events(&self) -> Result<Vec<EventSummaryRecord>, AppleError> {
        let client = self.client.clone();
        let events = self.run(async move { client.list_events().await }).await?;
        Ok(events.into_iter().map(Into::into).collect())
    }

    pub async fn event(&self, id: String) -> Result<EventRecord, AppleError> {
        let id = parse_id(&id)?;
        let client = self.client.clone();
        Ok(self
            .run(async move { client.event(id).await })
            .await?
            .into())
    }

    pub async fn connector_statuses(&self) -> Result<Vec<ConnectorStatusRecord>, AppleError> {
        let client = self.client.clone();
        let statuses = self
            .run(async move { client.connector_statuses().await })
            .await?;
        Ok(statuses
            .into_iter()
            .map(|(connector, status)| connector_status(connector, status))
            .collect())
    }

    pub async fn create_event(&self, draft: EventDraftRecord) -> Result<EventRecord, AppleError> {
        let event = CreateEvent::try_from(draft)?;
        let client = self.client.clone();
        Ok(self
            .run(async move { client.create_event(&event).await })
            .await?
            .into())
    }

    pub async fn update_event(
        &self,
        id: String,
        draft: EventDraftRecord,
    ) -> Result<EventRecord, AppleError> {
        let id = parse_id(&id)?;
        let event = CreateEvent::try_from(draft)?;
        let client = self.client.clone();
        Ok(self
            .run(async move { client.update_event(id, &event).await })
            .await?
            .into())
    }

    pub async fn delete_event(&self, id: String) -> Result<(), AppleError> {
        let id = parse_id(&id)?;
        let client = self.client.clone();
        self.run(async move { client.delete_event(id).await }).await
    }

    /// The template the published event title is rendered from.
    pub async fn title_template(&self) -> Result<String, AppleError> {
        let client = self.client.clone();
        Ok(self
            .run(async move { client.title_template().await })
            .await?
            .template)
    }

    pub async fn bible_verses(
        &self,
        reference: String,
        translation: String,
    ) -> Result<Vec<BibleVerseRecord>, AppleError> {
        let client = self.client.clone();
        let passage = self
            .run(async move { client.bible_passage(&reference, &translation).await })
            .await?;
        Ok(passage.verses.into_iter().map(Into::into).collect())
    }

    pub async fn start_events(&self, listener: Arc<dyn EventListener>) -> Result<(), AppleError> {
        self.shutdown_events();
        let client = self.client.clone();
        let stored_connection = Arc::clone(&self.connection);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let task = self.runtime.spawn(async move {
            let connection = client.connect_websocket();
            let mut events = connection.subscribe();
            let mut state = connection.state();
            if let Ok(mut slot) = stored_connection.lock() {
                *slot = Some(connection.clone());
            }
            ready_tx.send(()).log_gone();
            listener.on_state_changed(connection_state(*state.borrow()));
            loop {
                tokio::select! {
                    result = state.changed() => {
                        if result.is_err() { break; }
                        listener.on_state_changed(connection_state(*state.borrow()));
                        if *state.borrow() == ConnectionState::Closed { break; }
                    }
                    event = events.recv() => match event {
                        Ok(event) => listener.on_event(event.into()),
                        Err(broadcast_error) => {
                            if matches!(broadcast_error, tokio::sync::broadcast::error::RecvError::Closed) {
                                break;
                            }
                        }
                    }
                }
            }
        });
        *self.event_task.lock().map_err(|_| AppleError::Runtime)? = Some(task);
        ready_rx.await.map_err(|_| AppleError::Runtime)
    }

    pub async fn presenter_control(&self, control: PresenterControl) -> Result<(), AppleError> {
        self.send(PresenterCommand::from(control)).await
    }

    pub async fn production_control(&self, control: ProductionControl) -> Result<(), AppleError> {
        self.send(ProductionCommand::try_from(control)?).await
    }

    /// Manages the OBS sources the server watches and warns about.
    pub async fn device_alert_control(
        &self,
        command: DeviceAlertControl,
    ) -> Result<(), AppleError> {
        self.send(DeviceAlertCommand::try_from(command)?).await
    }

    pub async fn presentation_control(
        &self,
        command: PresentationCommand,
    ) -> Result<(), AppleError> {
        self.send(command).await
    }

    // ── Housekeeping ─────────────────────────────────────────────────────────

    pub async fn cron_jobs(&self) -> Result<Vec<CronJobRecord>, AppleError> {
        let client = self.client.clone();
        let jobs = self.run(async move { client.cron_jobs().await }).await?;
        Ok(jobs.into_iter().map(Into::into).collect())
    }

    /// Creates the job when `id` is empty, otherwise updates it.
    pub async fn save_cron_job(
        &self,
        id: String,
        job: CronJobRecord,
    ) -> Result<CronJobRecord, AppleError> {
        let draft = CronJobDraft {
            name: job.name,
            cron_expression: job.cron_expression,
            enabled: job.enabled,
            pull_youtube: job.pull_youtube,
            auto_upload: job.auto_upload,
        };
        if draft.name.trim().is_empty() || draft.cron_expression.trim().is_empty() {
            return Err(AppleError::InvalidInput);
        }
        let client = self.client.clone();
        let saved = if id.is_empty() {
            self.run(async move { client.create_cron_job(&draft).await })
                .await?
        } else {
            let id = parse_id(&id)?;
            self.run(async move { client.update_cron_job(id, &draft).await })
                .await?
        };
        Ok(saved.into())
    }

    pub async fn delete_cron_job(&self, id: String) -> Result<(), AppleError> {
        let id = parse_id(&id)?;
        let client = self.client.clone();
        self.run(async move { client.delete_cron_job(id).await })
            .await
    }

    // ── Automations ──────────────────────────────────────────────────────────

    pub async fn automations(&self) -> Result<AutomationListRecord, AppleError> {
        let client = self.client.clone();
        let list = self.run(async move { client.automations().await }).await?;
        Ok(list.into())
    }

    /// Creates the rule when its `id` is empty, otherwise replaces it. A rule that breaks a limit
    /// comes back as `AppleError::Server` with the server's own words.
    pub async fn save_automation(
        &self,
        automation: AutomationRecord,
    ) -> Result<AutomationRecord, AppleError> {
        let id = if automation.id.is_empty() {
            None
        } else {
            Some(parse_id(&automation.id)?)
        };
        let draft = AutomationDraft::from(automation);
        let client = self.client.clone();
        let saved = match id {
            None => {
                self.run(async move { client.create_automation(&draft).await })
                    .await?
            }
            Some(id) => {
                self.run(async move { client.update_automation(id, &draft).await })
                    .await?
            }
        };
        Ok(saved.into())
    }

    pub async fn delete_automation(&self, id: String) -> Result<(), AppleError> {
        let id = parse_id(&id)?;
        let client = self.client.clone();
        self.run(async move { client.delete_automation(id).await })
            .await
    }

    /// Runs the rule now, ignoring its cooldown, its event check and the master switch, and
    /// returns when the run ends. It really records and really moves cameras.
    pub async fn run_automation(&self, id: String) -> Result<AutomationRunRecord, AppleError> {
        let id = parse_id(&id)?;
        let client = self.client.clone();
        Ok(self
            .run(async move { client.run_automation(id).await })
            .await?
            .into())
    }

    /// The last 50 runs, newest first.
    pub async fn automation_runs(&self) -> Result<Vec<AutomationRunRecord>, AppleError> {
        let client = self.client.clone();
        let runs = self
            .run(async move { client.automation_runs().await })
            .await?;
        Ok(runs.into_iter().map(Into::into).collect())
    }

    pub async fn set_automations_armed(&self, armed: bool) -> Result<(), AppleError> {
        let client = self.client.clone();
        self.run(async move { client.set_automations_armed(armed).await })
            .await
    }

    pub async fn queues(&self) -> Result<Vec<QueueSummaryRecord>, AppleError> {
        let client = self.client.clone();
        let queues = self.run(async move { client.queues().await }).await?;
        Ok(queues.into_iter().map(Into::into).collect())
    }

    pub async fn queue_jobs(&self, queue: String) -> Result<Vec<QueueJobRecord>, AppleError> {
        let client = self.client.clone();
        let jobs = self
            .run(async move { client.queue_jobs(&queue).await })
            .await?;
        Ok(jobs.into_iter().map(Into::into).collect())
    }

    pub async fn retry_job(&self, id: String) -> Result<(), AppleError> {
        let id = parse_id(&id)?;
        let client = self.client.clone();
        self.run(async move { client.retry_job(id).await }).await
    }

    pub async fn purge_job(&self, id: String) -> Result<(), AppleError> {
        let id = parse_id(&id)?;
        let client = self.client.clone();
        self.run(async move { client.purge_job(id).await }).await
    }

    /// Runs the upload cycle now.
    pub async fn trigger_uploads(&self) -> Result<(), AppleError> {
        let client = self.client.clone();
        self.run(async move { client.trigger_uploads().await })
            .await
    }

    pub async fn broadlink_commands(&self) -> Result<Vec<BroadlinkCommandRecord>, AppleError> {
        let client = self.client.clone();
        let commands = self
            .run(async move { client.broadlink_commands().await })
            .await?;
        Ok(commands.into_iter().map(Into::into).collect())
    }

    pub async fn send_broadlink_command(&self, id: String) -> Result<(), AppleError> {
        let id = parse_id(&id)?;
        let client = self.client.clone();
        self.run(async move { client.send_broadlink_command(id).await })
            .await
    }

    pub async fn untracked_recordings(&self) -> Result<Vec<UntrackedRecordingRecord>, AppleError> {
        let client = self.client.clone();
        let recordings = self
            .run(async move { client.untracked_recordings().await })
            .await?;
        Ok(recordings.into_iter().map(Into::into).collect())
    }

    /// Describes a video file on the server's disk and the render the stored settings would make.
    /// With `sidecar_path` (such as Resolve's .sidecar), its settings become the defaults.
    pub async fn probe_media(
        &self,
        path: String,
        sidecar_path: Option<String>,
    ) -> Result<MediaProbeRecord, AppleError> {
        let client = self.client.clone();
        let probe = self
            .run(async move { client.probe_media(&path, sidecar_path.as_deref()).await })
            .await?;
        Ok(probe.into())
    }

    /// One frame of `path` as a JPEG: as recorded without `settings`, else graded with them.
    pub async fn preview_media(
        &self,
        path: String,
        frame: u64,
        settings: Option<RenderSettings>,
        max_width: Option<u32>,
    ) -> Result<PreviewFrame, AppleError> {
        let client = self.client.clone();
        let request = PreviewRequest {
            path,
            frame,
            settings,
            max_width,
        };
        self.run(async move { client.preview_media(&request).await })
            .await
    }

    /// Queues a render. `settings` replaces the stored defaults for this file only; without an
    /// event the finished file is listed with the untracked recordings. `portrait` also makes a
    /// 9:16 copy.
    pub async fn start_render(
        &self,
        path: String,
        event_id: Option<String>,
        settings: Option<RenderSettings>,
        portrait: Option<PortraitRequest>,
    ) -> Result<RenderQueuedRecord, AppleError> {
        let event_id = event_id.as_deref().map(parse_id).transpose()?;
        let client = self.client.clone();
        let request = RenderRequest {
            path,
            event_id,
            settings,
            portrait,
        };
        let queued = self
            .run(async move { client.start_render(&request).await })
            .await?;
        Ok(RenderQueuedRecord {
            job_id: queued.job_id.to_string(),
            output_path: queued.output_path,
        })
    }

    /// Describes audio files on the server's disk, joined in the order given.
    pub async fn probe_audio(&self, paths: Vec<String>) -> Result<AudioProbeRecord, AppleError> {
        let client = self.client.clone();
        let probe = self
            .run(async move { client.probe_audio(&paths).await })
            .await?;
        Ok(probe.into())
    }

    /// Queues an audio master. `settings` replaces the stored defaults for these files only;
    /// without an event the master is listed with the untracked recordings.
    pub async fn start_audio(
        &self,
        paths: Vec<String>,
        event_id: Option<String>,
        settings: Option<AudioSettings>,
    ) -> Result<RenderQueuedRecord, AppleError> {
        let event_id = event_id.as_deref().map(parse_id).transpose()?;
        let client = self.client.clone();
        let request = AudioRequest {
            paths,
            event_id,
            settings,
        };
        let queued = self
            .run(async move { client.start_audio(&request).await })
            .await?;
        Ok(RenderQueuedRecord {
            job_id: queued.job_id.to_string(),
            output_path: queued.output_path,
        })
    }

    /// Describes a video and its audio (none: the video's own sound) and how they'd be synced.
    pub async fn probe_program(
        &self,
        video_path: String,
        audio_paths: Vec<String>,
    ) -> Result<ProgramProbeRecord, AppleError> {
        let client = self.client.clone();
        let probe = self
            .run(async move { client.probe_program(&video_path, &audio_paths).await })
            .await?;
        Ok(probe.into())
    }

    /// Queues a program: the video with its audio, synced. Its `None` fields take the probe's
    /// choice and the stored defaults; see [`ProgramRequestRecord`].
    pub async fn start_program(
        &self,
        request: ProgramRequestRecord,
    ) -> Result<RenderQueuedRecord, AppleError> {
        let event_id = request.event_id.as_deref().map(parse_id).transpose()?;
        let client = self.client.clone();
        let request = ProgramRequest {
            video_path: request.video_path,
            audio_paths: request.audio_paths,
            event_id,
            picture: request.picture,
            settings: request.settings,
            audio_settings: request.audio_settings,
            start_seconds: request.start_seconds,
            subtitles: request.subtitles,
            portrait: request.portrait,
        };
        let queued = self
            .run(async move { client.start_program(&request).await })
            .await?;
        Ok(RenderQueuedRecord {
            job_id: queued.job_id.to_string(),
            output_path: queued.output_path,
        })
    }

    /// Moves a finished program's audio by whole frames (positive: later). Its progress arrives
    /// under the program's job id.
    pub async fn nudge_program(&self, job_id: String, frames: i32) -> Result<(), AppleError> {
        let id = parse_id(&job_id)?;
        let client = self.client.clone();
        self.run(async move { client.nudge_program(id, frames).await })
            .await?;
        Ok(())
    }

    /// Places transcripts dropped on a finished program, as its job would have. Its progress
    /// arrives under the program's job id.
    pub async fn add_program_subtitles(
        &self,
        job_id: String,
        subtitles: Vec<SubtitleSource>,
    ) -> Result<(), AppleError> {
        let id = parse_id(&job_id)?;
        let client = self.client.clone();
        self.run(async move { client.add_program_subtitles(id, subtitles).await })
            .await?;
        Ok(())
    }

    /// Stops a queued or running render, audio or program job.
    pub async fn cancel_render(&self, job_id: String) -> Result<(), AppleError> {
        let id = parse_id(&job_id)?;
        let client = self.client.clone();
        self.run(async move { client.cancel_render(id).await })
            .await
    }

    /// Moves a stray recording onto an event.
    pub async fn assign_untracked(&self, id: String, event_id: String) -> Result<(), AppleError> {
        let id = parse_id(&id)?;
        let event = parse_id(&event_id)?;
        let client = self.client.clone();
        self.run(async move { client.assign_untracked(id, event).await })
            .await
    }

    pub async fn delete_untracked(&self, id: String) -> Result<(), AppleError> {
        let id = parse_id(&id)?;
        let client = self.client.clone();
        self.run(async move { client.delete_untracked(id).await })
            .await
    }

    /// Book and chapter autocomplete for a Bible reference.
    pub async fn bible_suggestions(
        &self,
        term: String,
    ) -> Result<Vec<BibleSuggestionRecord>, AppleError> {
        let client = self.client.clone();
        let suggestions = self
            .run(async move { client.bible_suggestions(&term).await })
            .await?;
        Ok(suggestions
            .into_iter()
            .map(|suggestion| BibleSuggestionRecord {
                label: suggestion.label,
                category: suggestion.cat,
            })
            .collect())
    }

    // ── Transcript library ───────────────────────────────────────────────────

    pub async fn transcripts(&self) -> Result<Vec<StoredTranscriptRecord>, AppleError> {
        let client = self.client.clone();
        let list = self.run(async move { client.transcripts().await }).await?;
        Ok(list.into_iter().map(Into::into).collect())
    }

    pub async fn transcript(&self, id: String) -> Result<TranscriptDetailRecord, AppleError> {
        let id = parse_id(&id)?;
        let client = self.client.clone();
        let detail = self.run(async move { client.transcript(id).await }).await?;
        Ok(TranscriptDetailRecord {
            transcript: detail.transcript.into(),
            lines: detail
                .segments
                .into_iter()
                .map(|s| TranscriptLineRecord {
                    seq: s.seq,
                    text: s.text,
                    clean_text: s.clean_text,
                })
                .collect(),
        })
    }

    pub async fn delete_transcript(&self, id: String) -> Result<(), AppleError> {
        let id = parse_id(&id)?;
        let client = self.client.clone();
        self.run(async move { client.delete_transcript(id).await })
            .await
    }

    /// The file's text; `format` is `txt`, `srt` or `vtt`.
    pub async fn download_transcript(
        &self,
        id: String,
        format: String,
        clean: bool,
    ) -> Result<String, AppleError> {
        let id = parse_id(&id)?;
        let client = self.client.clone();
        self.run(async move { client.download_transcript(id, &format, clean).await })
            .await
    }

    /// Starts the OpenAI clean-up; progress arrives as `TranscriptCleanup` events.
    pub async fn cleanup_transcript(&self, id: String) -> Result<(), AppleError> {
        let id = parse_id(&id)?;
        let client = self.client.clone();
        self.run(async move { client.cleanup_transcript(id).await })
            .await
            .map(drop)
    }

    /// The core's own log. Only a core inside the desktop app has one.
    pub async fn application_log(&self) -> Result<String, AppleError> {
        let client = self.client.clone();
        Ok(self
            .run(async move { client.application_log().await })
            .await?
            .content)
    }

    /// The files recorded for one event, newest first.
    pub async fn recordings(&self, event_id: String) -> Result<Vec<RecordingRecord>, AppleError> {
        let id = parse_id(&event_id)?;
        let client = self.client.clone();
        let recordings = self.run(async move { client.recordings(id).await }).await?;
        Ok(recordings.into_iter().map(Into::into).collect())
    }

    /// Marks a recording to be uploaded to YouTube.
    pub async fn flag_upload(
        &self,
        event_id: String,
        recording_id: String,
        platforms: Vec<String>,
    ) -> Result<(), AppleError> {
        let event = parse_id(&event_id)?;
        let recording = parse_id(&recording_id)?;
        let client = self.client.clone();
        self.run(async move { client.flag_upload(event, recording, platforms).await })
            .await
    }

    /// Takes the event's YouTube or Facebook broadcast live (`start`) or ends it. A refusal
    /// arrives as `Server` with the server's code (`encoder_not_streaming`, `not_live`, …).
    pub async fn event_stream(
        &self,
        event_id: String,
        platform: String,
        start: bool,
    ) -> Result<(), AppleError> {
        let event = parse_id(&event_id)?;
        let client = self.client.clone();
        self.run(async move { client.event_stream(event, &platform, start).await })
            .await
    }

    /// The server's slide folders and title template.
    pub async fn server_settings(&self) -> Result<ServerSettingsRecord, AppleError> {
        let client = self.client.clone();
        let settings = self
            .run(async move { client.server_settings().await })
            .await?;
        Ok(ServerSettingsRecord {
            title_template: settings.title_template,
            slide_folder: settings.slide_folder,
            song_slide_folder: settings.song_slide_folder,
        })
    }

    /// Chooses the recorders the Record button starts and stops; every client hears it in
    /// `RecordersState`.
    pub async fn set_record_button(&self, targets: Vec<RecordTarget>) -> Result<(), AppleError> {
        let client = self.client.clone();
        self.run(async move { client.set_record_button(targets).await })
            .await
    }

    /// Saves them. The server refuses a folder that does not exist.
    pub async fn save_server_settings(
        &self,
        settings: ServerSettingsRecord,
    ) -> Result<(), AppleError> {
        let settings = metocast_client::ServerSettings {
            title_template: settings.title_template,
            slide_folder: settings.slide_folder,
            song_slide_folder: settings.song_slide_folder,
        };
        let client = self.client.clone();
        self.run(async move { client.save_server_settings(&settings).await })
            .await
    }

    /// Every connector this app can configure, in display order.
    pub fn connectors(&self) -> Vec<ConnectorNameRecord> {
        metocast_core::config::CONNECTORS
            .iter()
            .map(|(id, name)| ConnectorNameRecord {
                id: (*id).to_string(),
                name: (*name).to_string(),
            })
            .collect()
    }

    /// One connector's settings, as fields to show. Stored secrets come back blank but flagged.
    pub async fn connector_form(
        &self,
        connector: String,
    ) -> Result<Vec<ConfigFieldRecord>, AppleError> {
        let client = self.client.clone();
        let name = connector.clone();
        let config = self
            .run(async move { client.connector_config(&name).await })
            .await?;
        Ok(metocast_core::config::form(&connector, &config)
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// The stored secrets for one connector, which only the Mac hosting the server can read.
    /// Returns the same fields as `connector_form`, with the secrets filled in.
    pub async fn connector_secrets(
        &self,
        connector: String,
        admin_token: String,
    ) -> Result<Vec<ConfigFieldRecord>, AppleError> {
        let client = self.client.clone();
        let name = connector.clone();
        let secrets = self
            .run(async move { client.connector_secrets(&name, &admin_token).await })
            .await?;
        Ok(metocast_core::config::schema(&connector)
            .iter()
            .filter(|(_, _, kind)| matches!(kind, metocast_core::config::ConfigFieldKind::Secret))
            .map(|(key, label, kind)| ConfigFieldRecord {
                key: (*key).to_string(),
                label: (*label).to_string(),
                kind: *kind,
                value: secrets
                    .get(*key)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                is_set: true,
            })
            .collect())
    }

    /// Saves a connector's settings and applies them on the server.
    pub async fn save_connector_form(
        &self,
        connector: String,
        values: Vec<ConfigValueRecord>,
    ) -> Result<(), AppleError> {
        let body = metocast_core::config::body(
            &connector,
            &values.into_iter().map(Into::into).collect::<Vec<_>>(),
        );
        let client = self.client.clone();
        self.run(async move { client.save_connector_config(&connector, &body).await })
            .await
    }

    /// Where to send the operator to sign in to YouTube (`youtube`) or Facebook (`facebook`).
    pub async fn auth_url(&self, platform: String) -> Result<String, AppleError> {
        let client = self.client.clone();
        self.run(async move { client.auth_url(&platform).await })
            .await
    }

    /// Submits the OAuth code and state a client device captured on its loopback listener.
    pub async fn submit_oauth_callback(
        &self,
        code: String,
        state: String,
    ) -> Result<(), AppleError> {
        let client = self.client.clone();
        self.run(async move {
            client
                .submit_oauth_callback(&code, &state)
                .await
                .map(|_| ())
        })
        .await
    }

    pub async fn sign_out(&self, platform: String) -> Result<(), AppleError> {
        let client = self.client.clone();
        self.run(async move { client.sign_out(&platform).await })
            .await
    }

    /// Decks in the server's watched folders by title, hymn number or slide text, best match
    /// first. `folder_id` narrows to one folder; `limit` is 5 when absent.
    pub async fn ppt_files(
        &self,
        filter: String,
        folder_id: Option<String>,
        limit: Option<u32>,
    ) -> Result<Vec<PptFile>, AppleError> {
        let folder_id = folder_id.map(|id| parse_id(&id)).transpose()?;
        let client = self.client.clone();
        self.run(async move { client.ppt_files(&filter, folder_id, limit).await })
            .await
    }

    pub async fn ppt_folders(&self) -> Result<Vec<PptFolderRecord>, AppleError> {
        let client = self.client.clone();
        let folders = self.run(async move { client.ppt_folders().await }).await?;
        Ok(folders
            .into_iter()
            .map(|folder| PptFolderRecord {
                id: folder.id.to_string(),
                name: folder.name,
                path: folder.path,
            })
            .collect())
    }

    pub async fn welcome_settings(&self) -> Result<WelcomeSettingsRecord, AppleError> {
        let client = self.client.clone();
        let settings = self
            .run(async move { client.welcome_settings().await })
            .await?;
        Ok(settings.into())
    }

    pub async fn set_welcome_settings(
        &self,
        settings: WelcomeSettingsRecord,
    ) -> Result<WelcomeSettingsRecord, AppleError> {
        let settings = WelcomeSettings {
            title: settings.title,
            subtitle: settings.subtitle,
            church_name: settings.church_name,
            language: settings.language,
            has_logo: settings.has_logo,
            replaced_photos: settings.replaced_photos,
        };
        let client = self.client.clone();
        let stored = self
            .run(async move { client.set_welcome_settings(&settings).await })
            .await?;
        Ok(stored.into())
    }

    /// The welcome slide for month `number` (1–12) as SVG, to preview it.
    pub async fn welcome_preview(&self, number: u32) -> Result<String, AppleError> {
        let client = self.client.clone();
        self.run(async move { client.welcome_preview(number).await })
            .await
    }

    /// Welcome photo `number` (1–12) as JPEG bytes, for a thumbnail.
    pub async fn welcome_photo(&self, number: u32) -> Result<Vec<u8>, AppleError> {
        let client = self.client.clone();
        self.run(async move { client.welcome_photo(number).await })
            .await
    }

    pub async fn set_welcome_photo(&self, number: u32, bytes: Vec<u8>) -> Result<(), AppleError> {
        let client = self.client.clone();
        self.run(async move { client.set_welcome_photo(number, bytes).await })
            .await
    }

    pub async fn reset_welcome_photo(&self, number: u32) -> Result<(), AppleError> {
        let client = self.client.clone();
        self.run(async move { client.reset_welcome_photo(number).await })
            .await
    }

    pub async fn set_welcome_logo(&self, bytes: Vec<u8>) -> Result<(), AppleError> {
        let client = self.client.clone();
        self.run(async move { client.set_welcome_logo(bytes).await })
            .await
    }

    pub async fn remove_welcome_logo(&self) -> Result<(), AppleError> {
        let client = self.client.clone();
        self.run(async move { client.remove_welcome_logo().await })
            .await
    }

    /// An event's Order of Service, in order.
    pub async fn event_order(&self, id: String) -> Result<Vec<OrderItemRecord>, AppleError> {
        let id = parse_id(&id)?;
        let client = self.client.clone();
        let items = self
            .run(async move { client.event_order(id).await })
            .await?;
        Ok(items.into_iter().map(Into::into).collect())
    }

    /// Replaces an event's Order of Service; returns the stored steps with their ids.
    pub async fn set_event_order(
        &self,
        id: String,
        items: Vec<OrderItemRecord>,
    ) -> Result<Vec<OrderItemRecord>, AppleError> {
        let id = parse_id(&id)?;
        let items = items
            .into_iter()
            .map(|item| {
                Ok(OrderItem {
                    id: item.id.as_deref().map(parse_id).transpose()?,
                    kind: item.kind,
                    title: item.title,
                    deck_path: item.deck_path,
                    bible_type: item.bible_type,
                    reference: item.reference,
                    translation: item.translation,
                    notes: item.notes,
                })
            })
            .collect::<Result<Vec<_>, AppleError>>()?;
        let client = self.client.clone();
        let stored = self
            .run(async move { client.set_event_order(id, items).await })
            .await?;
        Ok(stored.into_iter().map(Into::into).collect())
    }

    /// Watches a folder on the server's disk for decks.
    pub async fn add_ppt_folder(&self, path: String, name: String) -> Result<(), AppleError> {
        let client = self.client.clone();
        self.run(async move { client.add_ppt_folder(&path, &name).await })
            .await
            .map(drop)
    }

    pub async fn remove_ppt_folder(&self, id: String) -> Result<(), AppleError> {
        let id = parse_id(&id)?;
        let client = self.client.clone();
        self.run(async move { client.remove_ppt_folder(id).await })
            .await
    }

    /// Writes a deck per Bible reference on the event; returns the files written.
    pub async fn create_event_slides(&self, id: String) -> Result<Vec<String>, AppleError> {
        let id = parse_id(&id)?;
        let client = self.client.clone();
        Ok(self
            .run(async move { client.create_event_slides(id).await })
            .await?
            .files)
    }

    pub async fn create_song_slides(
        &self,
        title: String,
        lyrics: String,
    ) -> Result<SongSlides, AppleError> {
        let song = CreateSongSlides { title, lyrics };
        let client = self.client.clone();
        self.run(async move { client.create_song_slides(&song).await })
            .await
    }

    pub fn shutdown_events(&self) {
        if let Ok(mut connection) = self.connection.lock()
            && let Some(connection) = connection.take()
        {
            connection.close();
        }
        if let Ok(mut task) = self.event_task.lock()
            && let Some(task) = task.take()
        {
            task.abort();
        }
    }
}

impl AppleClient {
    /// Queues one command on the event connection.
    async fn send(
        &self,
        command: impl Serialize + Send + Sync + 'static,
    ) -> Result<(), AppleError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| AppleError::Runtime)?
            .clone()
            .ok_or(AppleError::NotConnected)?;
        self.runtime
            .spawn(async move { connection.send(&command).await })
            .await
            .map_err(|_| AppleError::Runtime)?
            .map_err(|_| AppleError::NotConnected)
    }

    /// Runs one request on this client's runtime.
    async fn run<T: Send + 'static>(
        &self,
        request: impl Future<Output = Result<T, ClientError>> + Send + 'static,
    ) -> Result<T, AppleError> {
        self.runtime
            .spawn(request)
            .await
            .map_err(|_| AppleError::Runtime)?
            .map_err(map_client_error)
    }
}

fn parse_id(id: &str) -> Result<uuid::Uuid, AppleError> {
    uuid::Uuid::parse_str(id).map_err(|_| AppleError::InvalidIdentifier)
}

/// Keeps the three ways a request can fail apart, because they send the reader to three
/// different places: the network, the server's own refusal, and a contract mismatch.
fn map_client_error(error: ClientError) -> AppleError {
    match error {
        ClientError::Authentication => AppleError::Authentication,
        ClientError::Config(_) => AppleError::InvalidConfiguration,
        ClientError::Http {
            message: Some(message),
            ..
        } => AppleError::Server { message },
        ClientError::Http { status, .. } => AppleError::Refused {
            status: status.as_u16(),
        },
        ClientError::Transport(_) => AppleError::Transport,
        ClientError::Decode(_) => AppleError::Contract,
    }
}

fn connector_status(connector: String, status: ConnectorStatus) -> ConnectorStatusRecord {
    let (status, message) = match status {
        ConnectorStatus::Disconnected => (ConnectorStatusCode::Disconnected, None),
        ConnectorStatus::Connecting => (ConnectorStatusCode::Connecting, None),
        ConnectorStatus::Connected => (ConnectorStatusCode::Connected, None),
        ConnectorStatus::Error { message } => (ConnectorStatusCode::Error, Some(message)),
    };
    ConnectorStatusRecord {
        connector,
        status,
        message,
    }
}

fn connection_state(state: ConnectionState) -> AppleConnectionState {
    match state {
        ConnectionState::Connecting => AppleConnectionState::Connecting,
        ConnectionState::Connected => AppleConnectionState::Connected,
        ConnectionState::Reconnecting => AppleConnectionState::Reconnecting,
        ConnectionState::AuthenticationFailed => AppleConnectionState::AuthenticationFailed,
        ConnectionState::Closed => AppleConnectionState::Closed,
    }
}

impl From<EventSummary> for EventSummaryRecord {
    fn from(event: EventSummary) -> Self {
        Self {
            id: event.id.to_string(),
            title: event.title,
            computed_title: event.computed_title,
            date_time: event.date_time.to_rfc3339(),
            speaker: event.speaker,
            recording_count: event.recording_count,
            is_completed: event.is_completed,
            created_at: event.created_at.to_rfc3339(),
            updated_at: event.updated_at.to_rfc3339(),
            connections: event.connections.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<EventConnection> for EventConnectionRecord {
    fn from(connection: EventConnection) -> Self {
        Self {
            platform: connection.platform,
            external_id: connection.external_id,
            stream_url: connection.stream_url,
            event_url: connection.event_url,
            schedule_status: connection.schedule_status,
            privacy_status: connection.privacy_status,
        }
    }
}

impl From<BibleVerse> for BibleVerseRecord {
    fn from(verse: BibleVerse) -> Self {
        Self {
            chapter: verse.chapter,
            verse: verse.verse,
            text: verse.text,
        }
    }
}

impl From<BibleReference> for BibleReferenceRecord {
    fn from(reference: BibleReference) -> Self {
        let verses = serde_json::from_value::<Vec<BibleVerse>>(reference.verses)
            .unwrap_or_default()
            .into_iter()
            .map(Into::into)
            .collect();
        Self {
            reference_type: reference.r#type,
            reference: reference.reference,
            translation: reference.translation,
            verses,
        }
    }
}

impl From<Event> for EventRecord {
    fn from(event: Event) -> Self {
        Self {
            id: event.id.to_string(),
            title: event.title,
            computed_title: event.computed_title,
            date_time: event.date_time.to_rfc3339(),
            speaker: event.speaker,
            description: event.description,
            auto_upload_enabled: event.auto_upload_enabled,
            created_at: event.created_at.to_rfc3339(),
            updated_at: event.updated_at.to_rfc3339(),
            connections: event.connections.into_iter().map(Into::into).collect(),
            bible_references: event.bible_references.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<EventDraftRecord> for CreateEvent {
    type Error = AppleError;

    fn try_from(draft: EventDraftRecord) -> Result<Self, AppleError> {
        let date_time = chrono::DateTime::parse_from_rfc3339(&draft.date_time)
            .map_err(|_| AppleError::InvalidInput)?
            .to_utc();
        Ok(Self {
            title: draft.title,
            computed_title: Some(draft.computed_title),
            date_time,
            speaker: Some(draft.speaker),
            description: Some(draft.description),
            auto_upload_enabled: Some(draft.auto_upload_enabled),
            // Other platforms keep their stored privacy; the server creates them on insert.
            connections: Some(vec![CreateConnection {
                platform: "youtube".to_string(),
                privacy_status: Some(draft.youtube_privacy),
            }]),
            bible_references: Some(
                draft
                    .bible_references
                    .into_iter()
                    .map(|reference| CreateBibleReference {
                        r#type: reference.reference_type,
                        reference: Some(reference.reference),
                        translation: Some(reference.translation),
                        verses: Some(serde_json::json!(
                            reference
                                .verses
                                .into_iter()
                                .map(|verse| BibleVerse {
                                    chapter: verse.chapter,
                                    verse: verse.verse,
                                    text: verse.text,
                                })
                                .collect::<Vec<_>>()
                        )),
                    })
                    .collect(),
            ),
        })
    }
}

impl From<PresenterControl> for PresenterCommand {
    fn from(control: PresenterControl) -> Self {
        match control {
            PresenterControl::Status => Self::Status,
            PresenterControl::Next => Self::Next,
            PresenterControl::Previous => Self::Previous,
            PresenterControl::First => Self::First,
            PresenterControl::Last => Self::Last,
            PresenterControl::GoTo { slide } => Self::GoTo { slide },
            PresenterControl::Mute => Self::Mute,
            PresenterControl::Unmute => Self::Unmute,
        }
    }
}

impl TryFrom<ProductionControl> for ProductionCommand {
    type Error = AppleError;

    fn try_from(control: ProductionControl) -> Result<Self, AppleError> {
        Ok(match control {
            ProductionControl::ObsStreaming { on: true } => Self::ObsStreamStart,
            ProductionControl::ObsStreaming { on: false } => Self::ObsStreamStop,
            ProductionControl::ObsRecording { on: true } => Self::ObsRecordStart,
            ProductionControl::ObsRecording { on: false } => Self::ObsRecordStop,
            ProductionControl::AtemProgram { input } => Self::AtemProgram { input },
            ProductionControl::AtemPreview { input } => Self::AtemPreview { input },
            ProductionControl::AtemCut => Self::AtemCut,
            ProductionControl::AtemAuto => Self::AtemAuto,
            ProductionControl::AtemStreaming { on: true } => Self::AtemStreamStart,
            ProductionControl::AtemStreaming { on: false } => Self::AtemStreamStop,
            ProductionControl::AtemRecording { on: true } => Self::AtemRecordStart,
            ProductionControl::AtemRecording { on: false } => Self::AtemRecordStop,
            ProductionControl::MiddlecontrolCamera { camera_id } => {
                Self::MiddlecontrolCamera { camera_id }
            }
            ProductionControl::MiddlecontrolRecordingAll { on: true } => {
                Self::MiddlecontrolRecordStartAll
            }
            ProductionControl::MiddlecontrolRecordingAll { on: false } => {
                Self::MiddlecontrolRecordStopAll
            }
            ProductionControl::RodecasterProfile => Self::RodecasterProfile,
            ProductionControl::RodecasterMute { channel, mute } => {
                Self::RodecasterMute { channel, mute }
            }
            ProductionControl::RodecasterRecordStart { event_id } => Self::RodecasterRecordStart {
                event_id: event_id.as_deref().map(parse_id).transpose()?,
            },
            ProductionControl::RodecasterRecordStop => Self::RodecasterRecordStop,
            ProductionControl::RodecasterDeskRecording { on: true } => {
                Self::RodecasterDeskRecordStart
            }
            ProductionControl::RodecasterDeskRecording { on: false } => {
                Self::RodecasterDeskRecordStop
            }
            ProductionControl::TranscriptionStart { event_id } => Self::TranscriptionStart {
                event_id: event_id.as_deref().map(parse_id).transpose()?,
            },
            ProductionControl::TranscriptionStop => Self::TranscriptionStop,
            ProductionControl::CameraRecording { on: true } => Self::CameraRecordStart,
            ProductionControl::CameraRecording { on: false } => Self::CameraRecordStop,
            ProductionControl::CameraStreaming { on: true } => Self::CameraStreamStart,
            ProductionControl::CameraStreaming { on: false } => Self::CameraStreamStop,
            ProductionControl::CameraPushYoutube => Self::CameraPushYoutube,
            ProductionControl::AtemPushYoutube => Self::AtemPushYoutube,
            ProductionControl::Recorders { on: true } => Self::RecordersStart,
            ProductionControl::Recorders { on: false } => Self::RecordersStop,
        })
    }
}

impl TryFrom<DeviceAlertControl> for DeviceAlertCommand {
    type Error = AppleError;

    fn try_from(command: DeviceAlertControl) -> Result<Self, AppleError> {
        Ok(match command {
            DeviceAlertControl::List => Self::List,
            DeviceAlertControl::Scan => Self::Scan,
            DeviceAlertControl::Available => Self::Available,
            DeviceAlertControl::Create {
                category,
                device_item_value,
                device_item_name,
                friendly_name,
            } => Self::Create {
                // Only OBS reports devices to watch today.
                connector_type: "obs".to_string(),
                category,
                device_item_value,
                device_item_name,
                friendly_name,
            },
            DeviceAlertControl::Delete { id } => Self::Delete { id: parse_id(&id)? },
        })
    }
}

impl From<DeviceListener> for DeviceListenerRecord {
    fn from(listener: DeviceListener) -> Self {
        Self {
            id: listener.id.to_string(),
            category: listener.category,
            device_item_name: listener.device_item_name,
            friendly_name: listener.friendly_name,
        }
    }
}

impl From<CronJob> for CronJobRecord {
    fn from(job: CronJob) -> Self {
        Self {
            id: job.id.to_string(),
            name: job.name,
            cron_expression: job.cron_expression,
            enabled: job.enabled,
            pull_youtube: job.pull_youtube,
            auto_upload: job.auto_upload,
        }
    }
}

impl From<Automation> for AutomationRecord {
    fn from(automation: Automation) -> Self {
        Self {
            id: automation.id.to_string(),
            name: automation.name,
            enabled: automation.enabled,
            triggers: automation.triggers,
            actions: automation.actions,
            cooldown_seconds: automation.cooldown_seconds,
            require_event: automation.require_event,
        }
    }
}

impl From<AutomationRecord> for AutomationDraft {
    fn from(record: AutomationRecord) -> Self {
        Self {
            name: record.name,
            enabled: record.enabled,
            triggers: record.triggers,
            actions: record.actions,
            cooldown_seconds: record.cooldown_seconds,
            require_event: record.require_event,
        }
    }
}

impl From<AutomationDraft> for AutomationRecord {
    fn from(draft: AutomationDraft) -> Self {
        Self {
            id: String::new(),
            name: draft.name,
            enabled: draft.enabled,
            triggers: draft.triggers,
            actions: draft.actions,
            cooldown_seconds: draft.cooldown_seconds,
            require_event: draft.require_event,
        }
    }
}

impl From<AutomationList> for AutomationListRecord {
    fn from(list: AutomationList) -> Self {
        Self {
            armed: list.armed,
            automations: list.automations.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<AutomationRun> for AutomationRunRecord {
    fn from(run: AutomationRun) -> Self {
        Self {
            id: run.id.to_string(),
            automation_id: run.automation_id.to_string(),
            name: run.name,
            cause: run.cause,
            manual: run.manual,
            started_at: run.started_at.to_rfc3339(),
            results: run.results,
        }
    }
}

/// What is wrong with a rule, in words, or nothing when the server will accept it: the same
/// limits the server enforces, so an editor can say so before it saves.
#[uniffi::export]
pub fn automation_problem(automation: AutomationRecord) -> Option<String> {
    AutomationDraft::from(automation).validate().err()
}

/// The opposite rule: the actions in reverse order with start and stop swapped, so recording
/// stops before the camera moves. A channel trigger flips (one that went live now goes silent
/// for at least 5 s), preset triggers are dropped for the person to pick, and every camera move
/// goes to `camera_preset`, or is left out when there is none. The result has no id.
#[uniffi::export]
pub fn reverse_automation(
    automation: AutomationRecord,
    camera_preset: Option<u8>,
) -> AutomationRecord {
    AutomationDraft::from(automation)
        .reversed(camera_preset)
        .into()
}

impl From<QueueSummary> for QueueSummaryRecord {
    fn from(queue: QueueSummary) -> Self {
        Self {
            queue: queue.queue,
            pending: queue.pending,
            processing: queue.processing,
            succeeded: queue.succeeded,
            dead: queue.dead,
        }
    }
}

impl From<QueueJob> for QueueJobRecord {
    fn from(job: QueueJob) -> Self {
        Self {
            id: job.id.to_string(),
            queue: job.queue,
            job_type: job.job_type,
            status: job.status,
            attempts: job.attempts,
            max_attempts: job.max_attempts,
            last_error: job.last_error,
            updated_at: job.updated_at.to_rfc3339(),
        }
    }
}

impl From<BroadlinkCommand> for BroadlinkCommandRecord {
    fn from(command: BroadlinkCommand) -> Self {
        Self {
            id: command.id.to_string(),
            name: command.name,
            category: command.category,
        }
    }
}

impl From<MediaProbe> for MediaProbeRecord {
    fn from(probe: MediaProbe) -> Self {
        Self {
            path: probe.path,
            file_name: probe.file_name,
            file_size: probe.file_size,
            kind: probe.kind,
            camera: probe.camera,
            clip_name: probe.clip_name,
            width: probe.width,
            height: probe.height,
            active_area: probe.active_area,
            fps_num: probe.fps_num,
            fps_den: probe.fps_den,
            frame_count: probe.frame_count,
            duration_seconds: probe.duration_seconds,
            start_timecode: probe.start_timecode,
            recorded_at: probe.recorded_at.map(|date| date.to_rfc3339()),
            host_record_started_at: probe.host_record_started_at.map(|date| date.to_rfc3339()),
            audio: probe.audio,
            gamma: probe.gamma,
            gamut: probe.gamut,
            embedded_lut: probe.embedded_lut,
            defaults: probe.defaults,
            output: probe.output,
            missing: probe.missing,
            raw_options: probe.raw_options,
        }
    }
}

impl From<AudioProbe> for AudioProbeRecord {
    fn from(probe: AudioProbe) -> Self {
        Self {
            sources: probe
                .sources
                .into_iter()
                .map(|source: AudioSource| AudioSourceRecord {
                    path: source.path,
                    file_name: source.file_name,
                    file_size: source.file_size,
                    audio: source.audio,
                    duration_seconds: source.duration_seconds,
                    recorded_at: source.recorded_at.map(|date| date.to_rfc3339()),
                })
                .collect(),
            duration_seconds: probe.duration_seconds,
            channels: probe.channels,
            recorded_at: probe.recorded_at.map(|date| date.to_rfc3339()),
            defaults: probe.defaults,
            missing: probe.missing,
            downloads: probe.downloads,
        }
    }
}

impl From<AudioProgress> for AudioProgressRecord {
    fn from(progress: AudioProgress) -> Self {
        Self {
            job_id: progress.job_id.to_string(),
            state: progress.state,
            step: progress.step,
            fraction: progress.fraction,
            output_path: progress.output_path,
            error: progress.error,
            report: progress.report,
        }
    }
}

impl From<ProgramProbe> for ProgramProbeRecord {
    fn from(probe: ProgramProbe) -> Self {
        Self {
            video: probe.video.into(),
            audio: probe.audio.map(Into::into),
            video_sound: probe.video_sound,
            sync_method: probe.sync_method,
            clock: probe.clock,
            picture: probe.picture,
            warnings: probe.warnings,
            blocker: probe.blocker,
            missing: probe.missing,
            downloads: probe.downloads,
        }
    }
}

impl From<ProgramProgress> for ProgramProgressRecord {
    fn from(progress: ProgramProgress) -> Self {
        Self {
            job_id: progress.job_id.to_string(),
            state: progress.state,
            step: progress.step,
            fraction: progress.fraction,
            output_path: progress.output_path,
            error: progress.error,
            report: progress.report,
        }
    }
}

impl From<RenderProgress> for RenderProgressRecord {
    fn from(progress: RenderProgress) -> Self {
        Self {
            job_id: progress.job_id.to_string(),
            state: progress.state,
            frame: progress.frame,
            total_frames: progress.total_frames,
            fps: progress.fps,
            output_path: progress.output_path,
            error: progress.error,
        }
    }
}

impl From<UntrackedRecording> for UntrackedRecordingRecord {
    fn from(recording: UntrackedRecording) -> Self {
        Self {
            id: recording.id.to_string(),
            file_name: recording.file_name,
            file_size: recording.file_size,
            duration_seconds: recording.duration_seconds,
            detected_at: recording.detected_at.to_rfc3339(),
        }
    }
}

/// " 42%" for an upload of known size, empty otherwise. The nearest whole percent, in integers so
/// no float cast is involved; a count past the total reads as 100%.
fn progress_label(progress_bytes: i64, total_bytes: i64) -> String {
    if total_bytes <= 0 {
        return String::new();
    }
    let done = i128::from(progress_bytes.max(0)) * 100;
    let total = i128::from(total_bytes);
    format!(" {}%", ((done + total / 2) / total).min(100))
}

impl From<Recording> for RecordingRecord {
    fn from(recording: Recording) -> Self {
        Self {
            id: recording.id.to_string(),
            file_name: recording.file_name,
            file_size: recording.file_size,
            duration_seconds: recording.duration_seconds,
            source: recording.source,
            detected_at: recording.detected_at.to_rfc3339(),
            uploadable: recording.uploadable,
            uploaded: recording.uploaded,
            video_url: recording.video_url,
            upload_states: recording
                .uploads
                .into_iter()
                .map(|upload| {
                    let percent = progress_label(upload.progress_bytes, upload.total_bytes);
                    format!("{}: {}{percent}", upload.platform, upload.state)
                })
                .collect(),
        }
    }
}

impl From<metocast_core::config::ConfigField> for ConfigFieldRecord {
    fn from(field: metocast_core::config::ConfigField) -> Self {
        Self {
            key: field.key,
            label: field.label,
            kind: field.kind,
            value: field.value,
            is_set: field.is_set,
        }
    }
}

impl From<ConfigValueRecord> for metocast_core::config::ConfigValue {
    fn from(value: ConfigValueRecord) -> Self {
        Self {
            key: value.key,
            value: value.value,
            clear: value.clear,
        }
    }
}

impl From<ConnectedClient> for ConnectedClientRecord {
    fn from(client: ConnectedClient) -> Self {
        Self {
            id: client.id.to_string(),
            label: client.label,
            hostname: client.hostname,
            user_agent: client.user_agent,
            connected_at: client.connected_at.to_rfc3339(),
            latency_ms: client.latency_ms,
        }
    }
}

impl From<StoredTranscript> for StoredTranscriptRecord {
    fn from(t: StoredTranscript) -> Self {
        Self {
            id: t.id.to_string(),
            event_title: t.event_title,
            started_at: t.started_at.to_rfc3339(),
            duration_seconds: t.duration_seconds,
            segment_count: t.segment_count,
            live: t.live,
            cleanup_status: t.cleanup_status.as_str().to_string(),
            cleanup_error: t.cleanup_error,
        }
    }
}

impl From<TranscriptionState> for TranscriptionRecord {
    fn from(state: TranscriptionState) -> Self {
        Self {
            status: state.status,
            reason: state.reason,
            event_id: state.event_id.map(|id| id.to_string()),
            started_at: state.started_at.map(|at| at.to_rfc3339()),
            youtube_error: state
                .youtube
                .filter(|youtube| !youtube.ok)
                .and_then(|youtube| youtube.last_error),
        }
    }
}

impl From<CreditStatus> for CreditRecord {
    fn from(credit: CreditStatus) -> Self {
        Self {
            balance_usd: credit.balance_usd,
            low: credit.low,
            as_of: credit.as_of.to_rfc3339(),
        }
    }
}

impl From<RodecasterRecorderState> for RecorderStateRecord {
    fn from(state: RodecasterRecorderState) -> Self {
        Self {
            status: state.status,
            event_id: state.event_id.map(|id| id.to_string()),
            started_at: state.started_at.map(|date| date.to_rfc3339()),
            error: state.error,
        }
    }
}

impl From<ServerEvent> for AppleEvent {
    fn from(event: ServerEvent) -> Self {
        match event {
            ServerEvent::Connected { server_id } => Self::Connected { server_id },
            ServerEvent::ConnectorStatus { connector, status } => Self::ConnectorStatus {
                status: connector_status(connector, status),
            },
            ServerEvent::EventsList { events } => Self::EventsList {
                events: events.into_iter().map(Into::into).collect(),
            },
            ServerEvent::Event { event } => Self::Event {
                event: event.into(),
            },
            ServerEvent::PresenterState { state } => Self::PresenterState {
                state: state.into(),
            },
            ServerEvent::ConnectorStatuses { statuses } => Self::ConnectorStatuses {
                statuses: statuses
                    .into_iter()
                    .map(|(connector, status)| connector_status(connector, status))
                    .collect(),
            },
            ServerEvent::PresenterSlideChanged {
                current_slide,
                total_slides,
            } => Self::PresenterSlideChanged {
                current_slide,
                total_slides,
            },
            ServerEvent::EventChanged {} => Self::EventChanged,
            ServerEvent::OrderState { position } => Self::OrderState {
                event_id: position.map(|p| p.event_id.to_string()),
                item_id: position.map(|p| p.item_id.to_string()),
            },
            ServerEvent::EventOrderChanged { event_id } => Self::EventOrderChanged {
                event_id: event_id.to_string(),
            },
            ServerEvent::Notification { level, message } => Self::Notification { level, message },
            ServerEvent::ConnectorState(ConnectorState::Obs {
                is_streaming,
                is_recording,
            }) => Self::ObsState {
                streaming: is_streaming,
                recording: is_recording,
            },
            ServerEvent::ConnectorState(ConnectorState::Atem { state }) => {
                Self::AtemState { state }
            }
            ServerEvent::ConnectorState(ConnectorState::Middlecontrol { state }) => {
                Self::MiddlecontrolState { state }
            }
            ServerEvent::ConnectorState(ConnectorState::Camera {
                is_streaming,
                is_recording,
                stream_status,
                has_media,
                destination,
            }) => Self::CameraState {
                streaming: is_streaming,
                recording: is_recording,
                stream_status,
                has_media,
                destination,
            },
            ServerEvent::DeviceAlerts { listeners } => Self::DeviceAlerts {
                listeners: listeners.into_iter().map(Into::into).collect(),
            },
            ServerEvent::RodecasterProfile { profile } => Self::RodecasterProfile { profile },
            ServerEvent::RodecasterMute {
                channel,
                label,
                muted,
                remote,
                notify,
            } => Self::RodecasterMute {
                channel,
                label,
                muted,
                remote,
                notify,
            },
            ServerEvent::RodecasterRecorder { state } => Self::RodecasterRecorder {
                state: state.into(),
            },
            ServerEvent::RodecasterDeskRecorder { state } => Self::RodecasterDeskRecorder { state },
            ServerEvent::AutomationsChanged { armed, automations } => Self::AutomationsChanged {
                armed,
                automations: automations.into_iter().map(Into::into).collect(),
            },
            ServerEvent::AutomationRun { run } => Self::AutomationRun { run: run.into() },
            ServerEvent::RecordersState { recorders } => Self::RecordersState {
                recorders: recorders.into_iter().map(Into::into).collect(),
            },
            ServerEvent::RecorderStopped {
                target,
                reason,
                error,
            } => Self::RecorderStopped {
                target,
                reason,
                error,
            },
            ServerEvent::ObsDevices { devices } => Self::Discovery {
                tool: DiscoveryTool::Obs,
                devices: discovery::obs_devices(devices),
                message: None,
            },
            ServerEvent::AudioDiscovery { discovery } => {
                let (devices, message) = discovery::audio_devices(discovery);
                Self::Discovery {
                    tool: DiscoveryTool::Rodecaster,
                    devices,
                    message,
                }
            }
            ServerEvent::Error { message } => Self::CommandFailed { message },
            ServerEvent::PresentationSettings {
                use_web_presenter,
                presenter_theme,
                delivery_log,
            } => Self::PresentationSettings {
                use_web_presenter,
                theme: presenter_theme,
                delivery_log,
            },
            ServerEvent::PresentationStatus { status } => Self::PresentationStatus { status },
            ServerEvent::ClientsList { clients } | ServerEvent::ClientsUpdated { clients } => {
                Self::Clients {
                    clients: clients.into_iter().map(Into::into).collect(),
                }
            }
            ServerEvent::MediaRenderProgress { progress } => Self::MediaRenderProgress {
                progress: progress.into(),
            },
            ServerEvent::MediaAudioProgress { progress } => Self::MediaAudioProgress {
                progress: progress.into(),
            },
            ServerEvent::MediaProgramProgress { progress } => Self::MediaProgramProgress {
                progress: progress.into(),
            },
            ServerEvent::TranscriptionState { state } => Self::Transcription {
                state: state.into(),
            },
            // An empty partial only clears the open line; the last one stays on screen.
            ServerEvent::TranscriptionPartial { text } if !text.is_empty() => {
                Self::TranscriptLine { text }
            }
            ServerEvent::TranscriptionSegment { segment } => {
                Self::TranscriptLine { text: segment.text }
            }
            ServerEvent::TranscriptionSnapshot {
                state,
                segments,
                partial,
                credit,
            } => Self::TranscriptionSnapshot {
                state: state.into(),
                line: if partial.is_empty() {
                    segments
                        .into_iter()
                        .last()
                        .map(|s| s.text)
                        .unwrap_or_default()
                } else {
                    partial
                },
                credit: credit.map(Into::into),
            },
            ServerEvent::TranscriptionCredit { credit } => Self::TranscriptionCredit {
                credit: credit.into(),
            },
            ServerEvent::PptIndexState { indexing, pending } => Self::SlideIndexState {
                pending: if indexing { pending.max(1) } else { 0 },
            },
            ServerEvent::TranscriptCleanupState { progress } => Self::TranscriptCleanup {
                session_id: progress.session_id.to_string(),
                state: progress.state.as_str().to_string(),
                progress: progress.progress,
                error: progress.error,
            },
            ServerEvent::ConnectorState(ConnectorState::Other)
            | ServerEvent::TranscriptionPartial { .. }
            | ServerEvent::Ping { .. }
            | ServerEvent::Unknown => Self::Unknown,
        }
    }
}

impl From<ParagraphContent> for ParagraphRecord {
    fn from(paragraph: ParagraphContent) -> Self {
        Self {
            lines: paragraph.lines,
            align: paragraph.align,
            font_size_pt: paragraph.font_size_pt,
        }
    }
}

impl From<SlideContent> for SlideRecord {
    fn from(slide: SlideContent) -> Self {
        Self {
            index: slide.index,
            paragraphs: slide.paragraphs.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<SvgSlideContent> for SvgSlideRecord {
    fn from(slide: SvgSlideContent) -> Self {
        Self {
            index: slide.index,
            svg: slide.svg,
            width_px: slide.width_px,
            height_px: slide.height_px,
        }
    }
}

impl From<PresenterState> for PresenterStateRecord {
    fn from(state: PresenterState) -> Self {
        Self {
            loaded: state.loaded,
            file_path: state.file_path,
            current_slide: state.current_slide,
            total_slides: state.total_slides,
            render_mode: match state.render_mode {
                metocast_core::protocol::PresenterRenderMode::Text => "text",
                metocast_core::protocol::PresenterRenderMode::Svg => "svg",
            }
            .to_string(),
            slides: state.slides.into_iter().map(Into::into).collect(),
            svg_slides: state.svg_slides.into_iter().map(Into::into).collect(),
            muted: state.muted,
            slide_width_emu: state.slide_width_emu,
            slide_height_emu: state.slide_height_emu,
        }
    }
}

#[uniffi::export]
pub fn bridge_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_progress_reads_as_the_nearest_whole_percent() {
        assert_eq!(progress_label(0, 0), "");
        assert_eq!(progress_label(5, -1), "");
        assert_eq!(progress_label(0, 200), " 0%");
        assert_eq!(progress_label(1, 3), " 33%");
        assert_eq!(progress_label(2, 3), " 67%");
        assert_eq!(progress_label(200, 200), " 100%");
        assert_eq!(progress_label(900, 200), " 100%");
        assert_eq!(progress_label(-4, 200), " 0%");
        assert_eq!(progress_label(i64::MAX, i64::MAX), " 100%");
    }

    #[test]
    fn drafts_become_the_create_event_body_the_server_reads() {
        let draft = EventDraftRecord {
            title: "Sunday".to_string(),
            computed_title: "2026.09.20. Sunday".to_string(),
            date_time: "2026-09-20T08:00:00+00:00".to_string(),
            speaker: "Pastor".to_string(),
            description: String::new(),
            auto_upload_enabled: true,
            youtube_privacy: "unlisted".to_string(),
            bible_references: vec![BibleReferenceRecord {
                reference_type: "leckio".to_string(),
                reference: String::new(),
                translation: "RUF_v2".to_string(),
                verses: Vec::new(),
            }],
        };
        let body = serde_json::to_value(CreateEvent::try_from(draft.clone()).unwrap()).unwrap();
        assert_eq!(body["date_time"], "2026-09-20T08:00:00Z");
        assert_eq!(
            body["connections"],
            serde_json::json!([{"platform": "youtube", "privacy_status": "unlisted"}])
        );
        // An empty reference is how the server is told to delete one.
        assert_eq!(body["bible_references"][0]["reference"], "");

        let undated = EventDraftRecord {
            date_time: "tomorrow".to_string(),
            ..draft
        };
        assert!(matches!(
            CreateEvent::try_from(undated),
            Err(AppleError::InvalidInput)
        ));
    }

    #[test]
    fn production_controls_become_the_server_commands() {
        assert_eq!(
            ProductionCommand::try_from(ProductionControl::ObsStreaming { on: false }).unwrap(),
            ProductionCommand::ObsStreamStop
        );
        assert_eq!(
            ProductionCommand::try_from(ProductionControl::AtemRecording { on: true }).unwrap(),
            ProductionCommand::AtemRecordStart
        );
        assert!(matches!(
            ProductionCommand::try_from(ProductionControl::RodecasterRecordStart {
                event_id: Some("not an id".to_string())
            }),
            Err(AppleError::InvalidIdentifier)
        ));
        let event_id = uuid::Uuid::new_v4();
        assert_eq!(
            ProductionCommand::try_from(ProductionControl::TranscriptionStart {
                event_id: Some(event_id.to_string())
            })
            .unwrap(),
            ProductionCommand::TranscriptionStart {
                event_id: Some(event_id)
            }
        );
        // No event: the commands go out without one.
        assert_eq!(
            ProductionCommand::try_from(ProductionControl::TranscriptionStart { event_id: None })
                .unwrap(),
            ProductionCommand::TranscriptionStart { event_id: None }
        );
        assert_eq!(
            ProductionCommand::try_from(ProductionControl::RodecasterRecordStart {
                event_id: None
            })
            .unwrap(),
            ProductionCommand::RodecasterRecordStart { event_id: None }
        );
    }

    #[test]
    fn the_desk_recorder_controls_become_the_server_commands() {
        assert_eq!(
            ProductionCommand::try_from(ProductionControl::RodecasterDeskRecording { on: true })
                .unwrap(),
            ProductionCommand::RodecasterDeskRecordStart
        );
        assert_eq!(
            ProductionCommand::try_from(ProductionControl::RodecasterDeskRecording { on: false })
                .unwrap(),
            ProductionCommand::RodecasterDeskRecordStop
        );
    }

    fn service_start() -> AutomationRecord {
        AutomationRecord {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Service start".to_string(),
            enabled: true,
            triggers: vec![
                AutomationTrigger::MiddlecontrolPreset {
                    preset: 3,
                    camera_id: None,
                },
                AutomationTrigger::RodecasterChannel {
                    channel: 2,
                    label: "Wireless 1".to_string(),
                    live: true,
                    source: ChannelSource::Either,
                    hold_seconds: 0.0,
                },
            ],
            actions: vec![
                AutomationAction::PresetRecall {
                    preset: 3,
                    camera_id: None,
                    wait_for_arrival: true,
                },
                AutomationAction::RecordStart {
                    target: RecordTarget::Obs,
                },
                AutomationAction::RecordStart {
                    target: RecordTarget::Rodecaster,
                },
            ],
            cooldown_seconds: 5.0,
            require_event: false,
        }
    }

    #[test]
    fn a_rule_crosses_the_bridge_and_comes_back_the_same() {
        let rule = service_start();
        let draft = AutomationDraft::from(rule.clone());
        assert_eq!(draft.validate(), Ok(()));
        let back = AutomationRecord::from(draft.clone());
        // The id is the server's to give.
        assert_eq!(back.id, "");
        assert_eq!(AutomationDraft::from(back), draft);
    }

    #[test]
    fn the_editor_can_ask_what_is_wrong_with_a_rule() {
        assert_eq!(automation_problem(service_start()), None);
        let mut bad = service_start();
        bad.triggers = vec![AutomationTrigger::MiddlecontrolPreset {
            preset: 13,
            camera_id: None,
        }];
        assert_eq!(
            automation_problem(bad).as_deref(),
            Some("preset must be between 1 and 12")
        );
        let mut nameless = service_start();
        nameless.name = " ".to_string();
        assert!(automation_problem(nameless).is_some());
    }

    #[test]
    fn the_reverse_of_a_rule_has_no_id_and_stops_before_it_moves() {
        let reversed = reverse_automation(service_start(), Some(1));
        assert_eq!(reversed.id, "");
        assert_eq!(reversed.name, "Service start (reverse)");
        assert_eq!(
            reversed.actions,
            vec![
                AutomationAction::RecordStop {
                    target: RecordTarget::Rodecaster
                },
                AutomationAction::RecordStop {
                    target: RecordTarget::Obs
                },
                AutomationAction::PresetRecall {
                    preset: 1,
                    camera_id: None,
                    wait_for_arrival: true
                },
            ]
        );
        assert!(matches!(
            reversed.triggers.as_slice(),
            [AutomationTrigger::RodecasterChannel { live: false, hold_seconds, .. }] if *hold_seconds >= 5.0
        ));
    }

    #[test]
    fn automation_messages_reach_swift_as_records() {
        let changed: ServerEvent = serde_json::from_value(serde_json::json!({
            "type": "automations.changed", "armed": false,
            "automations": [{
                "id": "6f2c1f8e-9a3e-4a8b-9f6e-2d8a1b7c3e10", "name": "Service start",
                "enabled": true,
                "triggers": [{"kind": "middlecontrol.preset", "preset": 3, "cameraId": null}],
                "actions": [{"kind": "record.start", "target": {"device": "obs"}}],
                "cooldownSeconds": 5.0, "requireEvent": false,
                "createdAt": "2026-10-07T08:00:00Z", "updatedAt": "2026-10-07T08:00:00Z"
            }]
        }))
        .unwrap();
        let AppleEvent::AutomationsChanged { armed, automations } = changed.into() else {
            panic!("not automations.changed");
        };
        assert!(!armed);
        assert_eq!(automations[0].id, "6f2c1f8e-9a3e-4a8b-9f6e-2d8a1b7c3e10");

        let run: ServerEvent = serde_json::from_value(serde_json::json!({
            "type": "automation.run",
            "run": {
                "id": "0d9c4b7a-1e2f-4a3b-8c5d-6e7f8a9b0c1d",
                "automationId": "6f2c1f8e-9a3e-4a8b-9f6e-2d8a1b7c3e10",
                "name": "Service start", "cause": "Preset 3 finished on camera 1",
                "manual": false, "startedAt": "2026-10-07T08:00:00Z",
                "results": [{"action": "Start OBS", "outcome": "failed",
                             "message": "obs_not_connected"}]
            }
        }))
        .unwrap();
        let AppleEvent::AutomationRun { run } = run.into() else {
            panic!("not automation.run");
        };
        assert_eq!(run.cause, "Preset 3 finished on camera 1");
        assert!(matches!(run.results[0].outcome, ActionOutcome::Failed));
        assert_eq!(run.started_at, "2026-10-07T08:00:00+00:00");

        let desk: ServerEvent = serde_json::from_value(serde_json::json!({
            "type": "rodecaster.record.state",
            "state": {"status": "recording", "mediaPresent": true}
        }))
        .unwrap();
        let AppleEvent::RodecasterDeskRecorder { state } = desk.into() else {
            panic!("not rodecaster.record.state");
        };
        assert!(matches!(state.status, DeskRecorderStatus::Recording));
        assert!(state.media_present);
    }

    #[test]
    fn a_transcript_snapshot_keeps_the_open_line_or_else_the_last_one() {
        let event: ServerEvent = serde_json::from_value(serde_json::json!({
            "type": "transcription.snapshot",
            "state": {"status": "running", "reason": null, "sessionId": null, "eventId": null,
                      "startedAt": null,
                      "youtube": {"ok": false, "lastSeq": 3, "lastError": "HTTP 403"}},
            "segments": [{"seq": 1, "startedAt": "2026-10-05T09:00:00Z",
                          "endedAt": "2026-10-05T09:00:02Z", "text": "Amen."}],
            "partial": "",
            "credit": null
        }))
        .unwrap();
        let AppleEvent::TranscriptionSnapshot {
            state,
            line,
            credit,
        } = event.into()
        else {
            panic!("not a snapshot");
        };
        assert!(matches!(state.status, TranscriptionStatus::Running));
        assert_eq!(state.youtube_error.as_deref(), Some("HTTP 403"));
        assert_eq!(line, "Amen.");
        assert!(credit.is_none());
        let empty: ServerEvent = serde_json::from_value(
            serde_json::json!({"type": "transcription.partial", "text": ""}),
        )
        .unwrap();
        assert!(matches!(AppleEvent::from(empty), AppleEvent::Unknown));
    }

    #[test]
    fn invalid_configuration_has_a_typed_error() {
        assert!(matches!(
            AppleClient::new("not a URL".to_string(), "secret".to_string()),
            Err(AppleError::InvalidConfiguration)
        ));
    }
}

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    automations::{Automation, AutomationRun, RecordTarget, RecorderRow, StopReason},
    connectors::{
        ConnectorState, ConnectorStatus, RodecasterDeskRecorder, RodecasterProfile,
        RodecasterRecorderState,
    },
    events::{Event, EventSummary},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum PresenterCommand {
    #[serde(rename = "presenter.status")]
    Status,
    #[serde(rename = "presenter.next")]
    Next,
    #[serde(rename = "presenter.prev")]
    Previous,
    #[serde(rename = "presenter.first")]
    First,
    #[serde(rename = "presenter.last")]
    Last,
    #[serde(rename = "presenter.goto")]
    GoTo { slide: u32 },
    #[serde(rename = "presenter.mute")]
    Mute,
    #[serde(rename = "presenter.unmute")]
    Unmute,
}

/// Live production commands. Each is answered by `ok` or `error`; the resulting state arrives
/// separately (`connector.state`, `rodecaster.mute`, `rodecaster.audio.record.state`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum ProductionCommand {
    #[serde(rename = "obs.stream.start")]
    ObsStreamStart,
    #[serde(rename = "obs.stream.stop")]
    ObsStreamStop,
    #[serde(rename = "obs.record.start")]
    ObsRecordStart,
    #[serde(rename = "obs.record.stop")]
    ObsRecordStop,
    #[serde(rename = "atem.program.set")]
    AtemProgram { input: u16 },
    #[serde(rename = "atem.preview.set")]
    AtemPreview { input: u16 },
    #[serde(rename = "atem.cut")]
    AtemCut,
    #[serde(rename = "atem.auto")]
    AtemAuto,
    #[serde(rename = "atem.stream.start")]
    AtemStreamStart,
    #[serde(rename = "atem.stream.stop")]
    AtemStreamStop,
    #[serde(rename = "atem.record.start")]
    AtemRecordStart,
    #[serde(rename = "atem.record.stop")]
    AtemRecordStop,
    #[serde(rename = "middlecontrol.camera.select")]
    MiddlecontrolCamera { camera_id: u8 },
    #[serde(rename = "middlecontrol.record.start_all")]
    MiddlecontrolRecordStartAll,
    #[serde(rename = "middlecontrol.record.stop_all")]
    MiddlecontrolRecordStopAll,
    /// Answered by `rodecaster.profile`.
    #[serde(rename = "rodecaster.profile")]
    RodecasterProfile,
    #[serde(rename = "rodecaster.mute.set")]
    RodecasterMute { channel: u32, mute: bool },
    /// Without an event the server keeps the files as untracked recordings.
    #[serde(rename = "rodecaster.audio.record.start")]
    RodecasterRecordStart {
        #[serde(skip_serializing_if = "Option::is_none")]
        event_id: Option<Uuid>,
    },
    #[serde(rename = "rodecaster.audio.record.stop")]
    RodecasterRecordStop,
    /// The desk's own SD or USB recorder, not Metocast's audio recording. The result arrives
    /// as `rodecaster.record.state`; `rodecaster_no_media` when no card or drive is there.
    #[serde(rename = "rodecaster.record.start")]
    RodecasterDeskRecordStart,
    #[serde(rename = "rodecaster.record.stop")]
    RodecasterDeskRecordStop,
    #[serde(rename = "blackmagic-camera.record.start")]
    CameraRecordStart,
    #[serde(rename = "blackmagic-camera.record.stop")]
    CameraRecordStop,
    #[serde(rename = "blackmagic-camera.stream.start")]
    CameraStreamStart,
    #[serde(rename = "blackmagic-camera.stream.stop")]
    CameraStreamStop,
    /// Copies the channel's ingestion address and key into the camera; does not go live.
    #[serde(rename = "blackmagic-camera.stream.push_youtube")]
    CameraPushYoutube,
    /// The same for the ATEM's streaming service.
    #[serde(rename = "atem.stream.push_youtube")]
    AtemPushYoutube,
    /// Answered by `transcription.state`, or `error` with `transcription_unavailable` /
    /// `transcription_not_configured`.
    #[serde(rename = "transcription.start")]
    TranscriptionStart {
        #[serde(rename = "eventId", skip_serializing_if = "Option::is_none")]
        event_id: Option<Uuid>,
    },
    #[serde(rename = "transcription.stop")]
    TranscriptionStop,
    /// Starts every recorder the Record button is set to; each one's result arrives in
    /// `recorders.state`.
    #[serde(rename = "recorders.start")]
    RecordersStart,
    #[serde(rename = "recorders.stop")]
    RecordersStop,
}

/// Automations. `automations.list` is answered by `automations.changed`; the others by `ok` or
/// `error`, and a changed list or a finished run arrives as a broadcast.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum AutomationCommand {
    #[serde(rename = "automations.list")]
    List,
    /// Runs the rule now, ignoring its cooldown, its event check and the master switch. The
    /// run arrives as `automation.run`.
    #[serde(rename = "automations.run")]
    Run { id: Uuid },
    #[serde(rename = "automations.set_armed")]
    SetArmed { armed: bool },
}

/// The OBS sources the server watches and warns about. Each is answered by
/// `obs.listeners.list` or `obs.devices.available`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum DeviceAlertCommand {
    #[serde(rename = "obs.listeners.list")]
    List,
    #[serde(rename = "obs.devices.scan")]
    Scan,
    #[serde(rename = "obs.devices.available")]
    Available,
    #[serde(rename = "obs.listeners.create")]
    Create {
        connector_type: String,
        category: String,
        device_item_value: String,
        device_item_name: String,
        friendly_name: String,
    },
    #[serde(rename = "obs.listeners.delete")]
    Delete { id: Uuid },
}

/// The visual treatment of text-mode song and Bible slides.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PresenterTheme {
    #[default]
    Classic,
    Editorial,
}

impl PresenterTheme {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Editorial => "editorial",
        }
    }
}

impl std::str::FromStr for PresenterTheme {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "classic" => Ok(Self::Classic),
            "editorial" => Ok(Self::Editorial),
            _ => Err(()),
        }
    }
}

/// Slides and the presentation backend. `presentation.*` routes to the web presenter or to
/// Keynote depending on the server's setting, so one command drives either.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum PresentationCommand {
    #[serde(rename = "presentation.open")]
    Open { file_path: String },
    #[serde(rename = "presentation.close")]
    Close,
    #[serde(rename = "presentation.start")]
    Start,
    #[serde(rename = "presentation.stop")]
    Stop,
    /// Answered by `presentation.settings`.
    #[serde(rename = "presentation.get_settings")]
    GetSettings,
    /// Answered by `presentation.status`.
    #[serde(rename = "presentation.status")]
    Status,
    #[serde(rename = "presentation.set_use_web_presenter")]
    SetUseWebPresenter { enabled: bool },
    #[serde(rename = "presentation.set_presenter_theme")]
    SetPresenterTheme { theme: PresenterTheme },
    /// Logs each slide sent to every screen, and each screen's `presenter.ack`, on the server.
    #[serde(rename = "presentation.set_delivery_log")]
    SetDeliveryLog { enabled: bool },
    #[serde(rename = "presentation.next")]
    Next,
    #[serde(rename = "presentation.prev")]
    Previous,
    #[serde(rename = "presentation.first")]
    First,
    #[serde(rename = "presentation.last")]
    Last,
    #[serde(rename = "presentation.goto")]
    GoTo { slide: u32 },
    #[serde(rename = "presentation.mute")]
    Mute,
    #[serde(rename = "presentation.unmute")]
    Unmute,
    /// Shows a step of an event's Order of Service. Every client hears `order.state`.
    #[serde(rename = "order.go")]
    OrderGo { event_id: String, item_id: String },
    /// Moves to the next step; with none on screen, `event_id` names the event to start.
    #[serde(rename = "order.next")]
    OrderNext { event_id: Option<String> },
    #[serde(rename = "order.previous")]
    OrderPrevious { event_id: Option<String> },
    /// Names this client in everyone's connected-devices list.
    #[serde(rename = "presenter.register")]
    Register {
        label: String,
        hostname: Option<String>,
    },
    /// This screen shows slide `slide` since `shown_at` (RFC 3339, this device's clock).
    #[serde(rename = "presenter.ack")]
    Ack { slide: u32, shown_at: String },
    /// Answered by `clients.list`.
    #[serde(rename = "clients.list")]
    ClientsList,
    /// Pings one connected client by `client_id`; its latency arrives in `clients.updated`.
    #[serde(rename = "clients.ping")]
    ClientsPing { client_id: String },
}

/// `presentation.status`: the active backend, whichever it is.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PresentationStatus {
    pub app_running: bool,
    pub slideshow_active: bool,
    pub current_slide: Option<u32>,
    pub total_slides: Option<u32>,
    pub document_name: Option<String>,
    #[serde(default)]
    pub blanked: bool,
}

/// One WebSocket client in `clients.list` / `clients.updated`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedClient {
    pub id: Uuid,
    pub label: String,
    pub user_agent: Option<String>,
    pub hostname: Option<String>,
    pub connected_at: DateTime<Utc>,
    #[serde(default)]
    pub last_pong_at: Option<DateTime<Utc>>,
    pub latency_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PresenterRenderMode {
    #[default]
    Text,
    Svg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParagraphContent {
    #[serde(default)]
    pub lines: Vec<String>,
    pub align: String,
    #[serde(default)]
    pub font_size_pt: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlideContent {
    pub index: u32,
    pub paragraphs: Vec<ParagraphContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SvgSlideContent {
    pub index: u32,
    pub svg: String,
    pub width_px: u32,
    pub height_px: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenterState {
    pub loaded: bool,
    pub file_path: Option<String>,
    pub current_slide: u32,
    pub total_slides: u32,
    #[serde(default)]
    pub render_mode: PresenterRenderMode,
    #[serde(default)]
    pub slides: Vec<SlideContent>,
    #[serde(default)]
    pub svg_slides: Vec<SvgSlideContent>,
    pub muted: bool,
    #[serde(default = "default_slide_width")]
    pub slide_width_emu: u64,
    #[serde(default = "default_slide_height")]
    pub slide_height_emu: u64,
}

/// Largest WebSocket message the server sends and the Rust clients accept. A web presenter
/// deck travels as one `presenter.state` message with every slide's SVG and images inlined,
/// so this caps the size of a deck the presenter can show.
pub const MAX_WS_MESSAGE_BYTES: usize = 64 << 20;

const fn default_slide_width() -> u64 {
    12_192_000
}

const fn default_slide_height() -> u64 {
    6_858_000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerEvent {
    #[serde(rename = "connected")]
    Connected {
        #[serde(rename = "serverId")]
        server_id: String,
    },
    #[serde(rename = "connector.status")]
    ConnectorStatus {
        connector: String,
        status: ConnectorStatus,
    },
    /// The server sends one top-level key per connector next to `type`.
    #[serde(rename = "connectors.status")]
    ConnectorStatuses {
        #[serde(flatten)]
        statuses: HashMap<String, ConnectorStatus>,
    },
    #[serde(rename = "events.list")]
    EventsList { events: Vec<EventSummary> },
    #[serde(rename = "events.get")]
    Event { event: Event },
    #[serde(rename = "presenter.state")]
    PresenterState { state: PresenterState },
    /// Sent instead of `presenter.state` when only the slide moved.
    #[serde(rename = "presenter.slide_changed")]
    PresenterSlideChanged {
        #[serde(rename = "currentSlide")]
        current_slide: u32,
        #[serde(rename = "totalSlides")]
        total_slides: u32,
    },
    /// An event row was inserted, updated or deleted. Its `data` holds the raw row when it
    /// comes from a database trigger, so clients reload the list instead of reading it.
    #[serde(rename = "event.changed")]
    EventChanged {},
    /// The step on screen; `position` is `null` when none is.
    #[serde(rename = "order.state")]
    OrderState {
        position: Option<crate::events::OrderPosition>,
    },
    /// An event's Order of Service was replaced. Clients reload it.
    #[serde(rename = "event.order.changed")]
    EventOrderChanged {
        #[serde(rename = "eventId")]
        event_id: uuid::Uuid,
    },
    /// The slide library index started (`pending` decks to parse) or finished (0) parsing.
    #[serde(rename = "ppt.index.state")]
    PptIndexState { indexing: bool, pending: u32 },
    #[serde(rename = "notification")]
    Notification { level: String, message: String },
    #[serde(rename = "connector.state")]
    ConnectorState(ConnectorState),
    #[serde(rename = "rodecaster.profile")]
    RodecasterProfile { profile: RodecasterProfile },
    /// A channel was muted on the desk, or on its Wireless PRO transmitter when `remote`.
    #[serde(rename = "rodecaster.mute")]
    RodecasterMute {
        channel: u32,
        label: String,
        muted: bool,
        remote: bool,
        /// The desk's "notify on mute" setting is on.
        notify: bool,
    },
    #[serde(rename = "rodecaster.audio.record.state")]
    RodecasterRecorder { state: RodecasterRecorderState },
    /// The desk's own SD or USB recorder, as opposed to Metocast's audio recording above.
    #[serde(rename = "rodecaster.record.state")]
    RodecasterDeskRecorder { state: RodecasterDeskRecorder },
    /// The rules or the master switch changed. Also the answer to `automations.list`.
    #[serde(rename = "automations.changed")]
    AutomationsChanged {
        armed: bool,
        automations: Vec<Automation>,
    },
    /// A rule ran: what fired it and what each action came to.
    #[serde(rename = "automation.run")]
    AutomationRun { run: AutomationRun },
    /// Every recorder the Record button can drive, whenever one of them changes. Also the
    /// answer to `GET /api/recorders`.
    #[serde(rename = "recorders.state")]
    RecordersState { recorders: Vec<RecorderRow> },
    /// A recording stopped, for whatever reason. `error` is what the recorder said, if anything.
    #[serde(rename = "recorder.stopped")]
    RecorderStopped {
        target: RecordTarget,
        reason: StopReason,
        error: Option<String>,
    },
    #[serde(rename = "obs.devices.available")]
    ObsDevices {
        devices: crate::discovery::ObsDevices,
    },
    #[serde(rename = "rodecaster.audio.discovery")]
    AudioDiscovery {
        discovery: crate::discovery::AudioDiscovery,
    },
    /// A command this client sent failed. `message` is a stable code (`atem_not_connected`)
    /// or the device's own words.
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(rename = "presentation.settings")]
    PresentationSettings {
        #[serde(rename = "useWebPresenter")]
        use_web_presenter: bool,
        #[serde(rename = "presenterTheme")]
        presenter_theme: PresenterTheme,
        /// Older servers leave it out.
        #[serde(rename = "deliveryLog", default)]
        delivery_log: bool,
    },
    #[serde(rename = "presentation.status")]
    PresentationStatus { status: PresentationStatus },
    /// Reply to `clients.list`.
    #[serde(rename = "clients.list")]
    ClientsList { clients: Vec<ConnectedClient> },
    /// Broadcast whenever a client connects, leaves or is renamed.
    #[serde(rename = "clients.updated")]
    ClientsUpdated { clients: Vec<ConnectedClient> },
    #[serde(rename = "obs.listeners.list")]
    DeviceAlerts {
        listeners: Vec<crate::operations::DeviceListener>,
    },
    /// A render's progress: about once a second while it runs, and once when it ends.
    #[serde(rename = "media.render.progress")]
    MediaRenderProgress {
        progress: crate::media::RenderProgress,
    },
    /// An audio job's step and progress, and its report when it ends.
    #[serde(rename = "media.audio.progress")]
    MediaAudioProgress {
        progress: crate::media::AudioProgress,
    },
    /// A program job's step and progress, and its report when it ends or is nudged.
    #[serde(rename = "media.program.progress")]
    MediaProgramProgress {
        progress: crate::media::ProgramProgress,
    },
    #[serde(rename = "transcription.state")]
    TranscriptionState {
        state: crate::transcription::TranscriptionState,
    },
    /// The open line; empty clears it. Throttled to about one every 150 ms.
    #[serde(rename = "transcription.partial")]
    TranscriptionPartial { text: String },
    #[serde(rename = "transcription.segment")]
    TranscriptionSegment {
        segment: crate::transcription::TranscriptSegment,
    },
    /// Sent on connect: the state, the last ~20 segments and the credit line.
    #[serde(rename = "transcription.snapshot")]
    TranscriptionSnapshot {
        state: crate::transcription::TranscriptionState,
        segments: Vec<crate::transcription::TranscriptSegment>,
        partial: String,
        credit: Option<crate::transcription::CreditStatus>,
    },
    #[serde(rename = "transcription.credit")]
    TranscriptionCredit {
        credit: crate::transcription::CreditStatus,
    },
    /// A stored transcript's AI clean-up ran a step, finished or failed.
    #[serde(rename = "transcript.cleanup.state")]
    TranscriptCleanupState {
        #[serde(flatten)]
        progress: crate::transcription::CleanupProgress,
    },
    #[serde(rename = "ping")]
    Ping {
        #[serde(rename = "pingId")]
        ping_id: i64,
    },
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presenter_command_matches_existing_protocol() {
        assert_eq!(
            serde_json::to_string(&PresenterCommand::GoTo { slide: 3 }).unwrap(),
            r#"{"type":"presenter.goto","slide":3}"#
        );
    }

    #[test]
    fn production_commands_match_the_server_wire_names() {
        let json = |command: ProductionCommand| serde_json::to_value(command).unwrap();
        let event_id = Uuid::nil();
        assert_eq!(
            json(ProductionCommand::TranscriptionStart {
                event_id: Some(event_id)
            }),
            serde_json::json!({"type": "transcription.start", "eventId": event_id})
        );
        assert_eq!(
            json(ProductionCommand::TranscriptionStart { event_id: None }),
            serde_json::json!({"type": "transcription.start"})
        );
        assert_eq!(
            json(ProductionCommand::ObsStreamStart),
            serde_json::json!({"type": "obs.stream.start"})
        );
        assert_eq!(
            json(ProductionCommand::AtemProgram { input: 3 }),
            serde_json::json!({"type": "atem.program.set", "input": 3})
        );
        assert_eq!(
            json(ProductionCommand::MiddlecontrolCamera { camera_id: 2 }),
            serde_json::json!({"type": "middlecontrol.camera.select", "camera_id": 2})
        );
        assert_eq!(
            json(ProductionCommand::RodecasterMute {
                channel: 1,
                mute: true
            }),
            serde_json::json!({"type": "rodecaster.mute.set", "channel": 1, "mute": true})
        );
        let event_id = Uuid::nil();
        assert_eq!(
            json(ProductionCommand::RodecasterRecordStart {
                event_id: Some(event_id)
            }),
            serde_json::json!({"type": "rodecaster.audio.record.start", "event_id": event_id})
        );
        assert_eq!(
            json(ProductionCommand::RodecasterRecordStart { event_id: None }),
            serde_json::json!({"type": "rodecaster.audio.record.start"})
        );
        assert_eq!(
            json(ProductionCommand::RodecasterDeskRecordStart),
            serde_json::json!({"type": "rodecaster.record.start"})
        );
        assert_eq!(
            json(ProductionCommand::RodecasterDeskRecordStop),
            serde_json::json!({"type": "rodecaster.record.stop"})
        );
    }

    #[test]
    fn automation_commands_match_the_server_wire_names() {
        let json = |command: AutomationCommand| serde_json::to_value(command).unwrap();
        assert_eq!(
            json(AutomationCommand::List),
            serde_json::json!({"type": "automations.list"})
        );
        assert_eq!(
            json(AutomationCommand::Run { id: Uuid::nil() }),
            serde_json::json!({"type": "automations.run", "id": Uuid::nil()})
        );
        assert_eq!(
            json(AutomationCommand::SetArmed { armed: false }),
            serde_json::json!({"type": "automations.set_armed", "armed": false})
        );
    }

    #[test]
    fn automation_messages_decode_the_server_shapes() {
        let changed = r#"{"type":"automations.changed","armed":true,"automations":[{
            "id":"6f2c1f8e-9a3e-4a8b-9f6e-2d8a1b7c3e10","name":"Service start","enabled":true,
            "triggers":[{"kind":"middlecontrol.preset","preset":3,"cameraId":null}],
            "actions":[{"kind":"record.start","target":{"device":"obs"}}],
            "cooldownSeconds":5.0,"requireEvent":false,
            "createdAt":"2026-10-07T08:00:00Z","updatedAt":"2026-10-07T08:00:00Z"}]}"#;
        let ServerEvent::AutomationsChanged { armed, automations } =
            serde_json::from_str(changed).unwrap()
        else {
            panic!("expected automations.changed");
        };
        assert!(armed);
        assert_eq!(automations[0].name, "Service start");

        let run = r#"{"type":"automation.run","run":{
            "id":"0d9c4b7a-1e2f-4a3b-8c5d-6e7f8a9b0c1d",
            "automationId":"6f2c1f8e-9a3e-4a8b-9f6e-2d8a1b7c3e10","name":"Service start",
            "cause":"Preset 3 finished","manual":false,"startedAt":"2026-10-07T08:00:00Z",
            "results":[{"action":"Start OBS","outcome":"done","message":null}]}}"#;
        let ServerEvent::AutomationRun { run } = serde_json::from_str(run).unwrap() else {
            panic!("expected automation.run");
        };
        assert_eq!(run.cause, "Preset 3 finished");
        assert!(!run.failed());

        let desk = r#"{"type":"rodecaster.record.state","state":{"status":"paused","mediaPresent":false}}"#;
        let ServerEvent::RodecasterDeskRecorder { state } = serde_json::from_str(desk).unwrap()
        else {
            panic!("expected rodecaster.record.state");
        };
        assert_eq!(state.status, crate::connectors::DeskRecorderStatus::Paused);
        assert!(!state.media_present);
    }

    #[test]
    fn connector_state_decodes_each_connector_shape() {
        // Shapes sent by server/mod.rs and on connect by websocket.rs.
        let decode = |json: &str| match serde_json::from_str(json).unwrap() {
            ServerEvent::ConnectorState(state) => state,
            other => panic!("expected connector.state, got {other:?}"),
        };
        assert_eq!(
            decode(
                r#"{"type":"connector.state","connector":"obs","isStreaming":true,"isRecording":false}"#
            ),
            ConnectorState::Obs {
                is_streaming: true,
                is_recording: false
            }
        );
        let ConnectorState::Atem { state: Some(atem) } = decode(
            r#"{"type":"connector.state","connector":"atem","state":{
                "product":"ATEM Mini Pro","program":1,"preview":2,
                "inputs":[{"id":1,"name":"Camera 1","shortName":"CAM1"}],
                "streaming":"streaming","recording":null,"streamService":"YouTube"}}"#,
        ) else {
            panic!("expected an ATEM state");
        };
        assert_eq!(atem.inputs[0].short_name, "CAM1");
        assert_eq!(
            atem.streaming,
            Some(crate::connectors::StreamStatus::Streaming)
        );
        assert_eq!(atem.recording, None);
        let ConnectorState::Middlecontrol {
            state: Some(middle),
        } = decode(
            r#"{"type":"connector.state","connector":"middlecontrol","state":{
                "selectedCamera":2,"recording":false,"recordingCameraIds":[],
                "connectedCameraIds":[1,2],"connectedApcrIds":null,"presetMoveActive":null}}"#,
        )
        else {
            panic!("expected a Middle Control state");
        };
        assert_eq!(middle.connected_camera_ids, Some(vec![1, 2]));
        assert_eq!(
            decode(
                r#"{"type":"connector.state","connector":"blackmagic-camera","isStreaming":false,"streamStatus":"Idle","isRecording":false}"#
            ),
            ConnectorState::Camera {
                is_streaming: false,
                is_recording: false,
                stream_status: "Idle".to_string(),
                has_media: true,
                destination: None
            }
        );
        let ConnectorState::Camera { destination, .. } = decode(
            r#"{"type":"connector.state","connector":"blackmagic-camera","isStreaming":true,"streamStatus":"Streaming","isRecording":false,
                "destination":{"service":"youtube","url":"rtmp://a.rtmp.youtube.com/…","broadcast":"Vasárnapi istentisztelet","matchesYoutube":true}}"#,
        ) else {
            panic!("expected a camera state");
        };
        assert_eq!(
            destination.unwrap().broadcast.as_deref(),
            Some("Vasárnapi istentisztelet")
        );
        assert_eq!(
            decode(r#"{"type":"connector.state","connector":"vmix","isStreaming":true}"#),
            ConnectorState::Other
        );
    }

    #[test]
    fn device_alert_commands_match_the_server_wire_names() {
        assert_eq!(
            serde_json::to_value(DeviceAlertCommand::Create {
                connector_type: "obs".to_string(),
                category: "audio_input".to_string(),
                device_item_value: "mic-1".to_string(),
                device_item_name: "Mic".to_string(),
                friendly_name: "Pulpit mic".to_string(),
            })
            .unwrap(),
            serde_json::json!({"type": "obs.listeners.create", "connector_type": "obs",
                               "category": "audio_input", "device_item_value": "mic-1",
                               "device_item_name": "Mic", "friendly_name": "Pulpit mic"})
        );
        assert_eq!(
            serde_json::to_value(ProductionCommand::CameraPushYoutube).unwrap(),
            serde_json::json!({"type": "blackmagic-camera.stream.push_youtube"})
        );
    }

    #[test]
    fn rodecaster_messages_decode_the_server_shapes() {
        let profile = r#"{"type":"rodecaster.profile","profile":{"model":"RØDECaster Pro II",
            "firmware":"1.6.0","channels":[{"channel":0,"label":"Host","source":3,"level":0.5,
            "mute":false,"wirelessMute":true,"cue":false,"processing":["compressor"],"settings":[]}]}}"#;
        let ServerEvent::RodecasterProfile { profile } = serde_json::from_str(profile).unwrap()
        else {
            panic!("expected rodecaster.profile");
        };
        assert_eq!(profile.channels[0].label, "Host");
        assert!(profile.channels[0].wireless_mute);

        let mute = r#"{"type":"rodecaster.mute","channel":2,"label":"Pastor","muted":true,"remote":true,"notify":false}"#;
        assert!(matches!(
            serde_json::from_str(mute).unwrap(),
            ServerEvent::RodecasterMute {
                channel: 2,
                muted: true,
                remote: true,
                ..
            }
        ));

        let recorder = r#"{"type":"rodecaster.audio.record.state","state":{"status":"recording",
            "sessionId":"6f2c1f8e-9a3e-4a8b-9f6e-2d8a1b7c3e10","eventId":"0d9c4b7a-1e2f-4a3b-8c5d-6e7f8a9b0c1d",
            "outputMode":"mainMix","startedAt":"2026-09-20T08:00:00Z","endpoint":{"name":"Main"},
            "error":null,"partialFiles":["a.flac.partial"],"finalizedFiles":[]}}"#;
        let ServerEvent::RodecasterRecorder { state } = serde_json::from_str(recorder).unwrap()
        else {
            panic!("expected rodecaster.audio.record.state");
        };
        assert_eq!(state.status, crate::connectors::RecorderStatus::Recording);
        assert!(state.event_id.is_some() && state.started_at.is_some());

        let error = r#"{"type":"error","message":"atem_not_connected"}"#;
        assert!(matches!(
            serde_json::from_str(error).unwrap(),
            ServerEvent::Error { message } if message == "atem_not_connected"
        ));
    }

    #[test]
    fn presentation_messages_decode_the_server_shapes() {
        // Shapes from websocket.rs `make_presentation_settings` / `make_presentation_status`.
        let settings = r#"{"type":"presentation.settings","useWebPresenter":false,"presenterTheme":"editorial"}"#;
        assert!(matches!(
            serde_json::from_str(settings).unwrap(),
            ServerEvent::PresentationSettings {
                use_web_presenter: false,
                presenter_theme: PresenterTheme::Editorial,
                delivery_log: false,
            }
        ));

        let status = r#"{"type":"presentation.status","status":{"appRunning":true,
            "slideshowActive":true,"currentSlide":3,"totalSlides":12,
            "documentName":"Song.key","blanked":false}}"#;
        let ServerEvent::PresentationStatus { status } = serde_json::from_str(status).unwrap()
        else {
            panic!("expected presentation.status");
        };
        assert_eq!(status.document_name.as_deref(), Some("Song.key"));
        assert_eq!(
            (status.current_slide, status.total_slides),
            (Some(3), Some(12))
        );

        let clients = r#"{"type":"clients.updated","clients":[{"id":"6f2c1f8e-9a3e-4a8b-9f6e-2d8a1b7c3e10",
            "label":"Sanctum","userAgent":"Mozilla/5.0","hostname":"mac.local",
            "connectedAt":"2026-09-20T08:00:00Z","lastPongAt":null,"latencyMs":12}]}"#;
        let ServerEvent::ClientsUpdated { clients } = serde_json::from_str(clients).unwrap() else {
            panic!("expected clients.updated");
        };
        assert_eq!(
            (clients[0].label.as_str(), clients[0].latency_ms),
            ("Sanctum", Some(12))
        );
    }

    #[test]
    fn presentation_commands_match_the_server_wire_names() {
        assert_eq!(
            serde_json::to_value(PresentationCommand::Open {
                file_path: "/Decks/Song.pptx".to_string()
            })
            .unwrap(),
            serde_json::json!({"type": "presentation.open", "file_path": "/Decks/Song.pptx"})
        );
        assert_eq!(
            serde_json::to_value(PresentationCommand::SetPresenterTheme {
                theme: PresenterTheme::Editorial
            })
            .unwrap(),
            serde_json::json!({"type": "presentation.set_presenter_theme", "theme": "editorial"})
        );
        assert_eq!(
            serde_json::to_value(PresentationCommand::Register {
                label: "iPhone".to_string(),
                hostname: None
            })
            .unwrap(),
            serde_json::json!({"type": "presenter.register", "label": "iPhone", "hostname": null})
        );
        assert_eq!(
            serde_json::to_value(PresentationCommand::Ack {
                slide: 2,
                shown_at: "2026-11-08T10:15:02.123Z".to_string()
            })
            .unwrap(),
            serde_json::json!({"type": "presenter.ack", "slide": 2, "shown_at": "2026-11-08T10:15:02.123Z"})
        );
    }

    #[test]
    fn connector_statuses_decode_the_flat_server_message() {
        // Shape sent by the server's `connectors.status` reply (websocket.rs).
        let json = r#"{
            "type": "connectors.status",
            "obs": {"type": "connected"},
            "atem": {"type": "error", "message": "unreachable"},
            "blackmagic-camera": {"type": "disconnected"}
        }"#;
        let ServerEvent::ConnectorStatuses { statuses } = serde_json::from_str(json).unwrap()
        else {
            panic!("expected connectors.status");
        };
        assert_eq!(statuses.len(), 3);
        assert_eq!(statuses["obs"], ConnectorStatus::Connected);
        assert_eq!(
            statuses["atem"],
            ConnectorStatus::Error {
                message: "unreachable".to_string()
            }
        );
        assert_eq!(statuses["blackmagic-camera"], ConnectorStatus::Disconnected);
    }

    #[test]
    fn broadcasts_decode_the_server_messages() {
        // Shapes sent by websocket.rs and scheduler/mod.rs.
        let changed =
            r#"{"type":"event.changed","data":{"operation":"UPDATE","record":{"id":"x"}}}"#;
        assert!(matches!(
            serde_json::from_str(changed).unwrap(),
            ServerEvent::EventChanged {}
        ));
        let slide = r#"{"type":"presenter.slide_changed","currentSlide":3,"totalSlides":9}"#;
        assert!(matches!(
            serde_json::from_str(slide).unwrap(),
            ServerEvent::PresenterSlideChanged {
                current_slide: 3,
                total_slides: 9
            }
        ));
        let notification = r#"{"type":"notification","level":"error","message":"Upload failed"}"#;
        let ServerEvent::Notification { level, message } =
            serde_json::from_str(notification).unwrap()
        else {
            panic!("expected notification");
        };
        assert_eq!(
            (level.as_str(), message.as_str()),
            ("error", "Upload failed")
        );
        let render = r#"{"type":"media.render.progress","progress":{"jobId":"6f2c1f8e-9a3e-4a8b-9f6e-2d8a1b7c3e10",
            "state":"running","frame":240,"totalFrames":41412,"fps":11.5,
            "outputPath":"/Volumes/A002/Metocast Renders/Untitled CAM 1 05.mov","error":null}}"#;
        let ServerEvent::MediaRenderProgress { progress } = serde_json::from_str(render).unwrap()
        else {
            panic!("expected media.render.progress");
        };
        assert_eq!(progress.state, crate::media::RenderState::Running);
        assert_eq!((progress.frame, progress.total_frames), (240, 41412));
        let audio = r#"{"type":"media.audio.progress","progress":{"jobId":"6f2c1f8e-9a3e-4a8b-9f6e-2d8a1b7c3e10",
            "state":"running","step":"clean","fraction":null,
            "outputPath":"/Users/operator/Music/Metocast Audio/00015_Wireless_PRO.wav","error":null,"report":null}}"#;
        let ServerEvent::MediaAudioProgress { progress } = serde_json::from_str(audio).unwrap()
        else {
            panic!("expected media.audio.progress");
        };
        assert_eq!(progress.step, Some(crate::media::AudioStep::Clean));
        assert_eq!(progress.fraction, None);
        let program = r#"{"type":"media.program.progress","progress":{"jobId":"6f2c1f8e-9a3e-4a8b-9f6e-2d8a1b7c3e10",
            "state":"running","step":"sync","fraction":0.5,
            "outputPath":"/Volumes/A002/Metocast Renders/Untitled CAM 1 05.mov","error":null,"report":null}}"#;
        let ServerEvent::MediaProgramProgress { progress } = serde_json::from_str(program).unwrap()
        else {
            panic!("expected media.program.progress");
        };
        assert_eq!(progress.step, Some(crate::media::ProgramStep::Sync));
        assert_eq!(progress.fraction, Some(0.5));
    }
}

//! MCP server (Model Context Protocol, Streamable HTTP) mounted at `POST /mcp`.
//!
//! Plain JSON-RPC, no SDK. Tools run the code the UIs already use: REST operations go through
//! the real `/api` router (same auth layer, validation and errors), WebSocket commands through
//! `websocket::run_command`. Tool schemas come from the code: `schemars` on `WsCommand` and the
//! OpenAPI spec. State resources, prompts and the manual are in `mcp_content.rs`; subscriptions
//! are answered over `GET /mcp` as server-sent events. See plans/PLAN-mcp-server.md and
//! docs/mcp/README.md.

use metocast_core::best_effort::LogGone;
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::ws::Message;
use axum::{
    Router,
    body::Body,
    extract::{ConnectInfo, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::post,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::{Notify, broadcast, watch};
use tower::ServiceExt;
use uuid::Uuid;

use super::websocket::{self, WsClientInfo};
use super::{AppState, auth, mcp_content, openapi, openapi_routes, redact, ws_events};
use crate::database::settings;

pub const CONFIG_KEY: &str = "mcp_config";
const LATEST_PROTOCOL: &str = "2025-06-18";
const PROTOCOLS: [&str; 3] = [LATEST_PROTOCOL, "2025-03-26", "2024-11-05"];
const RING: usize = 500;
const SESSION_IDLE: Duration = Duration::from_secs(15 * 60);
const MAX_WAIT_MS: u64 = 25_000;
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Stored under `mcp_config` and edited through `/api/connectors/mcp/config`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct McpConfig {
    /// Off until the operator turns it on: an agent endpoint can start a stream.
    pub enabled: bool,
    /// Hides every tool that changes something, and refuses it if called anyway.
    pub read_only: bool,
    /// Accept callers that are not on this machine. Loopback only otherwise.
    pub allow_lan: bool,
}

// ── What is covered ─────────────────────────────────────────────────────────

/// Every REST operation: (method, path under `/api`, tool, summary). A tool name with a `.` is
/// the WebSocket command that does the same thing, so there is one tool for both. Routes
/// outside this table must be in `EXCLUDED`; `mcp_covers_everything` fails otherwise.
#[rustfmt::skip]
const REST: &[(&str, &str, &str, &str)] = &[
    ("GET", "/events", "events.list", "List service events"),
    ("POST", "/events", "events.create", "Create a service event; it starts with the default Order of Service (welcome, songs, Lekció, Textus, preaching, announcements)"),
    ("GET", "/events/{id}", "events.get", "Read one service event"),
    ("PUT", "/events/{id}", "events.update", "Update a service event; a Lekció or Textus set or changed here gets a step in its Order of Service if it has none"),
    ("DELETE", "/events/{id}", "events.delete", "Delete a service event"),
    ("POST", "/events/{id}/stream/{platform}/start", "events.stream.start", "Take the event's YouTube or Facebook broadcast live now"),
    ("POST", "/events/{id}/stream/{platform}/stop", "events.stream.stop", "End the event's live YouTube or Facebook broadcast"),
    ("POST", "/events/{id}/slides", "events_slides_create", "Build the event's Bible-reading slide deck"),
    ("GET", "/events/{id}/order", "events_order_get", "An event's Order of Service: its songs, Bible readings and blocks, in order"),
    ("PUT", "/events/{id}/order", "events_order_set", "Replace an event's Order of Service; steps keep their ids to stay the live step. Send the whole list after each edit"),
    ("GET", "/events/{id}/recordings", "recordings.list", "List an event's recordings"),
    ("POST", "/events/{id}/recordings", "recordings.create", "Attach a recording file to an event"),
    ("DELETE", "/events/{id}/recordings/{recording_id}", "recordings.delete", "Remove a recording from an event"),
    ("POST", "/events/{id}/recordings/flag-upload", "recordings.flag_upload", "Choose which recordings upload, and where"),
    ("GET", "/events/{id}/activities", "activities.list", "List an event's activity log"),
    ("POST", "/events/{id}/activities", "activities.create", "Add an entry to an event's activity log"),
    ("DELETE", "/events/{id}/activities/{activity_id}", "activities.delete", "Delete an activity log entry"),
    ("GET", "/recordings", "recordings.list_all", "List every recording, optionally filtered"),
    ("GET", "/recordings/untracked", "recordings.untracked.list", "List recording files not yet linked to an event"),
    ("POST", "/recordings/untracked/{id}/assign", "recordings.untracked.assign", "Link an untracked recording to an event"),
    ("DELETE", "/recordings/untracked/{id}", "recordings.untracked.delete", "Forget an untracked recording, optionally deleting the file"),
    ("POST", "/recordings/{id}/captions", "recordings_captions_create", "Queue English captions for a recording"),
    ("GET", "/recordings/{id}/captions.vtt", "recordings_captions_vtt", "Read a recording's English captions as WebVTT"),
    ("GET", "/transcripts", "transcripts_list", "List stored live transcripts, newest first"),
    ("GET", "/transcripts/{id}", "transcripts_get", "Read a stored live transcript, every line with its cleaned-up text"),
    ("DELETE", "/transcripts/{id}", "transcripts_delete", "Delete a stored live transcript and its lines"),
    ("GET", "/transcripts/{id}/download", "transcripts_download", "Read a stored transcript as .txt, .srt or .vtt"),
    ("POST", "/transcripts/{id}/cleanup", "transcripts_cleanup", "Start the OpenAI clean-up of a finished transcript"),
    ("GET", "/settings/title-template", "settings_title_template_get", "Read the event title template"),
    ("PUT", "/settings/title-template", "settings_title_template_set", "Set the event title template"),
    ("GET", "/settings/record-button", "settings_record_button_get", "Read which recorders the Record button starts and stops"),
    ("PUT", "/settings/record-button", "settings_record_button_set", "Choose which recorders the Record button starts and stops"),
    ("GET", "/recorders", "recorders_list", "Every recorder the Record button can drive: selected, connected, recording, last start"),
    ("GET", "/settings/slide-folder", "settings_slide_folder_get", "Read the folder event slide decks are saved in"),
    ("PUT", "/settings/slide-folder", "settings_slide_folder_set", "Set the folder event slide decks are saved in"),
    ("GET", "/settings/song-slide-folder", "settings_song_slide_folder_get", "Read the folder song slide decks are saved in"),
    ("PUT", "/settings/song-slide-folder", "settings_song_slide_folder_set", "Set the folder song slide decks are saved in"),
    ("GET", "/connectors/state", "connectors.state", "Live state of every connector"),
    ("GET", "/connectors/status", "connectors.status", "Connection status of every connector"),
    ("GET", "/connectors/{name}/config", "connectors_config_get", "Read a connector's settings; secrets come back blank"),
    ("PUT", "/connectors/{name}/config", "connectors_config_set", "Save a connector's settings and apply them"),
    ("POST", "/connectors/obs/connect", "obs_connect", "Connect to OBS now"),
    ("POST", "/connectors/obs/disconnect", "obs_disconnect", "Disconnect from OBS"),
    ("GET", "/connectors/obs/stream-settings", "obs_stream_settings_get", "Read OBS's stream destination settings"),
    ("PUT", "/connectors/obs/stream-settings", "obs_stream_settings_set", "Set OBS's stream destination settings"),
    ("POST", "/connectors/blackmagic-camera/connect", "blackmagic_camera_connect", "Connect to the Blackmagic camera now"),
    ("POST", "/connectors/blackmagic-camera/discover", "blackmagic-camera.discover", "Find Blackmagic cameras on the network"),
    ("GET", "/connectors/blackmagic-camera/settings", "blackmagic_camera_settings_get", "Read the camera's streaming settings"),
    ("PUT", "/connectors/blackmagic-camera/settings", "blackmagic_camera_settings_apply", "Apply streaming settings to the camera"),
    ("POST", "/connectors/blackmagic-camera/stream/youtube", "blackmagic-camera.stream.push_youtube", "Send the YouTube stream key to the camera"),
    ("POST", "/connectors/middlecontrol/discover", "middlecontrol_discover", "Find Middle Control on the network"),
    ("POST", "/connectors/atem/discover", "atem_discover", "Find ATEM switchers on the network"),
    ("POST", "/connectors/atem/stream/youtube", "atem.stream.push_youtube", "Send the YouTube stream key to the ATEM"),
    ("GET", "/connectors/broadlink/status", "broadlink.status", "Broadlink connector status"),
    ("GET", "/connectors/broadlink/devices", "broadlink.devices.list", "List Broadlink devices"),
    ("POST", "/connectors/broadlink/devices", "broadlink.devices.add", "Add a Broadlink device"),
    ("DELETE", "/connectors/broadlink/devices/{id}", "broadlink.devices.remove", "Remove a Broadlink device"),
    ("POST", "/connectors/broadlink/discover", "broadlink.discover", "Scan for Broadlink devices"),
    ("GET", "/connectors/broadlink/commands", "broadlink.commands.list", "List learned Broadlink commands"),
    ("POST", "/connectors/broadlink/commands", "broadlink.commands.add", "Save a Broadlink command"),
    ("PUT", "/connectors/broadlink/commands/{id}", "broadlink.commands.update", "Edit a Broadlink command"),
    ("DELETE", "/connectors/broadlink/commands/{id}", "broadlink.commands.remove", "Delete a Broadlink command"),
    ("POST", "/connectors/broadlink/devices/{id}/learn", "broadlink.learn.start", "Put a Broadlink device into learn mode"),
    ("POST", "/connectors/broadlink/learn/cancel", "broadlink.learn.cancel", "Cancel learn mode"),
    ("POST", "/connectors/broadlink/commands/{id}/send", "broadlink.commands.send", "Transmit a learned Broadlink command"),
    ("GET", "/connectors/youtube/content", "connectors.youtube.content", "YouTube channel content for the event forms"),
    ("POST", "/connectors/youtube/schedule/{event_id}", "connectors.youtube.schedule", "Create the YouTube broadcast for an event"),
    ("POST", "/connectors/facebook/schedule/{event_id}", "connectors.facebook.schedule", "Create the Facebook live video for an event"),
    ("GET", "/auth/youtube/url", "auth.youtube.url", "The URL that signs the operator in to YouTube"),
    ("POST", "/auth/youtube/logout", "auth.youtube.logout", "Sign out of YouTube"),
    ("GET", "/auth/facebook/url", "auth.facebook.url", "The URL that signs the operator in to Facebook"),
    ("POST", "/auth/facebook/logout", "auth.facebook.logout", "Sign out of Facebook"),
    ("GET", "/bible/verses", "bible_verses", "Look up a Bible passage"),
    ("GET", "/bible/suggest", "bible_suggest", "Autocomplete a Bible reference"),
    ("GET", "/cron-jobs", "cron_jobs.list", "List scheduled jobs"),
    ("POST", "/cron-jobs", "cron_jobs.create", "Create a scheduled job"),
    ("PUT", "/cron-jobs/{id}", "cron_jobs.update", "Edit a scheduled job"),
    ("DELETE", "/cron-jobs/{id}", "cron_jobs.delete", "Delete a scheduled job"),
    ("GET", "/logs", "logs_get", "Read the application log"),
    ("DELETE", "/logs", "logs_clear", "Clear the application log"),
    ("POST", "/uploads/trigger", "uploads.trigger", "Run an upload cycle now"),
    ("GET", "/queues", "queues_list", "List the job queues with their counters"),
    ("GET", "/queues/{queue}/jobs", "queues_jobs_list", "List one queue's jobs"),
    ("POST", "/jobs/{id}/retry", "jobs_retry", "Retry a failed job"),
    ("DELETE", "/jobs/{id}", "jobs_purge", "Delete a job from its queue"),
    ("POST", "/media/probe", "media_probe", "Describe a video file on the server's disk"),
    ("POST", "/media/preview", "media_preview", "Render a still preview of the graded look"),
    ("POST", "/media/renders", "media_renders_start", "Start a graded HEVC render"),
    ("POST", "/media/renders/{id}/cancel", "media_renders_cancel", "Cancel a running render"),
    ("POST", "/media/audio/probe", "media_audio_probe", "Describe the audio files of a recording"),
    ("POST", "/media/audio", "media_audio_start", "Start an audio master render"),
    ("POST", "/media/programs/probe", "media_programs_probe", "Describe a program (video plus audio) before rendering"),
    ("POST", "/media/programs", "media_programs_start", "Start a program render"),
    ("POST", "/media/programs/{id}/nudge", "media_programs_nudge", "Shift a program's audio against its video"),
    ("POST", "/media/programs/{id}/subtitles", "media_programs_subtitles", "Place transcripts on a finished program (replaces that language's track and sends it to YouTube)"),
    ("GET", "/automations", "automations.list", "List automation rules and the armed switch"),
    ("POST", "/automations", "automations_create", "Create an automation rule"),
    ("GET", "/automations/runs", "automations_runs", "Recent automation runs"),
    ("PUT", "/automations/armed", "automations.set_armed", "Arm or disarm every automation"),
    ("PUT", "/automations/{id}", "automations_replace", "Replace an automation rule"),
    ("DELETE", "/automations/{id}", "automations_delete", "Delete an automation rule"),
    ("POST", "/automations/{id}/run", "automations.run", "Run an automation rule now"),
    ("POST", "/openai/verify", "openai_verify", "Check that the OpenAI key works"),
    ("GET", "/ppt/folders", "ppt.folders.list", "List slide folders"),
    ("POST", "/ppt/folders", "ppt.folders.add", "Add a slide folder"),
    ("DELETE", "/ppt/folders/{id}", "ppt.folders.remove", "Remove a slide folder"),
    ("GET", "/ppt/files", "ppt.search", "Search the slide folders for decks"),
    ("POST", "/ppt/song", "ppt_song_create", "Build a song slide deck from lyrics"),
    ("POST", "/presenter/parse", "presenter_parse", "Read a .pptx into slides without loading it"),
    ("GET", "/presenter/welcome", "presenter_welcome_get", "Read the welcome screen's greeting, blessing line, church name and language, and which month photos are replaced"),
    ("PUT", "/presenter/welcome", "presenter_welcome_set", "Set the welcome screen's greeting, blessing line, church name and language (hu or en)"),
    ("DELETE", "/presenter/welcome/photos/{number}", "presenter_welcome_photo_reset", "Put a month's welcome photo (1-12, January-December) back to its default"),
    ("DELETE", "/presenter/welcome/logo", "presenter_welcome_logo_remove", "Remove the welcome screen's logo"),
    ("GET", "/keynote/status", "keynote.status", "Keynote status"),
    ("POST", "/keynote/open", "keynote.open", "Open a deck in Keynote"),
    ("POST", "/keynote/next", "keynote.next", "Keynote: next slide"),
    ("POST", "/keynote/prev", "keynote.prev", "Keynote: previous slide"),
    ("POST", "/keynote/first", "keynote.first", "Keynote: first slide"),
    ("POST", "/keynote/last", "keynote.last", "Keynote: last slide"),
    ("POST", "/keynote/goto", "keynote.goto", "Keynote: go to a slide"),
    ("POST", "/keynote/start", "keynote.start", "Keynote: start the slideshow"),
    ("POST", "/keynote/stop", "keynote.stop", "Keynote: stop the slideshow"),
    ("POST", "/keynote/close_all", "keynote.close_all", "Keynote: close every document"),
];

/// Routes with no tool, with the reason. `(method, path)` as the router spells them; the first
/// seven are outside `/api`.
#[rustfmt::skip]
const EXCLUDED: &[(&str, &str, &str)] = &[
    ("GET", "/health", "liveness probe"),
    ("GET", "/caption", "HTML for OBS browser sources"),
    ("GET", "/caption/logo", "HTML for OBS browser sources"),
    ("GET", "/openapi.json", "served as the metocast://docs/openapi.json resource"),
    ("GET", "/docs", "HTML API reference"),
    ("GET", "/ws", "the WebSocket itself; every command is a tool"),
    ("GET", "/callback", "OAuth redirect target, on its own port"),
    ("POST", "/auth/callback", "finishes OAuth started by a client device; handled via the app"),
    ("GET", "/connectors/{name}/config/secrets", "reveals stored credentials; loopback and admin token only"),
    ("GET", "/connectors/youtube/stream-key", "returns the live stream key, which would sit in the agent's transcript; use connectors_youtube_schedule or the push_youtube tools, which send it on without showing it"),
    ("GET", "/presenter/welcome/photos/{number}", "a JPEG, which an agent can't use; the photo is set in the app"),
    ("PUT", "/presenter/welcome/photos/{number}", "takes image bytes from the operator's computer, not JSON"),
    ("GET", "/presenter/welcome/logo", "a PNG or SVG, which an agent can't use"),
    ("GET", "/presenter/welcome/preview/{number}", "an SVG picture of the slide, for the settings' preview"),
    ("PUT", "/presenter/welcome/logo", "takes image bytes from the operator's computer, not JSON"),
    ("GET", "/connectors/facebook/stream-key", "returns the live stream key, which would sit in the agent's transcript; use connectors_facebook_schedule, which sends it on without showing it"),
];

/// Where the unprefixed entries of `EXCLUDED` are mounted: the first seven.
const ROOT_ROUTES: usize = 7;

/// WebSocket commands with no tool.
const EXCLUDED_WS: &[(&str, &str)] = &[
    (
        "connectors.youtube.stream_key",
        "returns the live stream key, which would sit in the agent's transcript",
    ),
    (
        "connectors.facebook.stream_key",
        "returns the live stream key, which would sit in the agent's transcript",
    ),
    (
        "presenter.register",
        "socket lifecycle: labels a connected browser",
    ),
    ("pong", "socket lifecycle: answers a ping"),
    (
        "presenter.frame",
        "replies with a binary JPEG for a screen to show, not data for an agent",
    ),
];

fn tool_name(wire: &str) -> String {
    wire.replace(['.', '-'], "_")
}

// ── Tools ───────────────────────────────────────────────────────────────────

enum Op {
    Rest(&'static str, &'static str),
    Ws(String),
    Poll,
}

struct Tool {
    name: String,
    about: String,
    input: Value,
    read_only: bool,
    destructive: bool,
    idempotent: bool,
    op: Op,
}

impl Tool {
    /// Access hints from the name and verb (D6): GET and the list/get/status family are read-only;
    /// delete, logout, stream, record-stop (the Record button's too) and the transcript clean-up
    /// are destructive.
    fn new(name: String, about: String, input: Value, op: Op) -> Self {
        let (label, method) = match &op {
            // A command inherits the verb of its REST twin, so `auth.youtube.url` is a read.
            Op::Ws(wire) => (
                wire.as_str(),
                REST.iter()
                    .find(|row| row.2 == wire.as_str())
                    .map_or("", |row| row.0),
            ),
            Op::Rest(method, _) => (name.as_str(), *method),
            Op::Poll => (name.as_str(), "GET"),
        };
        let has = |words: &[&str]| label.split(['.', '_']).any(|token| words.contains(&token));
        let read_only = method == "GET"
            || (method != "DELETE"
                && has(&[
                    "list",
                    "get",
                    "status",
                    "state",
                    "search",
                    "available",
                    "content",
                    "key",
                    "discover",
                    "scan",
                    "profile",
                    "probe",
                    "parse",
                ]));
        let destructive = !read_only
            && (method == "DELETE"
                || has(&[
                    "delete",
                    "remove",
                    "logout",
                    "purge",
                    "clear",
                    "close",
                    "unload",
                    // sends the transcript to OpenAI and replaces the clean version
                    "cleanup",
                    // replaces a finished program's caption track and sends it to YouTube
                    "subtitles",
                ])
                || label.contains(".stream.")
                || label.contains(".record.stop")
                || label == "recorders.stop"
                || label.ends_with("commands.send"));
        let idempotent = read_only
            || matches!(method, "PUT" | "DELETE")
            || has(&["set", "update", "delete", "remove"]);
        Self {
            name,
            about,
            input,
            read_only,
            destructive,
            idempotent,
            op,
        }
    }

    fn source(&self) -> String {
        match &self.op {
            Op::Rest(method, path) => format!("REST {method} /api{path}"),
            Op::Ws(wire) => {
                let twin = REST.iter().find(|row| row.2 == wire.as_str());
                match twin {
                    Some((method, path, _, _)) => format!("WS `{wire}` / REST {method} /api{path}"),
                    None => format!("WS `{wire}`"),
                }
            }
            Op::Poll => "MCP".to_string(),
        }
    }
}

fn path_params(path: &str) -> Vec<&str> {
    path.split('/')
        .filter_map(|segment| segment.strip_prefix('{')?.strip_suffix('}'))
        .collect()
}

/// Replaces `$ref`s into `components.schemas` with the schema itself, so a tool is
/// self-contained. Depth-limited: a cycle ends in an open object.
fn inline_refs(value: &mut Value, schemas: &Value, depth: u8) {
    let target = value
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix("#/components/schemas/"))
        .and_then(|name| schemas.get(name))
        .cloned();
    if let Some(schema) = target {
        if depth == 0 {
            *value = json!({});
        } else {
            *value = schema;
            inline_refs(value, schemas, depth - 1);
        }
        return;
    }
    match value {
        Value::Object(map) => map
            .values_mut()
            .for_each(|child| inline_refs(child, schemas, depth)),
        Value::Array(items) => items
            .iter_mut()
            .for_each(|child| inline_refs(child, schemas, depth)),
        _ => {}
    }
}

/// `path params` + `query` + `body`, taken from the OpenAPI operation.
fn rest_input(spec: &Value, method: &str, path: &str) -> Value {
    let operation = &spec["paths"][format!("/api{path}")][method.to_lowercase()];
    let schemas = &spec["components"]["schemas"];
    let mut properties = Map::new();
    let mut required: Vec<Value> = Vec::new();
    for param in path_params(path) {
        properties.insert(param.to_string(), json!({ "type": "string" }));
        required.push(json!(param));
    }
    let mut query = Map::new();
    let mut query_required: Vec<Value> = Vec::new();
    for param in operation["parameters"].as_array().into_iter().flatten() {
        if param["in"] != "query" {
            continue;
        }
        let Some(name) = param["name"].as_str() else {
            continue;
        };
        let mut schema = param["schema"].clone();
        inline_refs(&mut schema, schemas, 4);
        if let (Some(map), Some(description)) =
            (schema.as_object_mut(), param["description"].as_str())
        {
            map.insert("description".into(), json!(description));
        }
        query.insert(name.to_string(), schema);
        if param["required"] == true {
            query_required.push(json!(name));
        }
    }
    if !query.is_empty() {
        properties.insert(
            "query".into(),
            json!({ "type": "object", "properties": query, "required": query_required }),
        );
    }
    let body = &operation["requestBody"];
    if body.is_object() {
        let mut schema = body["content"]["application/json"]["schema"].clone();
        if schema.is_null() {
            schema = json!({ "type": "object" });
        }
        inline_refs(&mut schema, schemas, 4);
        properties.insert("body".into(), schema);
        if body["required"] == true {
            required.push(json!("body"));
        }
    }
    json!({ "type": "object", "properties": properties, "required": required })
}

fn build_catalog() -> Vec<Tool> {
    let spec = openapi::spec();
    let schema = websocket::command_schema();
    let defs = schema.get("$defs").cloned();
    let mut tools = Vec::new();
    let mut wires = HashSet::new();
    for branch in schema["oneOf"].as_array().into_iter().flatten() {
        let Some(wire) = branch["properties"]["type"]["const"].as_str() else {
            continue;
        };
        wires.insert(wire.to_string());
        if EXCLUDED_WS.iter().any(|(name, _)| *name == wire) {
            continue;
        }
        let mut input = branch.clone();
        if let Some(properties) = input.get_mut("properties").and_then(Value::as_object_mut) {
            properties.remove("type");
        }
        if let Some(required) = input.get_mut("required").and_then(Value::as_array_mut) {
            required.retain(|name| name != "type");
        }
        if let Some(defs) = &defs
            && input.to_string().contains("#/$defs/")
        {
            input["$defs"] = defs.clone();
        }
        let about = REST
            .iter()
            .find(|row| row.2 == wire)
            .map(|row| row.3.to_string())
            .or_else(|| branch["description"].as_str().map(str::to_string))
            .unwrap_or_else(|| format!("WebSocket command `{wire}`"));
        tools.push(Tool::new(
            tool_name(wire),
            about,
            input,
            Op::Ws(wire.to_string()),
        ));
    }
    for (method, path, name, about) in REST {
        // The WebSocket tool already covers it; macOS-only commands fall through to REST.
        if name.contains('.') && wires.contains(*name) {
            continue;
        }
        tools.push(Tool::new(
            tool_name(name),
            (*about).to_string(),
            rest_input(&spec, method, path),
            Op::Rest(method, path),
        ));
    }
    tools.push(Tool::new(
        "events_poll".into(),
        "Read what Metocast pushed to its clients (connector status, event and recording changes, \
         upload progress, notifications, transcript lines). Returns events after `since`, and \
         `next` to pass on the next call. With `wait_ms` it waits for new events. Events are \
         buffered from the moment the first agent connects; the last 500 are kept."
            .into(),
        json!({
            "type": "object",
            "properties": {
                "since": { "type": "integer", "minimum": 0, "description": "Cursor from the previous call's `next`. Omit for the most recent events." },
                "types": { "type": "array", "items": { "type": "string" }, "description": "Only these event types, e.g. [\"event.changed\"]. See docs/api/WEBSOCKET.md." },
                "wait_ms": { "type": "integer", "minimum": 0, "maximum": MAX_WAIT_MS, "description": "Wait this long for a new event when there is none yet." },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200, "description": "At most this many events (default 100)." }
            }
        }),
        Op::Poll,
    ));
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    tools
}

fn catalog() -> &'static [Tool] {
    static CATALOG: OnceLock<Vec<Tool>> = OnceLock::new();
    CATALOG.get_or_init(build_catalog)
}

#[cfg(test)]
pub(crate) fn ws_command_names() -> Vec<String> {
    websocket::command_schema()["oneOf"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|branch| branch["properties"]["type"]["const"].as_str())
        .map(str::to_string)
        .collect()
}

/// Adds the REST operations the hand-written spec leaves out, from the same table the tools
/// use, so the spec lists every route. `openapi_routes.rs` holds real operations; whatever is
/// still missing after those is a summary-only stub, to be replaced by writing it out.
pub fn document_missing(spec: &mut Value) {
    let rows = REST
        .iter()
        .map(|(method, path, name, about)| (method, format!("/api{path}"), *about, Some(*name)))
        .chain(
            EXCLUDED
                .iter()
                .enumerate()
                .map(|(index, (method, path, why))| {
                    let full = if index < ROOT_ROUTES {
                        (*path).to_string()
                    } else {
                        format!("/api{path}")
                    };
                    (method, full, *why, None)
                }),
        );
    let Some(paths) = spec.get_mut("paths").and_then(Value::as_object_mut) else {
        return;
    };
    // The operations written out in `openapi_routes.rs`, where the hand-written spec has none.
    for (route, methods) in openapi_routes::paths().as_object().into_iter().flatten() {
        let entry = paths.entry(route.clone()).or_insert_with(|| json!({}));
        for (verb, operation) in methods.as_object().into_iter().flatten() {
            if entry.get(verb).is_none() {
                entry[verb] = operation.clone();
            }
        }
    }
    for (method, path, about, name) in rows {
        let verb = method.to_lowercase();
        let entry = paths.entry(path.clone()).or_insert_with(|| json!({}));
        if let Some(existing) = entry.get_mut(&verb) {
            // Same id as the tool, so a client can map an operation to its tool.
            if let (Some(name), None) = (name, existing.get("operationId")) {
                existing["operationId"] = json!(tool_name(name));
            }
            continue;
        }
        let params: Vec<Value> = path_params(&path)
            .into_iter()
            .map(|param| json!({ "name": param, "in": "path", "required": true, "schema": { "type": "string" } }))
            .collect();
        let mut operation = json!({
            "summary": about,
            "tags": ["Other"],
            "parameters": params,
            "responses": { "200": { "description": "OK" }, "401": { "description": "Unauthorized" } }
        });
        if let Some(name) = name {
            operation["operationId"] = json!(tool_name(name));
        }
        if matches!(verb.as_str(), "post" | "put") {
            operation["requestBody"] = json!({
                "content": { "application/json": { "schema": { "type": "object" } } }
            });
        }
        entry[&verb] = operation;
    }
}

// ── Events ──────────────────────────────────────────────────────────────────

/// The last `RING` things the server pushed, fed by a client entry in `ws_clients`.
#[derive(Default)]
struct Events {
    ring: Mutex<VecDeque<(u64, Value)>>,
    head: AtomicU64,
    wake: Notify,
}

impl Events {
    fn push(&self, event: Value) {
        let cursor = self.head.fetch_add(1, Ordering::SeqCst) + 1;
        let at = Utc::now().to_rfc3339();
        let mut ring = self
            .ring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if ring.len() == RING {
            ring.pop_front();
        }
        ring.push_back((
            cursor,
            json!({ "cursor": cursor, "at": at, "event": event }),
        ));
        drop(ring);
        self.wake.notify_waiters();
    }

    fn since(&self, since: Option<u64>, types: &[String], limit: usize) -> Value {
        let ring = self
            .ring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let next = self.head.load(Ordering::SeqCst);
        let oldest = ring.front().map_or(next + 1, |(cursor, _)| *cursor);
        let matching = ring
            .iter()
            .filter(|(cursor, event)| {
                since.is_none_or(|since| *cursor > since)
                    && (types.is_empty()
                        || event["event"]["type"]
                            .as_str()
                            .is_some_and(|kind| types.iter().any(|wanted| wanted == kind)))
            })
            .map(|(_, event)| event.clone())
            .collect::<Vec<_>>();
        let skip = matching.len().saturating_sub(limit);
        let events: Vec<Value> = matching.into_iter().skip(skip).collect();
        json!({
            "events": events,
            "next": next,
            "missed": since.is_some_and(|since| since + 1 < oldest && next > 0),
        })
    }

    async fn poll(&self, args: &Value) -> Value {
        let since = args["since"].as_u64();
        let types: Vec<String> = args["types"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|kind| kind.as_str().map(str::to_string))
            .collect();
        let limit = args["limit"]
            .as_u64()
            .map_or(100, |n| n.clamp(1, 200) as usize);
        let deadline = Instant::now()
            + Duration::from_millis(args["wait_ms"].as_u64().unwrap_or(0).min(MAX_WAIT_MS));
        loop {
            let woken = self.wake.notified();
            tokio::pin!(woken);
            woken.as_mut().enable();
            let found = self.since(since, &types, limit);
            let nothing = found["events"].as_array().is_none_or(Vec::is_empty);
            if !nothing || since.is_none() || Instant::now() >= deadline {
                return found;
            }
            tokio::time::timeout_at(deadline.into(), woken)
                .await
                .log_gone();
        }
    }
}

// ── Sessions ────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Sessions {
    seen: HashMap<Uuid, Instant>,
    /// The `ws_clients` entry feeding `Events`, present only while an agent is connected: the
    /// queue-stats guard skips its aggregate query when nobody listens.
    feed: Option<Uuid>,
    /// What each session subscribed to with `resources/subscribe`.
    subs: HashMap<Uuid, HashSet<String>>,
}

#[derive(Clone)]
struct McpState {
    app: AppState,
    api: Router,
    events: Arc<Events>,
    sessions: Arc<tokio::sync::Mutex<Sessions>>,
    /// Resource URIs made stale by a push, fed by the same entry as `events`.
    updates: broadcast::Sender<String>,
    /// Ends the server-sent streams, which would otherwise hold a graceful shutdown open.
    shutdown: Option<watch::Receiver<bool>>,
}

impl McpState {
    async fn begin(&self, name: &str, user_agent: Option<String>) -> Uuid {
        let id = Uuid::new_v4();
        let mut sessions = self.sessions.lock().await;
        sessions.seen.insert(id, Instant::now());
        if sessions.feed.is_none() {
            let (tx, mut rx) = crate::server::ws_client::ClientTx::channel(false);
            let feed = Uuid::new_v4();
            self.app.ws_clients.write().await.insert(feed, tx);
            sessions.feed = Some(feed);
            let events = self.events.clone();
            let updates = self.updates.clone();
            tokio::spawn(async move {
                while let Some(outgoing) = rx.recv().await {
                    if let Message::Text(text) = outgoing.message
                        && let Ok(event) = serde_json::from_str::<Value>(&text)
                        && event["type"] != "ping"
                    {
                        for uri in mcp_content::affected(&event) {
                            updates.send(uri).log_gone();
                        }
                        events.push(event);
                    }
                }
            });
        }
        drop(sessions);
        self.app.ws_client_info.write().await.insert(
            id,
            WsClientInfo {
                id,
                label: format!("MCP · {name}"),
                user_agent,
                hostname: None,
                connected_at: Utc::now(),
                last_pong_at: None,
                latency_ms: None,
                ping_sent_at: None,
            },
        );
        websocket::broadcast_clients_updated(&self.app).await;
        id
    }

    async fn end(&self, gone: &[Uuid]) {
        if gone.is_empty() {
            return;
        }
        let mut sessions = self.sessions.lock().await;
        for id in gone {
            sessions.seen.remove(id);
            sessions.subs.remove(id);
        }
        if sessions.seen.is_empty()
            && let Some(feed) = sessions.feed.take()
        {
            self.app.ws_clients.write().await.remove(&feed);
        }
        drop(sessions);
        let mut info = self.app.ws_client_info.write().await;
        for id in gone {
            info.remove(id);
        }
        drop(info);
        websocket::broadcast_clients_updated(&self.app).await;
    }

    async fn sweep(&self) {
        let gone: Vec<Uuid> = self
            .sessions
            .lock()
            .await
            .seen
            .iter()
            .filter(|(_, seen)| seen.elapsed() > SESSION_IDLE)
            .map(|(id, _)| *id)
            .collect();
        self.end(&gone).await;
    }
}

// ── HTTP ────────────────────────────────────────────────────────────────────

/// `/mcp`, behind the same bearer-token middleware as `/api`. `api` is the finished `/api`
/// router, which REST tools call in-process.
pub fn router(app: AppState, api: Router, shutdown: Option<watch::Receiver<bool>>) -> Router {
    let state = McpState {
        app: app.clone(),
        api,
        events: Arc::new(Events::default()),
        sessions: Arc::default(),
        updates: broadcast::channel(256).0,
        shutdown,
    };
    let sweeper = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            sweeper.sweep().await;
        }
    });
    Router::new()
        .route("/mcp", post(handle).get(stream).delete(close))
        .with_state(state)
        .route_layer(middleware::from_fn_with_state(app, auth::auth_middleware))
}

fn rpc_error(id: &Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn refuse(status: StatusCode, message: &str) -> Response {
    (status, axum::Json(rpc_error(&Value::Null, -32000, message))).into_response()
}

fn local_origin(origin: &str) -> bool {
    let host = origin.split("://").nth(1).unwrap_or(origin);
    let host = host.split('/').next().unwrap_or(host);
    let host = host
        .rsplit_once(':')
        .filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit()))
        .map_or(host, |(name, _)| name);
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

/// With LAN access off only this machine may call: the peer must be on loopback, and a browser
/// `Origin` (which the MCP spec says to validate, so a web page can't reach a local server) must
/// be local too. An unknown peer is a test harness without `ConnectInfo`.
fn caller_allowed(
    peer: Option<SocketAddr>,
    origin: Option<&str>,
    allow_lan: bool,
) -> Result<(), &'static str> {
    if allow_lan {
        return Ok(());
    }
    if peer.is_some_and(|peer| !peer.ip().to_canonical().is_loopback()) {
        return Err(
            "MCP only accepts callers on this machine. Turn on LAN access in the Metocast app.",
        );
    }
    match origin {
        Some(origin) if !local_origin(origin) => Err("Origin not allowed."),
        _ => Ok(()),
    }
}

/// The part of the checks that needs no database or session: MCP is on and the caller may use
/// it. Returns the peer address when the connection has one.
fn gate(
    config: &McpConfig,
    parts: &axum::http::request::Parts,
) -> Result<Option<SocketAddr>, Box<Response>> {
    if !config.enabled {
        return Err(Box::new(refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "MCP is turned off. Turn it on in the Metocast app under Connectors, AI Agents (MCP).",
        )));
    }
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0);
    let origin = parts
        .headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    match caller_allowed(peer, origin, config.allow_lan) {
        Ok(()) => Ok(peer),
        Err(why) => Err(Box::new(refuse(StatusCode::FORBIDDEN, why))),
    }
}

/// The checks every request passes: MCP on, caller allowed, session known. Returns the config,
/// the peer, and the session the request names (touched, so it stays alive).
async fn admit(
    state: &McpState,
    parts: &axum::http::request::Parts,
) -> Result<(McpConfig, Option<SocketAddr>, Option<Uuid>), Box<Response>> {
    let config = settings::get_json::<McpConfig>(&state.app.pool, CONFIG_KEY).await;
    let peer = gate(&config, parts)?;
    let session = parts
        .headers
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<Uuid>().ok());
    if let Some(id) = session {
        let mut sessions = state.sessions.lock().await;
        match sessions.seen.get_mut(&id) {
            Some(seen) => *seen = Instant::now(),
            // Server restarted or the session timed out: the client must initialize again.
            None => {
                return Err(Box::new(refuse(
                    StatusCode::NOT_FOUND,
                    "Unknown session; send initialize again.",
                )));
            }
        }
    }
    Ok((config, peer, session))
}

/// `GET /mcp` with `Accept: text/event-stream`: the notifications for what the session subscribed
/// to. Anything else is not offered.
async fn stream(State(state): State<McpState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    let wants_stream = parts
        .headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("text/event-stream"));
    if !wants_stream {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let session = match admit(&state, &parts).await {
        Ok((_, _, Some(session))) => session,
        Ok(_) => {
            return refuse(
                StatusCode::BAD_REQUEST,
                "Send initialize first and pass its Mcp-Session-Id.",
            );
        }
        Err(response) => return *response,
    };
    let seed = (
        state.updates.subscribe(),
        state.shutdown.clone(),
        state.sessions.clone(),
    );
    let notices = futures_util::stream::unfold(
        seed,
        move |(mut updates, mut shutdown, sessions)| async move {
            loop {
                let stop = async {
                    match shutdown.as_mut() {
                        Some(signal) => {
                            while !*signal.borrow() && signal.changed().await.is_ok() {}
                        }
                        None => std::future::pending::<()>().await,
                    }
                };
                tokio::select! {
                    () = stop => return None,
                    // Ends the stream once the session is gone.
                    () = tokio::time::sleep(Duration::from_secs(30)) => {
                        if !sessions.lock().await.seen.contains_key(&session) {
                            return None;
                        }
                    }
                    update = updates.recv() => match update {
                        Ok(uri) => {
                            let wanted = sessions
                                .lock()
                                .await
                                .subs
                                .get(&session)
                                .is_some_and(|subscribed| subscribed.contains(&uri));
                            if wanted {
                                let note = json!({
                                    "jsonrpc": "2.0",
                                    "method": "notifications/resources/updated",
                                    "params": { "uri": uri },
                                });
                                let event = Event::default().data(note.to_string());
                                return Some((
                                    Ok::<_, std::convert::Infallible>(event),
                                    (updates, shutdown, sessions),
                                ));
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => return None,
                    },
                }
            }
        },
    );
    Sse::new(notices)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn close(State(state): State<McpState>, req: Request) -> Response {
    if let Some(id) = req
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<Uuid>().ok())
    {
        state.end(&[id]).await;
    }
    StatusCode::OK.into_response()
}

async fn handle(State(state): State<McpState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let (config, peer, session) = match admit(&state, &parts).await {
        Ok(admitted) => admitted,
        Err(response) => return *response,
    };
    let Ok(bytes) = axum::body::to_bytes(body, 4 * 1024 * 1024).await else {
        return refuse(StatusCode::PAYLOAD_TOO_LARGE, "Request body too large.");
    };
    let Ok(message) = serde_json::from_slice::<Value>(&bytes) else {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(rpc_error(&Value::Null, -32700, "Parse error")),
        )
            .into_response();
    };
    let call = Call {
        auth: parts.headers.get(header::AUTHORIZATION).cloned(),
        user_agent: parts
            .headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string),
        peer,
        config,
        session,
    };
    let mut new_session = None;
    let reply = match message {
        // Batches were dropped from the spec in 2025-06-18; accepted for older clients.
        Value::Array(items) => {
            let mut replies = Vec::new();
            for item in items {
                replies.extend(dispatch(&state, &call, item, &mut new_session).await);
            }
            (!replies.is_empty()).then_some(Value::Array(replies))
        }
        single => dispatch(&state, &call, single, &mut new_session).await,
    };
    let mut response = match reply {
        Some(body) => axum::Json(body).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    };
    if let Some(id) = new_session
        && let Ok(value) = HeaderValue::from_str(&id.to_string())
    {
        response.headers_mut().insert("mcp-session-id", value);
    }
    response
}

struct Call {
    auth: Option<HeaderValue>,
    user_agent: Option<String>,
    peer: Option<SocketAddr>,
    config: McpConfig,
    /// The session the request names, when it names one.
    session: Option<Uuid>,
}

// ── JSON-RPC ────────────────────────────────────────────────────────────────

async fn dispatch(
    state: &McpState,
    call: &Call,
    message: Value,
    new_session: &mut Option<Uuid>,
) -> Option<Value> {
    let id = message.get("id").cloned()?; // no id: a notification, nothing to answer
    let Some(method) = message["method"].as_str() else {
        return Some(rpc_error(&id, -32600, "Invalid request"));
    };
    let params = &message["params"];
    let result = match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or_default();
            let version = PROTOCOLS
                .iter()
                .find(|p| **p == asked)
                .unwrap_or(&LATEST_PROTOCOL);
            let name = params["clientInfo"]["name"].as_str().unwrap_or("agent");
            *new_session = Some(state.begin(name, call.user_agent.clone()).await);
            json!({
                "protocolVersion": version,
                "capabilities": {
                    "tools": { "listChanged": false },
                    "resources": { "subscribe": true, "listChanged": false },
                    "prompts": { "listChanged": false },
                },
                "serverInfo": { "name": "metocast", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Metocast runs a live church broadcast. Read before you act: use the *_list, \
                    *_get and *_status tools first, and treat stream, record and delete tools as live \
                    actions. `events_poll` returns what changed; the metocast://state/ resources hold current state \
                    and can be subscribed to. The manual is under metocast://docs/, and the prompts \
                    prepare_service, go_live_checklist and post_service_upload walk through routines."
            })
        }
        "ping" => json!({}),
        "tools/list" => {
            let tools: Vec<Value> = catalog()
                .iter()
                .filter(|tool| tool.read_only || !call.config.read_only)
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": format!("{} [{}]", tool.about, tool.source()),
                        "inputSchema": tool.input,
                        "annotations": {
                            "readOnlyHint": tool.read_only,
                            "destructiveHint": tool.destructive,
                            "idempotentHint": tool.idempotent,
                            "openWorldHint": false,
                        }
                    })
                })
                .collect();
            json!({ "tools": tools })
        }
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let Some(tool) = catalog().iter().find(|tool| tool.name == name) else {
                return Some(rpc_error(&id, -32602, &format!("Unknown tool: {name}")));
            };
            if !tool.read_only && call.config.read_only {
                text_result(
                    "MCP is in read-only mode; this tool changes things.".into(),
                    true,
                )
            } else {
                match tokio::time::timeout(CALL_TIMEOUT, run_tool(state, call, tool, args)).await {
                    Ok((text, is_error)) => text_result(text, is_error),
                    Err(_) => text_result(
                        format!("{name} did not answer within {}s", CALL_TIMEOUT.as_secs()),
                        true,
                    ),
                }
            }
        }
        "resources/list" => mcp_content::resources(),
        "resources/templates/list" => mcp_content::templates(),
        "resources/read" => {
            let uri = params["uri"].as_str().unwrap_or_default();
            match read_resource(state, call, uri).await {
                Ok((mime, text)) => {
                    let text = redact::apply(&text).into_owned();
                    json!({ "contents": [{ "uri": uri, "mimeType": mime, "text": text }] })
                }
                Err((code, message)) => return Some(rpc_error(&id, code, &message)),
            }
        }
        "resources/subscribe" | "resources/unsubscribe" => {
            let uri = params["uri"].as_str().unwrap_or_default();
            if !is_state_uri(uri) {
                return Some(rpc_error(
                    &id,
                    -32602,
                    &format!("Only metocast://state/ resources can be subscribed to: {uri}"),
                ));
            }
            let Some(session) = call.session else {
                return Some(rpc_error(
                    &id,
                    -32602,
                    "Subscribing needs a session: send initialize first and pass its Mcp-Session-Id.",
                ));
            };
            let mut sessions = state.sessions.lock().await;
            let subscribed = sessions.subs.entry(session).or_default();
            if method == "resources/subscribe" {
                subscribed.insert(uri.to_string());
            } else {
                subscribed.remove(uri);
            }
            json!({})
        }
        "prompts/list" => mcp_content::prompts(),
        "prompts/get" => {
            match mcp_content::prompt(
                params["name"].as_str().unwrap_or_default(),
                &params["arguments"],
            ) {
                Ok(prompt) => prompt,
                Err(message) => return Some(rpc_error(&id, -32602, &message)),
            }
        }
        other => {
            return Some(rpc_error(
                &id,
                -32601,
                &format!("Method not found: {other}"),
            ));
        }
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn is_state_uri(uri: &str) -> bool {
    mcp_content::STATE.iter().any(|(known, _, _)| *known == uri)
        || uri
            .strip_prefix(mcp_content::EVENT_PREFIX)
            .is_some_and(|id| id.parse::<Uuid>().is_ok())
}

/// `(mime type, text)`, or a JSON-RPC `(code, message)`.
async fn read_resource(
    state: &McpState,
    call: &Call,
    uri: &str,
) -> Result<(&'static str, String), (i32, String)> {
    match uri {
        "metocast://docs/openapi.json" => {
            return Ok(("application/json", openapi::spec().to_string()));
        }
        "metocast://docs/websocket.md" => return Ok(("text/markdown", websocket_markdown())),
        "metocast://docs/mcp-tools.md" => return Ok(("text/markdown", tools_markdown())),
        "metocast://state/transcription" => {
            return Ok((
                "application/json",
                state.app.transcription.snapshot_message(),
            ));
        }
        _ => {}
    }
    if let Some(name) = uri
        .strip_prefix(mcp_content::FEATURE_PREFIX)
        .and_then(|rest| rest.strip_suffix(".md"))
    {
        return mcp_content::FEATURES
            .iter()
            .find(|(feature, _)| *feature == name)
            .map(|(_, text)| ("text/markdown", (*text).to_string()))
            .ok_or_else(|| (-32002, format!("Unknown resource: {uri}")));
    }
    let (tool, args) =
        if let Some((_, _, tool)) = mcp_content::STATE.iter().find(|row| row.0 == uri) {
            (*tool, json!({}))
        } else if let Some(id) = uri.strip_prefix(mcp_content::EVENT_PREFIX) {
            ("events_get", json!({ "id": id }))
        } else {
            return Err((-32002, format!("Unknown resource: {uri}")));
        };
    let Some(tool) = catalog().iter().find(|candidate| candidate.name == tool) else {
        return Err((-32603, format!("No tool {tool} for {uri}")));
    };
    match run_tool(state, call, tool, args).await {
        (text, false) => Ok(("application/json", text)),
        (message, true) => Err((-32603, message)),
    }
}

fn text_result(text: String, is_error: bool) -> Value {
    let text = redact::apply(&text).into_owned();
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

/// `(text, is_error)`.
async fn run_tool(state: &McpState, call: &Call, tool: &Tool, args: Value) -> (String, bool) {
    match &tool.op {
        Op::Poll => (state.events.poll(&args).await.to_string(), false),
        Op::Ws(wire) => {
            let mut command = match args {
                Value::Object(map) => map,
                _ => Map::new(),
            };
            command.insert("type".into(), json!(wire));
            match websocket::run_command(&state.app, Value::Object(command)).await {
                Err(error) => (
                    format!("Invalid arguments for {}: {error}", tool.name),
                    true,
                ),
                Ok(replies) => {
                    let failed = replies.iter().find(|r| r["type"] == "error");
                    if let Some(failure) = failed {
                        return (
                            failure["message"]
                                .as_str()
                                .unwrap_or("command failed")
                                .to_string(),
                            true,
                        );
                    }
                    let text = match replies.len() {
                        0 => json!({ "type": "sent" }).to_string(),
                        1 => replies[0].to_string(),
                        _ => Value::Array(replies).to_string(),
                    };
                    (text, false)
                }
            }
        }
        // An agent must not widen its own access: only the operator edits these switches.
        Op::Rest("PUT", "/connectors/{name}/config") if args["name"] == "mcp" => (
            "MCP settings can only be changed by the operator, in the Metocast app.".into(),
            true,
        ),
        Op::Rest(method, path) => call_rest(state, call, method, path, &args).await,
    }
}

async fn call_rest(
    state: &McpState,
    call: &Call,
    method: &str,
    path: &str,
    args: &Value,
) -> (String, bool) {
    let mut uri = String::from("/api");
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        uri.push('/');
        match segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            Some(param) => match &args[param] {
                Value::String(value) => uri.push_str(&urlencoding::encode(value)),
                Value::Number(value) => uri.push_str(&value.to_string()),
                _ => return (format!("Missing argument: {param}"), true),
            },
            None => uri.push_str(segment),
        }
    }
    if let Some(query) = args["query"].as_object() {
        let pairs: Vec<String> = query
            .iter()
            .filter(|(_, value)| !value.is_null())
            .map(|(key, value)| {
                let text = value
                    .as_str()
                    .map_or_else(|| value.to_string(), str::to_string);
                format!(
                    "{}={}",
                    urlencoding::encode(key),
                    urlencoding::encode(&text)
                )
            })
            .collect();
        if !pairs.is_empty() {
            uri.push('?');
            uri.push_str(&pairs.join("&"));
        }
    }
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = &call.auth {
        builder = builder.header(header::AUTHORIZATION, token);
    }
    let body = match &args["body"] {
        Value::Null => Body::empty(),
        body => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.to_string())
        }
    };
    let Ok(mut request) = builder.body(body) else {
        return ("Could not build the request".into(), true);
    };
    if let Some(peer) = call.peer {
        request.extensions_mut().insert(ConnectInfo(peer));
    }
    let Ok(response) = state.api.clone().oneshot(request).await;
    let status = response.status();
    let Ok(bytes) = axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024).await else {
        return ("Response too large".into(), true);
    };
    let text = String::from_utf8_lossy(&bytes).into_owned();
    if status.is_success() {
        (if text.is_empty() { "{}".into() } else { text }, false)
    } else {
        (format!("HTTP {}: {text}", status.as_u16()), true)
    }
}

// ── Generated docs ──────────────────────────────────────────────────────────

fn field_list(input: &Value) -> String {
    let required: Vec<&str> = input["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let fields: Vec<String> = input["properties"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, _)| {
            format!(
                "`{name}`{}",
                if required.contains(&name.as_str()) {
                    "*"
                } else {
                    ""
                }
            )
        })
        .collect();
    if fields.is_empty() {
        "—".into()
    } else {
        fields.join(", ")
    }
}

/// `docs/mcp/TOOLS.md`.
pub fn tools_markdown() -> String {
    let mut out = String::from(
        "# MCP tool reference\n\n\
         Generated by `cargo test -p metocast-server generated_docs` with `UPDATE_GOLDEN=1`; do not edit.\n\n\
         **Access**: `read` changes nothing; `write` changes something; `destructive` stops, deletes or \
         sends something that is hard to undo. Read-only mode hides everything but `read`. A `*` marks a \
         required input.\n\n\
         | Tool | Access | Does | Inputs | Source |\n|---|---|---|---|---|\n",
    );
    // Keynote exists only in macOS builds, so its rows come from the fixed table below and the
    // generated files read the same on every platform.
    for tool in catalog()
        .iter()
        .filter(|tool| !tool.name.starts_with("keynote_"))
    {
        let access = if tool.read_only {
            "read"
        } else if tool.destructive {
            "destructive"
        } else {
            "write"
        };
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            tool.name,
            access,
            tool.about.replace('|', "/"),
            field_list(&tool.input),
            tool.source().replace('|', "/"),
        ));
    }
    out.push_str("\n## Keynote (macOS builds only)\n\n| Tool | Does | Source |\n|---|---|---|\n");
    for (method, path, name, about) in REST.iter().filter(|row| row.2.starts_with("keynote.")) {
        out.push_str(&format!(
            "| `{}` | {about} | WS `{name}` / REST {method} /api{path} |\n",
            tool_name(name)
        ));
    }
    out.push_str("\n## Not exposed\n\n| Route or command | Why |\n|---|---|\n");
    for (method, path, why) in EXCLUDED {
        out.push_str(&format!("| `{method} {path}` | {why} |\n"));
    }
    for (wire, why) in EXCLUDED_WS {
        out.push_str(&format!("| WS `{wire}` | {why} |\n"));
    }
    out
}

/// `docs/api/WEBSOCKET.md`.
pub fn websocket_markdown() -> String {
    let mut out = String::from(
        "# WebSocket protocol\n\n\
         Generated by `cargo test -p metocast-server generated_docs` with `UPDATE_GOLDEN=1`; do not edit.\n\n\
         Connect to `ws://<host>/ws?token=<token>`. Send `{\"type\": \"<command>\", …fields}`. A command \
         answers with `{\"type\":\"ok\"}`, `{\"type\":\"error\",\"message\":…}` or a typed reply; replies \
         carry no request id, so send one command at a time per socket.\n\n\
         ## Commands\n\n| Command | Fields | REST twin |\n|---|---|---|\n",
    );
    let mut wires: Vec<&Tool> = catalog()
        .iter()
        .filter(|tool| matches!(&tool.op, Op::Ws(wire) if !wire.starts_with("keynote.")))
        .collect();
    wires.sort_by_key(|tool| match &tool.op {
        Op::Ws(wire) => wire.clone(),
        _ => String::new(),
    });
    for tool in wires {
        let Op::Ws(wire) = &tool.op else { continue };
        let twin = REST.iter().find(|row| row.2 == wire.as_str()).map_or_else(
            || "—".to_string(),
            |(method, path, _, _)| format!("`{method} /api{path}`"),
        );
        out.push_str(&format!(
            "| `{wire}` | {} | {twin} |\n",
            field_list(&tool.input)
        ));
    }
    out.push_str(
        "| `keynote.*` | `keynote.open`, `.next`, `.prev`, `.first`, `.last`, `.goto`, `.start`, \
         `.stop`, `.close_all`, `.status` (macOS builds only) | `/api/keynote/*` |\n",
    );
    out.push_str("\n## Messages the server sends\n\n| Type | When | Fields |\n|---|---|---|\n");
    for (name, when, fields) in ws_events::EVENTS {
        let fields = if fields.is_empty() {
            "none".to_string()
        } else {
            fields
                .iter()
                .map(|(field, declared)| format!("`{field}` {declared}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        out.push_str(&format!("| `{name}` | {when} | {fields} |\n"));
    }
    out.push_str(
        "\nThe same fields as JSON Schema: `docs/api/ws-events.schema.json`. A type ending in \
         `?` may be null.\n",
    );
    out.push_str(
        "\nA command's reply reuses the command's own name as `type` (for example `events.list`).\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

    /// Every `(method, path)` the router declares, read from `server/mod.rs`. Crude, like the
    /// event scan: a route added there must be catalogued here or the test fails.
    fn routes() -> BTreeSet<(String, String)> {
        let source = include_str!("mod.rs");
        let route = regex::Regex::new(r#"\.route\(\s*"([^"]+)""#).unwrap();
        let verb = regex::Regex::new(r"\b(get|post|put|delete)\(").unwrap();
        let starts: Vec<(usize, String)> = route
            .captures_iter(source)
            .map(|found| (found.get(0).unwrap().start(), found[1].to_string()))
            .collect();
        let mut found = BTreeSet::new();
        for (index, (start, path)) in starts.iter().enumerate() {
            let after = &source[*start + 7..];
            let end = starts
                .get(index + 1)
                .map_or(after.len(), |next| next.0 - start - 7);
            let end = after[..end].find(';').unwrap_or(end);
            for verb in verb.captures_iter(&after[..end]) {
                found.insert((verb[1].to_uppercase(), path.clone()));
            }
        }
        found
    }

    #[test]
    fn mcp_covers_everything() {
        let declared = routes();
        assert!(
            declared.len() > 90,
            "route scan found only {}",
            declared.len()
        );
        let mut known: BTreeSet<(String, String)> = REST
            .iter()
            .map(|(method, path, _, _)| ((*method).to_string(), (*path).to_string()))
            .collect();
        let listed = known.len();
        known.extend(
            EXCLUDED
                .iter()
                .map(|(method, path, _)| ((*method).to_string(), (*path).to_string())),
        );
        assert_eq!(listed, REST.len(), "duplicate row in REST");
        let missing: Vec<_> = declared.difference(&known).collect();
        assert!(
            missing.is_empty(),
            "routes without a tool or an exclusion: {missing:?}"
        );
        let stale: Vec<_> = known.difference(&declared).collect();
        assert!(stale.is_empty(), "catalogue rows with no route: {stale:?}");

        let names: Vec<&str> = catalog().iter().map(|tool| tool.name.as_str()).collect();
        let unique: BTreeSet<&str> = names.iter().copied().collect();
        assert_eq!(unique.len(), names.len(), "duplicate tool names");
        let valid = regex::Regex::new(r"^[a-z][a-z0-9_]{0,63}$").unwrap();
        for name in &names {
            assert!(valid.is_match(name), "bad tool name {name}");
        }
        // Every command is a tool unless excluded; every dotted row names a real command.
        let commands: BTreeSet<String> = ws_command_names().into_iter().collect();
        for wire in &commands {
            let excluded = EXCLUDED_WS.iter().any(|(name, _)| name == wire);
            assert!(
                excluded || names.contains(&tool_name(wire).as_str()),
                "{wire} has no tool"
            );
        }
        for (_, _, name, _) in REST {
            let mac_only = name.starts_with("keynote.");
            assert!(
                !name.contains('.') || commands.contains(*name) || mac_only,
                "{name} is not a WebSocket command"
            );
        }
        assert!(catalog().len() > 150, "only {} tools", catalog().len());
    }

    #[test]
    fn openapi_covers_router() {
        let spec = openapi::spec();
        for (method, path) in routes() {
            let full = if EXCLUDED
                .iter()
                .take(ROOT_ROUTES)
                .any(|(_, p, _)| *p == path)
            {
                path.clone()
            } else {
                format!("/api{path}")
            };
            assert!(
                spec["paths"][&full][method.to_lowercase()].is_object(),
                "{method} {full} is missing from the OpenAPI spec"
            );
        }
    }

    /// A route can still be a one-line stub; this keeps the list of them from growing back.
    #[test]
    fn every_catalogued_route_has_a_written_operation() {
        let spec = openapi::spec();
        let stubs: Vec<String> = REST
            .iter()
            .filter(|(method, path, _, _)| {
                spec["paths"][format!("/api{path}")][method.to_lowercase()]["tags"][0] == "Other"
            })
            .map(|(method, path, _, _)| format!("{method} {path}"))
            .collect();
        assert!(stubs.is_empty(), "stub operations: {stubs:?}");
    }

    #[test]
    fn destructive_tools_are_marked() {
        let find = |name: &str| catalog().iter().find(|tool| tool.name == name).unwrap();
        for name in [
            "events_delete",
            "obs_stream_stop",
            "broadlink_commands_send",
            "jobs_purge",
            "auth_youtube_logout",
            "obs_record_stop",
            "recorders_stop",
            "transcripts_delete",
            "transcripts_cleanup",
            "media_programs_subtitles",
            "events_stream_start",
            "events_stream_stop",
        ] {
            assert!(find(name).destructive, "{name} should be destructive");
        }
        for name in [
            "events_list",
            "connectors_status",
            "events_poll",
            "clients_list",
        ] {
            assert!(find(name).read_only, "{name} should be read-only");
        }
        assert!(!find("events_create").read_only && !find("events_create").destructive);
    }

    #[test]
    fn only_this_machine_may_call_unless_lan_is_allowed() {
        let local: SocketAddr = "127.0.0.1:50000".parse().unwrap();
        let mapped: SocketAddr = "[::ffff:127.0.0.1]:50000".parse().unwrap();
        let v6: SocketAddr = "[::1]:50000".parse().unwrap();
        let lan: SocketAddr = "192.168.0.27:50000".parse().unwrap();
        for peer in [local, mapped, v6] {
            assert!(caller_allowed(Some(peer), None, false).is_ok(), "{peer}");
        }
        assert!(caller_allowed(Some(lan), None, false).is_err());
        assert!(caller_allowed(Some(lan), None, true).is_ok());
        // A web page, even from this machine's browser, is not an agent.
        assert!(caller_allowed(Some(local), Some("https://evil.example"), false).is_err());
        assert!(caller_allowed(Some(local), Some("http://localhost:5173"), false).is_ok());
        assert!(caller_allowed(Some(local), Some("http://[::1]:5173"), false).is_ok());
        assert!(caller_allowed(Some(local), Some("http://127.0.0.1.evil.example"), false).is_err());
        assert!(caller_allowed(None, None, false).is_ok());
    }

    #[test]
    fn prompts_and_resources_name_real_tools() {
        let tools: BTreeSet<&str> = catalog().iter().map(|tool| tool.name.as_str()).collect();
        let inputs: BTreeSet<&str> = catalog()
            .iter()
            .flat_map(|tool| tool.input["properties"].as_object().into_iter().flatten())
            .map(|(name, _)| name.as_str())
            .collect();
        for (uri, _, tool) in mcp_content::STATE {
            assert!(
                tool.is_empty() || tools.contains(tool),
                "{uri} reads missing tool {tool}"
            );
        }
        assert!(tools.contains("events_get"));
        // Words in backticks that are neither a tool nor an input: error codes and the like.
        let other = ["no_scan_data"];
        let backticked = regex::Regex::new(r"`([a-z][a-z0-9_]*)`").unwrap();
        for (name, _, _, steps) in mcp_content::PROMPTS {
            for found in backticked.captures_iter(steps) {
                let word = &found[1];
                if word.contains('_') {
                    assert!(
                        tools.contains(word) || inputs.contains(word) || other.contains(&word),
                        "prompt {name} names `{word}`, which is not a tool or an input"
                    );
                }
            }
        }
    }

    #[test]
    fn prompts_fill_in_their_arguments() {
        let text = |name: &str, args: Value| {
            mcp_content::prompt(name, &args).map(|prompt| {
                prompt["messages"][0]["content"]["text"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
        };
        assert!(text("post_service_upload", json!({})).is_err());
        let given = text("post_service_upload", json!({ "event_id": "abc" })).unwrap();
        assert!(given.contains("event abc") && !given.contains("{event_id}"));
        assert!(
            text("prepare_service", json!({}))
                .unwrap()
                .contains("not given")
        );
        assert!(text("no_such_prompt", json!({})).is_err());
    }

    #[test]
    fn a_push_marks_its_resources_stale() {
        let changed = json!({ "type": "event.changed", "data": { "record": { "id": "e1" } } });
        assert_eq!(
            mcp_content::affected(&changed),
            ["metocast://state/events", "metocast://state/events/e1"]
        );
        assert!(mcp_content::affected(&json!({ "type": "transcription.partial" })).is_empty());
        for (uri, _, _) in mcp_content::STATE {
            assert!(super::is_state_uri(uri));
        }
        assert!(super::is_state_uri(
            "metocast://state/events/00000000-0000-4000-8000-000000000000"
        ));
        assert!(!super::is_state_uri("metocast://docs/openapi.json"));
    }

    /// The HTTP side of the rule, without a socket: what a non-loopback caller is answered with.
    #[test]
    fn the_gate_answers_with_the_right_status() {
        let parts = |peer: Option<&str>, origin: Option<&str>| {
            let mut request = axum::http::Request::builder().method("POST").uri("/mcp");
            if let Some(origin) = origin {
                request = request.header(header::ORIGIN, origin);
            }
            let mut request = request.body(()).unwrap();
            if let Some(peer) = peer {
                let peer: SocketAddr = peer.parse().unwrap();
                request.extensions_mut().insert(ConnectInfo(peer));
            }
            request.into_parts().0
        };
        let status =
            |config: &McpConfig, parts: &axum::http::request::Parts| match gate(config, parts) {
                Ok(_) => StatusCode::OK,
                Err(response) => response.status(),
            };
        let on = McpConfig {
            enabled: true,
            ..McpConfig::default()
        };
        let lan = McpConfig {
            allow_lan: true,
            ..on.clone()
        };
        assert_eq!(
            status(&McpConfig::default(), &parts(Some("127.0.0.1:1"), None)),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            status(&on, &parts(Some("127.0.0.1:1"), None)),
            StatusCode::OK
        );
        assert_eq!(
            status(&on, &parts(Some("192.168.0.27:5000"), None)),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(
                &on,
                &parts(Some("127.0.0.1:1"), Some("https://evil.example"))
            ),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(&lan, &parts(Some("192.168.0.27:5000"), None)),
            StatusCode::OK
        );
    }

    #[test]
    fn events_poll_keeps_a_cursor() {
        let events = Events::default();
        for n in 0..3 {
            events.push(json!({ "type": if n == 1 { "b" } else { "a" }, "n": n }));
        }
        let all = events.since(None, &[], 100);
        assert_eq!(all["events"].as_array().unwrap().len(), 3);
        assert_eq!(all["next"], 3);
        let after = events.since(Some(1), &["a".into()], 100);
        assert_eq!(after["events"].as_array().unwrap().len(), 1);
        assert_eq!(after["events"][0]["event"]["n"], 2);
        assert_eq!(
            events.since(Some(3), &[], 100)["events"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
    }

    /// Run `UPDATE_GOLDEN=1 cargo test -p metocast-server generated_docs` after changing a tool.
    #[test]
    fn generated_docs_are_current() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for (file, text) in [
            ("docs/mcp/TOOLS.md", tools_markdown()),
            ("docs/api/WEBSOCKET.md", websocket_markdown()),
        ] {
            let path = root.join(file);
            if std::env::var_os("UPDATE_GOLDEN").is_some() {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, &text).unwrap();
            }
            assert!(
                std::fs::read_to_string(&path).unwrap_or_default() == text,
                "{file} is stale: UPDATE_GOLDEN=1 cargo test -p metocast-server generated_docs"
            );
        }
    }
}

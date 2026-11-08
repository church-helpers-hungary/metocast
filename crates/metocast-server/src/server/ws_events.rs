//! Every message the server sends over `/ws` that is not simply a command's own name echoed back:
//! pushes, and replies that carry a name no command owns. It feeds `docs/api/WEBSOCKET.md`, the
//! JSON Schema file `docs/api/ws-events.schema.json`, and two guards: `ws_events_catalogued`
//! fails when a `"type": "…"` literal in the sources is missing here, and
//! `e2e/ws/event-payloads.test.ts` validates the messages a live server really sends against
//! the schemas.
//!
//! Each entry is `(type, when it is sent, [(field, type)])`. A field's type is `string`, `uuid`,
//! `datetime`, `integer`, `number`, `boolean`, `object`, `array` or `any`; a trailing `?` means it
//! may be null. A message is a flat object: `type` plus those fields.

use serde_json::{Map, Value, json};

/// `(type, when it is sent, fields)`.
pub type Event = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

pub const EVENTS: &[Event] = &[
    (
        "automation.run",
        "An automation rule fired, with what it did",
        &[("run", "object")],
    ),
    (
        "automations.changed",
        "The automation rules or the armed switch changed",
        &[("armed", "boolean"), ("automations", "array")],
    ),
    (
        "blackmagic-camera.discovered",
        "Reply to `blackmagic-camera.discover`: cameras found",
        &[("cameras", "array")],
    ),
    (
        "broadlink.device.discovered",
        "A Broadlink device answered a discovery scan",
        &[("device", "object")],
    ),
    (
        "broadlink.learn.result",
        "A learn session ended with a code, or timed out",
        &[("code", "string?"), ("error", "string?")],
    ),
    (
        "captions.job.state",
        "A caption job for a recording changed state",
        &[
            ("jobId", "uuid"),
            ("recordingId", "uuid"),
            ("state", "string"),
            ("progress", "number"),
            ("error", "string?"),
        ],
    ),
    (
        "clients.updated",
        "A client connected, disconnected or reported its label",
        &[("clients", "array")],
    ),
    (
        "connected",
        "First message on every socket, with the server id",
        &[("serverId", "string")],
    ),
    (
        "connector.state",
        "A connector's live state (OBS output, camera, ATEM, Middle Control)",
        &[
            ("connector", "string"),
            ("isStreaming", "boolean?"),
            ("isRecording", "boolean?"),
            ("streamStatus", "string?"),
            ("hasMedia", "boolean?"),
            ("destination", "object?"),
            ("state", "object?"),
        ],
    ),
    (
        "connector.status",
        "A connector connected, disconnected or failed",
        &[("connector", "string"), ("status", "object")],
    ),
    (
        "cron.youtube_pull",
        "A scheduled YouTube pull ran",
        &[("hasLive", "boolean")],
    ),
    (
        "event.changed",
        "A service event was inserted, updated or deleted, or a broadcast's `scheduleStatus` changed (`scheduled`, `starting`, `live`, `ended`)",
        &[("data", "object")],
    ),
    (
        "event.order.changed",
        "An event's Order of Service was replaced; carries the new list",
        &[("eventId", "uuid"), ("items", "array")],
    ),
    (
        "keynote.status",
        "Keynote status, sent on connect and on change (macOS)",
        &[("status", "object")],
    ),
    (
        "media.audio.progress",
        "Progress of an audio master render",
        &[("progress", "object")],
    ),
    (
        "media.program.progress",
        "Progress of a program job: its render, a nudge, or transcripts added to it (step `captions`)",
        &[("progress", "object")],
    ),
    (
        "media.render.progress",
        "Progress of a video render",
        &[("progress", "object")],
    ),
    (
        "notification",
        "A message for the operator, with a level",
        &[("level", "string"), ("message", "string")],
    ),
    (
        "obs.state",
        "OBS streaming and recording state changed",
        &[("isStreaming", "boolean"), ("isRecording", "boolean")],
    ),
    (
        "order.state",
        "The Order of Service step on screen and its event's steps; sent on connect, on change and for `order.status`",
        &[("position", "object?"), ("items", "array")],
    ),
    (
        "ping",
        "Latency probe; answer with `pong`",
        &[("pingId", "integer")],
    ),
    ("ppt.folders_changed", "The slide folders list changed", &[]),
    (
        "ppt.index.state",
        "The slide library index started or finished parsing decks; also sent on connect",
        &[("indexing", "boolean"), ("pending", "integer")],
    ),
    (
        "ppt.search_results",
        "Reply to `ppt.search`",
        &[("files", "array"), ("filter", "string")],
    ),
    (
        "presentation.settings",
        "Reply to `presentation.get_settings`, and sent on change",
        &[
            ("useWebPresenter", "boolean"),
            ("presenterTheme", "string"),
            ("deliveryLog", "boolean"),
        ],
    ),
    (
        "presenter.slide_changed",
        "The presenter moved to another slide",
        &[("currentSlide", "integer?"), ("totalSlides", "integer?")],
    ),
    (
        "presenter.state",
        "The full web-presenter state. `svgOmitted` is true for a client connected with `/ws?presenter=frames`: each `svgSlides[].svg` is empty and the slides come from `presenter.frame`",
        &[("state", "object"), ("svgOmitted", "boolean")],
    ),
    (
        "queue.stats",
        "Job queue counters; sent while any client is connected",
        &[("queues", "array")],
    ),
    (
        "recorder.stopped",
        "A recording stopped: which recorder, why, and what it said",
        &[
            ("target", "object"),
            ("reason", "string"),
            ("error", "string?"),
        ],
    ),
    (
        "recorders.state",
        "The Record button's recorders: selected, connected, recording, last start",
        &[("recorders", "array")],
    ),
    (
        "recording.changed",
        "A recording was inserted or updated",
        &[("data", "object")],
    ),
    (
        "recording.detected",
        "A new recording file appeared on disk",
        &[("fileName", "string"), ("eventTitle", "string?")],
    ),
    (
        "recording.untracked.removed",
        "An untracked recording was assigned or deleted",
        &[("id", "uuid")],
    ),
    (
        "rodecaster.audio.discovery",
        "Reply to `rodecaster.audio.discover`",
        &[("discovery", "object")],
    ),
    (
        "rodecaster.mute",
        "A RØDECaster channel was muted or unmuted",
        &[
            ("channel", "integer"),
            ("label", "string"),
            ("muted", "boolean"),
            ("remote", "boolean"),
            ("notify", "boolean"),
        ],
    ),
    (
        "rodecaster.record.state",
        "The RØDECaster desk recorder state",
        &[("state", "object?")],
    ),
    (
        "transcript.cleanup.state",
        "A stored transcript's AI clean-up ran a step, finished or failed",
        &[
            ("sessionId", "uuid"),
            ("state", "string"),
            ("progress", "number"),
            ("error", "string?"),
        ],
    ),
    (
        "transcription.credit",
        "OpenAI credit balance for live transcription",
        &[("credit", "object")],
    ),
    (
        "transcription.partial",
        "A live transcript line still being spoken",
        &[("text", "string")],
    ),
    (
        "transcription.segment",
        "A finished live transcript line",
        &[("segment", "object")],
    ),
    (
        "transcription.snapshot",
        "The transcript so far, sent on connect",
        &[
            ("state", "object"),
            ("segments", "array"),
            ("partial", "string?"),
            ("credit", "object?"),
        ],
    ),
    (
        "transcription.state",
        "Live transcription started, stopped or failed",
        &[("state", "object")],
    ),
    (
        "upload.completed",
        "An upload finished",
        &[
            ("recordingId", "uuid"),
            ("platform", "string"),
            ("videoId", "string"),
            ("videoUrl", "string"),
        ],
    ),
    (
        "upload.failed",
        "An upload failed",
        &[
            ("recordingId", "uuid"),
            ("platform", "string"),
            ("error", "string"),
        ],
    ),
    (
        "upload.paused",
        "An upload paused",
        &[("recordingId", "uuid"), ("reason", "string")],
    ),
    (
        "upload.progress",
        "Bytes sent so far for an upload",
        &[
            ("recordingId", "uuid"),
            ("platform", "string"),
            ("progressBytes", "integer"),
            ("totalBytes", "integer"),
        ],
    ),
];

/// The JSON Schema of one message: a flat object with `type` and the fields.
#[must_use]
pub fn schema(kind: &str, fields: &[(&str, &str)]) -> Value {
    let mut properties = Map::new();
    properties.insert("type".into(), json!({ "const": kind }));
    let mut required = vec![json!("type")];
    for (name, declared) in fields {
        let (base, nullable) = declared
            .strip_suffix('?')
            .map_or((*declared, false), |base| (base, true));
        let schema = match base {
            "string" => json!({ "type": "string" }),
            "uuid" => json!({ "type": "string", "format": "uuid" }),
            "datetime" => json!({ "type": "string", "format": "date-time" }),
            "integer" => json!({ "type": "integer" }),
            "number" => json!({ "type": "number" }),
            "boolean" => json!({ "type": "boolean" }),
            "object" => json!({ "type": "object" }),
            "array" => json!({ "type": "array" }),
            _ => json!({}),
        };
        let schema = if nullable {
            json!({ "anyOf": [schema, { "type": "null" }] })
        } else {
            schema
        };
        properties.insert((*name).to_string(), schema);
        if !nullable {
            required.push(json!(name));
        }
    }
    json!({ "type": "object", "properties": properties, "required": required })
}

/// `{ "<type>": schema }` for every catalogued message, written to
/// `docs/api/ws-events.schema.json`.
#[must_use]
pub fn schemas() -> Value {
    let mut all = Map::new();
    for (kind, _, fields) in EVENTS {
        all.insert((*kind).to_string(), schema(kind, fields));
    }
    Value::Object(all)
}

/// `"type": "…"` literals that are not WebSocket messages: JSON Schema words, nested status
/// objects, OpenAI's own realtime messages, and the unknown command a test sends.
#[cfg(test)]
const NOT_MESSAGES: &[&str] = &[
    "array",
    "blackmagic.record.stop",
    "boolean",
    "disconnected",
    "error",
    "http",
    "integer",
    "json_schema",
    "null",
    "number",
    "object",
    "ok",
    "session.created",
    "session.output_transcript.delta",
    "string",
    "textus",
];

#[cfg(test)]
mod tests {
    use super::{EVENTS, NOT_MESSAGES};
    use std::collections::BTreeSet;
    use std::path::Path;

    fn sources(dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs")
                && !matches!(
                    path.file_name().and_then(|n| n.to_str()),
                    Some("openapi.rs" | "mcp.rs" | "mcp_content.rs" | "ws_events.rs")
                )
            {
                out.push(std::fs::read_to_string(&path).unwrap());
            }
        }
    }

    #[test]
    fn every_field_has_a_known_type() {
        let known = [
            "string", "uuid", "datetime", "integer", "number", "boolean", "object", "array", "any",
        ];
        let mut seen = BTreeSet::new();
        for (kind, when, fields) in EVENTS {
            assert!(seen.insert(*kind), "{kind} is listed twice");
            assert!(!when.is_empty(), "{kind} has no description");
            for (name, declared) in *fields {
                let base = declared.strip_suffix('?').unwrap_or(declared);
                assert!(
                    known.contains(&base),
                    "{kind}.{name}: unknown type {declared}"
                );
            }
        }
    }

    /// `UPDATE_GOLDEN=1 cargo test -p metocast-server --lib -- ws_events` rewrites the file.
    #[test]
    fn the_schema_file_is_current() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/api/ws-events.schema.json");
        let text = serde_json::to_string_pretty(&super::schemas()).unwrap() + "\n";
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &text).unwrap();
        }
        assert!(
            std::fs::read_to_string(&path).unwrap_or_default() == text,
            "docs/api/ws-events.schema.json is stale: UPDATE_GOLDEN=1 cargo test -p metocast-server --lib -- ws_events"
        );
    }

    /// Crude on purpose: any `"type": "x"` literal must be a catalogued message, a command name
    /// (replies reuse it) or a known non-message. New pushes fail here until they are listed.
    #[test]
    fn ws_events_catalogued() {
        let mut files = Vec::new();
        sources(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        let literal = regex::Regex::new(r#""type"\s*:\s*"([A-Za-z0-9_.\-]+)""#).unwrap();
        let commands: BTreeSet<String> =
            super::super::mcp::ws_command_names().into_iter().collect();
        let known: BTreeSet<&str> = EVENTS
            .iter()
            .map(|(name, _, _)| *name)
            .chain(NOT_MESSAGES.iter().copied())
            .collect();
        assert!(
            files.len() > 20,
            "the source scan found {} files",
            files.len()
        );
        let mut missing = BTreeSet::new();
        for file in &files {
            for found in literal.captures_iter(file) {
                let name = &found[1];
                if !known.contains(name) && !commands.contains(name) {
                    missing.insert(name.to_string());
                }
            }
        }
        assert!(
            missing.is_empty(),
            "add these to server/ws_events.rs (or NOT_MESSAGES): {missing:?}"
        );
    }
}

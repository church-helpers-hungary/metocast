use axum::Json;
use axum::response::{Html, IntoResponse};
use serde_json::{Value, json};

const DOCS_HTML: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Metocast API Reference</title>
  </head>
  <body>
    <script id="api-reference" data-url="/openapi.json"></script>
    <script src="https://cdn.jsdelivr.net/npm/@scalar/api-reference"></script>
  </body>
</html>"#;

pub async fn serve_spec() -> impl IntoResponse {
    Json(spec())
}

pub async fn serve_docs() -> Html<&'static str> {
    Html(DOCS_HTML)
}

/// The hand-written spec plus a stub for every route it leaves out; see `mcp::document_missing`.
pub fn spec() -> Value {
    let mut spec = handwritten();
    crate::server::mcp::document_missing(&mut spec);
    spec
}

fn handwritten() -> Value {
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Metocast API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "REST API and WebSocket interface for the Metocast desktop application.\n\n## Authentication\n\nAll `/api/*` endpoints require a **Bearer token** in the `Authorization` header:\n```\nAuthorization: Bearer <token>\n```\nThe token is displayed in the app's *Connection Guide* screen. It stays the same across restarts until the operator rotates it.\n\n## WebSocket — real-time push stream\n\n> **Note:** WebSocket is not an HTTP operation and cannot be tested from this page. Use a WebSocket client (e.g. [Hoppscotch](https://hoppscotch.io), [websocat](https://github.com/vi/websocat), or Bruno's socket type).\n\n**Endpoint:** `ws://<host>/ws?token=<token>`\n\nAuthentication uses the same bearer token passed as a **query parameter** (headers are not available during the WebSocket handshake).\n\n### Initial messages (sent immediately on connect)\n\n```json\n{ \"type\": \"connected\", \"serverId\": \"<uuid>\" }\n{ \"type\": \"connector.status\", \"connector\": \"obs\",  \"status\": { \"type\": \"connected\" } }\n{ \"type\": \"connector.status\", \"connector\": \"vmix\", \"status\": { \"type\": \"disconnected\" } }\n```\n\n### Push messages (broadcast on change)\n\n| `type` | Trigger | Schema |\n|---|---|---|\n| `connector.status` | OBS or VMix connection state changes | `WsConnectorStatusMessage` |\n| `event.changed` | Event created, updated, or deleted | `WsEventChangedMessage` |\n| `recording.changed` | Recording created or updated | `WsRecordingChangedMessage` |\n\n```json\n{ \"type\": \"connector.status\", \"connector\": \"obs\", \"status\": { \"type\": \"error\", \"message\": \"connection refused\" } }\n{ \"type\": \"event.changed\",     \"data\": { \"operation\": \"INSERT\", \"record\": { ...Event } } }\n{ \"type\": \"recording.changed\", \"data\": { \"operation\": \"UPDATE\", \"record\": { ...Recording } } }\n```\n\nFull payload definitions are in the `Ws*Message` schemas below."
        },
        "servers": [
            {
                "url": "/",
                "description": "Current server — replace host and port as needed (default port: 3737)"
            }
        ],
        "security": [
            { "bearerAuth": [] }
        ],
        "tags": [
            { "name": "Events",     "description": "Service events" },
            { "name": "Recordings", "description": "Video recording files linked to events" },
            { "name": "Connectors", "description": "Connector status, configuration and control" },
            { "name": "Bible",      "description": "Bible passage lookups and reference autocomplete" },
            { "name": "Presenter",  "description": "Web presenter — parse .pptx files and push slide changes to all connected browsers" },
            { "name": "Media",      "description": "Video processing — describe a video file on the server's disk and render it to a graded HEVC master for YouTube" },
            { "name": "Transcription", "description": "Hungarian → English live transcript (OpenAI) and English captions for finished videos" },
            { "name": "Automations", "description": "Rules that run inside the core, so they work with no UI open: when one device does something (a Middle Control preset finishes moving, a RØDECaster channel goes live or silent), Metocast does something on others (move a camera, start or stop recorders)" },
            { "name": "WebSocket",  "description": "Real-time push stream — requires a WebSocket client, not HTTP" }
        ],
        "components": {
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "description": "Token shown in the app Connection Guide. Stays the same across restarts until rotated."
                }
            },
            "schemas": {
                "WelcomeSettings": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "maxLength": 120, "description": "The greeting a new event's welcome step is named after", "example": "Isten hozott." },
                        "subtitle": { "type": "string", "maxLength": 200, "description": "Shown under the title when the step has no notes of its own", "example": "Az ÚR áldása legyen életeden!" },
                        "churchName": { "type": "string", "maxLength": 120, "description": "Shown above the event's date at the bottom", "example": "Nyíregyházi Metodista Gyülekezet" },
                        "language": { "type": "string", "enum": ["hu", "en"], "description": "The language the date is written in: 2026. október 25. vasárnap, or Sunday, October 25, 2026", "example": "hu" },
                        "hasLogo": { "type": "boolean", "readOnly": true },
                        "replacedPhotos": { "type": "array", "items": { "type": "integer", "minimum": 1, "maximum": 12 }, "readOnly": true, "description": "Months whose photo was replaced by an upload" }
                    }
                },
                "SlideContent": {
                    "type": "object",
                    "description": "Text content extracted from a single slide.",
                    "required": ["index", "paragraphs"],
                    "properties": {
                        "index": { "type": "integer", "minimum": 1, "description": "1-based slide number" },
                        "paragraphs": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "required": ["lines", "align", "fontSizePt"],
                                "properties": {
                                    "lines": { "type": "array", "items": { "type": "string" } },
                                    "align": { "type": "string", "enum": ["left", "center", "right", "justify"] },
                                    "fontSizePt": { "type": "number" }
                                }
                            }
                        }
                    }
                },
                "SvgSlideContent": {
                    "type": "object",
                    "description": "Self-contained SVG rendering of a single slide.",
                    "required": ["index", "svg", "widthPx", "heightPx"],
                    "properties": {
                        "index": { "type": "integer", "minimum": 1 },
                        "svg": { "type": "string", "description": "Inline SVG document. Embedded images are data URIs." },
                        "widthPx": { "type": "integer", "minimum": 1 },
                        "heightPx": { "type": "integer", "minimum": 1 }
                    }
                },
                "PresenterTheme": {
                    "type": "string",
                    "enum": ["classic", "editorial"],
                    "description": "Persisted design returned as presenterTheme in presentation.settings and changed with presentation.set_presenter_theme. Classic shows source SVG whenever it is available; Editorial renders the extracted song or Bible text in the shared warm-black design."
                },
                "PresenterState": {
                    "type": "object",
                    "required": ["loaded", "filePath", "currentSlide", "totalSlides", "renderMode", "slides", "svgSlides", "muted"],
                    "properties": {
                        "loaded": { "type": "boolean" },
                        "filePath": { "type": ["string", "null"] },
                        "currentSlide": { "type": "integer", "minimum": 0 },
                        "totalSlides": { "type": "integer", "minimum": 0 },
                        "renderMode": { "type": "string", "enum": ["text", "svg"] },
                        "slides": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/SlideContent" },
                            "description": "Text slides for text mode and fallback/editing data."
                        },
                        "svgSlides": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/SvgSlideContent" },
                            "description": "SVG slides when renderMode is svg."
                        },
                        "muted": { "type": "boolean" },
                        "slideWidthEmu": { "type": "integer" },
                        "slideHeightEmu": { "type": "integer" }
                    }
                },
                "ParsedPresentation": {
                    "type": "object",
                    "description": "Structured text content extracted from a parsed .pptx file.",
                    "required": ["filePath", "totalSlides", "slides", "slideWidthEmu", "slideHeightEmu"],
                    "properties": {
                        "filePath":    { "type": "string", "example": "/Users/admin/Presentations/sunday-service.pptx" },
                        "totalSlides": { "type": "integer", "minimum": 0 },
                        "slides": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/SlideContent" }
                        },
                        "slideWidthEmu": { "type": "integer" },
                        "slideHeightEmu": { "type": "integer" }
                    }
                },
                "BibleReference": {
                    "type": "object",
                    "required": ["type", "reference", "translation", "verses"],
                    "properties": {
                        "type":        { "type": "string", "enum": ["textus", "leckio"] },
                        "reference":   { "type": "string", "example": "John 3:16" },
                        "translation": { "type": "string", "example": "UF" },
                        "verses": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "required": ["chapter", "verse", "text"],
                                "properties": {
                                    "chapter": { "type": "integer" },
                                    "verse":   { "type": "integer" },
                                    "text":    { "type": "string" }
                                }
                            }
                        }
                    }
                },
                "TitleTemplate": {
                    "type": "object",
                    "description": "Template for the published event title. Placeholders are `{name}`; `{date}` takes an optional pipe (`{date|YYYY.MM.DD.}`). Text in `[ ... ]` is dropped whole when a placeholder inside it is empty.",
                    "required": ["template"],
                    "properties": {
                        "template": {
                            "type": "string",
                            "example": "{date|YYYY.MM.DD.} {title}[ | Textus: {textus}][ Lekció: {leckio}][ | {speaker}]"
                        }
                    }
                },
                "SlideFolder": {
                    "type": "object",
                    "description": "A generated-slide output folder on the core's machine. Empty means not configured.",
                    "required": ["path"],
                    "properties": {
                        "path": { "type": "string", "example": "/Users/me/Documents/Slides" }
                    }
                },
                "Event": {
                    "type": "object",
                    "description": "Full event record including platform connections and bible references.",
                    "required": [
                        "id", "title", "dateTime", "speaker", "description",
                        "autoUploadEnabled", "connections", "bibleReferences", "createdAt", "updatedAt"
                    ],
                    "properties": {
                        "id":               { "type": "string", "format": "uuid" },
                        "title":            { "type": "string", "example": "Sunday Morning Service" },
                        "computedTitle":    { "type": "string", "description": "Composed \"date · reference · title · — speaker\" line published to YouTube. Empty falls back to title.", "example": "Sun, September 6, 2026 · 2Kor 1,1-10 · Worship Service — Pastor Smith" },
                        "dateTime":         { "type": "string", "format": "date-time", "description": "Scheduled date and time (ISO 8601 / UTC)" },
                        "speaker":          { "type": "string", "example": "Pastor Smith" },
                        "description":      { "type": "string" },
                        "autoUploadEnabled":{ "type": "boolean" },
                        "connections":      { "type": "array", "items": { "$ref": "#/components/schemas/EventConnection" } },
                        "bibleReferences":  { "type": "array", "items": { "$ref": "#/components/schemas/BibleReference" } },
                        "createdAt":        { "type": "string", "format": "date-time" },
                        "updatedAt":        { "type": "string", "format": "date-time" }
                    }
                },
                "EventSummary": {
                    "type": "object",
                    "description": "Lightweight event entry returned by the list endpoint. Omits large text fields; adds a recording count.",
                    "required": ["id", "title", "dateTime", "speaker", "recordingCount", "createdAt", "updatedAt"],
                    "properties": {
                        "id":             { "type": "string", "format": "uuid" },
                        "title":          { "type": "string", "example": "Sunday Morning Service" },
                        "computedTitle":  { "type": "string", "description": "Composed title published to YouTube. Empty falls back to title." },
                        "dateTime":       { "type": "string", "format": "date-time" },
                        "speaker":        { "type": "string" },
                        "recordingCount": { "type": "integer", "format": "int64", "description": "Number of recording files attached to this event" },
                        "createdAt":      { "type": "string", "format": "date-time" },
                        "updatedAt":      { "type": "string", "format": "date-time" },
                        "connections":    { "type": "array", "items": { "$ref": "#/components/schemas/EventConnection" }, "description": "The event's YouTube and Facebook broadcasts" }
                    }
                },
                "CreateEventRequest": {
                    "type": "object",
                    "description": "Request body for creating or fully replacing an event. Field names are **snake_case**.",
                    "required": ["title", "date_time"],
                    "properties": {
                        "title":             { "type": "string", "example": "Sunday Morning Service" },
                        "computed_title":    { "type": "string", "default": "", "description": "Composed title to publish to YouTube. Omit or leave empty to publish `title` instead." },
                        "date_time":         { "type": "string", "format": "date-time", "description": "Scheduled date and time" },
                        "speaker":           { "type": "string", "default": "" },
                        "description":       { "type": "string", "default": "" },
                        "auto_upload_enabled": { "type": "boolean", "default": false },
                        "bible_references":  {
                            "type": "array",
                            "description": "Bible readings for the event. Each entry has a type (textus or leckio). An empty reference string removes the existing entry.",
                            "items": {
                                "type": "object",
                                "required": ["type"],
                                "properties": {
                                    "type":        { "type": "string", "enum": ["textus", "leckio"] },
                                    "reference":   { "type": "string" },
                                    "translation": { "type": "string", "default": "UF" },
                                    "verses":      { "type": "array", "items": { "$ref": "#/components/schemas/BibleReference/properties/verses/items" } }
                                }
                            }
                        },
                        "connections": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "required": ["platform"],
                                "properties": {
                                    "platform":       { "type": "string" },
                                    "privacy_status": { "type": "string" }
                                }
                            }
                        }
                    }
                },
                "Recording": {
                    "type": "object",
                    "description": "Video or audio recording file linked to an event.",
                    "required": [
                        "id", "eventId", "filePath", "fileName", "fileSize",
                        "durationSeconds", "mediaKind", "source", "metadata", "detectedAt", "whitelisted", "uploaded",
                        "createdAt", "updatedAt"
                    ],
                    "properties": {
                        "id":              { "type": "string", "format": "uuid" },
                        "eventId":         { "type": "string", "format": "uuid" },
                        "filePath":        { "type": "string", "description": "Absolute path on the server's filesystem" },
                        "fileName":        { "type": "string", "example": "service-2025-01-19.mp4" },
                        "fileSize":        { "type": "integer", "format": "int64", "description": "File size in bytes" },
                        "durationSeconds": { "type": "number",  "format": "double",  "description": "Duration in seconds" },
                        "mediaKind":      { "type": "string", "description": "Media kind such as video, deviceMix, deviceTrack, derivedMix, or deviceMultichannel" },
                        "source":         { "type": "string", "description": "Originating recorder, such as obs or rodecaster" },
                        "metadata":       { "type": "object", "additionalProperties": true, "description": "Recorder-specific source mapping and provenance snapshot" },
                        "detectedAt":      { "type": "string",  "format": "date-time", "description": "When the file was detected or added" },
                        "whitelisted":     { "type": "boolean", "description": "Approved for YouTube upload" },
                        "uploaded":        { "type": "boolean", "description": "Whether the file has been uploaded to YouTube" },
                        "uploadedAt":      { "type": ["string", "null"], "format": "date-time" },
                        "videoId":         { "type": ["string", "null"], "description": "YouTube video ID (set after upload)" },
                        "videoUrl":        { "type": ["string", "null"], "description": "YouTube watch URL (set after upload)" },
                        "customTitle":     { "type": ["string", "null"], "description": "Custom YouTube title; falls back to the event title when null" },
                        "createdAt":       { "type": "string", "format": "date-time" },
                        "updatedAt":       { "type": "string", "format": "date-time" }
                    }
                },
                "CreateRecordingRequest": {
                    "type": "object",
                    "description": "Request body for registering a new recording. Field names are **snake_case**.",
                    "required": ["file_path", "file_name"],
                    "properties": {
                        "file_path":        { "type": "string", "description": "Absolute path to the recording file" },
                        "file_name":        { "type": "string", "example": "service-2025-01-19.mp4" },
                        "file_size":        { "type": "integer", "format": "int64", "default": 0, "description": "File size in bytes" },
                        "duration_seconds": { "type": "number",  "format": "double",  "default": 0.0 },
                        "custom_title":     { "type": "string",  "description": "Optional custom YouTube title" }
                    }
                },
                "ConnectorStatus": {
                    "description": "Discriminated union representing the current connection state of a streaming connector. Discriminator field: `type`.",
                    "oneOf": [
                        {
                            "type": "object",
                            "title": "Disconnected",
                            "required": ["type"],
                            "properties": {
                                "type": { "type": "string", "enum": ["disconnected"] }
                            }
                        },
                        {
                            "type": "object",
                            "title": "Connecting",
                            "description": "A connection attempt is in progress.",
                            "required": ["type"],
                            "properties": {
                                "type": { "type": "string", "enum": ["connecting"] }
                            }
                        },
                        {
                            "type": "object",
                            "title": "Connected",
                            "required": ["type"],
                            "properties": {
                                "type": { "type": "string", "enum": ["connected"] }
                            }
                        },
                        {
                            "type": "object",
                            "title": "Error",
                            "description": "The last connection attempt failed.",
                            "required": ["type", "message"],
                            "properties": {
                                "type":    { "type": "string", "enum": ["error"] },
                                "message": { "type": "string", "description": "Human-readable error description" }
                            }
                        }
                    ],
                    "discriminator": {
                        "propertyName": "type"
                    }
                },
                "ConnectorStatuses": {
                    "type": "object",
                    "description": "Current status of all connectors.",
                    "required": ["obs", "blackmagic-camera", "middlecontrol", "rodecaster", "vmix", "atem", "broadlink", "youtube", "facebook", "discord", "szentiras"],
                    "properties": {
                        "obs":       { "$ref": "#/components/schemas/ConnectorStatus" },
                        "blackmagic-camera": { "$ref": "#/components/schemas/ConnectorStatus" },
                        "middlecontrol": { "$ref": "#/components/schemas/ConnectorStatus" },
                        "rodecaster": { "$ref": "#/components/schemas/ConnectorStatus" },
                        "vmix":      { "$ref": "#/components/schemas/ConnectorStatus" },
                        "atem":      { "$ref": "#/components/schemas/ConnectorStatus" },
                        "broadlink": { "$ref": "#/components/schemas/ConnectorStatus" },
                        "youtube":   { "$ref": "#/components/schemas/ConnectorStatus" },
                        "facebook":  { "$ref": "#/components/schemas/ConnectorStatus" },
                        "discord":   { "$ref": "#/components/schemas/ConnectorStatus" },
                        "szentiras": { "$ref": "#/components/schemas/ConnectorStatus" }
                    }
                },
                "BiblePassage": {
                    "type": "object",
                    "description": "A Bible passage normalised across both upstream APIs.",
                    "required": ["label", "verses"],
                    "properties": {
                        "label": { "type": "string", "example": "János 3,16" },
                        "verses": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "required": ["chapter", "verse", "text"],
                                "properties": {
                                    "chapter": { "type": "integer" },
                                    "verse":   { "type": "integer" },
                                    "text":    { "type": "string", "description": "Verse text with all markup stripped" }
                                }
                            }
                        }
                    }
                },
                "BibleSuggestion": {
                    "type": "object",
                    "required": ["cat", "label", "link"],
                    "properties": {
                        "cat":   { "type": "string", "example": "ref" },
                        "label": { "type": "string", "example": "1Móz 1" },
                        "link":  { "type": "string", "description": "Reference to pass back as the `reference` parameter" }
                    }
                },
                "ConnectorConfig": {
                    "description": "Configuration for one connector. The shape depends on the connector: `obs` (enabled, host, port, password), `blackmagic-camera` (enabled, host, fingerprint, username, password), `middlecontrol` (enabled, host, port; default 11584), `vmix`/`atem` (enabled, host, port), `broadlink` (enabled), `rodecaster` (enabled, notifyOnMute, audioRecording), `youtube` (enabled, clientId, clientSecret), `facebook` (enabled, appId, appSecret, pageId), `discord` (enabled, webhookUrl), `szentiras` (enabled, apiKey).",
                    "type": "object",
                    "required": ["enabled"],
                    "properties": {
                        "enabled":      { "type": "boolean" },
                        "notifyOnMute": { "type": "boolean", "description": "RODECaster: raise a notification when a channel mute state changes on the device." },
                        "audioRecording": { "type": "object", "description": "RØDECaster FLAC recording configuration: schemaVersion, enabled, server directory, processingMode, outputMode, and typed sources. Fields a save leaves out, or the whole object, keep their stored values, so a client that only shows enabled and notifyOnMute cannot reset the recording." },
                        "host":         { "type": "string" },
                        "port":         { "type": "integer" },
                        "fingerprint":  { "type": "string", "description": "Blackmagic camera: pinned SHA-256 of the camera's self-signed certificate. Blank means trust-on-first-use." },
                        "username":     { "type": "string" },
                        "password":     { "type": "string", "nullable": true, "description": "Write-only: reads return an empty string" },
                        "clientId":     { "type": "string" },
                        "clientSecret": { "type": "string", "description": "Write-only: reads return an empty string" },
                        "appId":        { "type": "string" },
                        "appSecret":    { "type": "string", "description": "Write-only: reads return an empty string" },
                        "pageId":       { "type": "string" },
                        "webhookUrl":   { "type": "string", "description": "Write-only: reads return an empty string" },
                        "apiKey":       { "type": "string", "description": "szentiras.eu API key, sent upstream as X-API-Key. Write-only: reads return an empty string." },
                        "passwordSet":    { "type": "boolean", "readOnly": true, "description": "Whether a password is stored" },
                        "clientSecretSet":{ "type": "boolean", "readOnly": true, "description": "Whether a client secret is stored" },
                        "appSecretSet":   { "type": "boolean", "readOnly": true, "description": "Whether an app secret is stored" },
                        "webhookUrlSet":  { "type": "boolean", "readOnly": true, "description": "Whether a webhook URL is stored" },
                        "apiKeySet":      { "type": "boolean", "readOnly": true, "description": "Whether an API key is stored. Send false to clear it." }
                    }
                },
                "CameraStreamTarget": {
                    "type": "object",
                    "description": "The camera's livestream destination after YouTube settings were pushed.",
                    "required": ["rtmpUrl", "platform", "server", "quality"],
                    "properties": {
                        "rtmpUrl":  { "type": "string", "description": "The ingestion address the camera pushes to. The stream key is never returned", "example": "rtmp://a.rtmp.youtube.com/live2" },
                        "platform": { "type": "string", "example": "YouTube" },
                        "server":   { "type": "string", "description": "The platform's server name, or `Custom` when the URL was overridden" },
                        "quality":  { "type": "string", "description": "Quality profile the camera will encode at" },
                        "url":      { "type": "string", "nullable": true, "description": "Set only when the destination is a custom URL rather than a platform entry: the ingestion address, without the key" }
                    }
                },
                "DiscoveredAtem": {
                    "type": "object",
                    "description": "An ATEM switcher advertising `_switcher_ctrl._udp` over mDNS, once per address it answered on.",
                    "required": ["name", "host", "port", "usb"],
                    "properties": {
                        "name": { "type": "string", "description": "The switcher's name as set in ATEM Setup", "example": "MetoDeck" },
                        "host": { "type": "string", "example": "172.25.69.185" },
                        "port": { "type": "integer", "minimum": 1, "maximum": 65535, "example": 9910 },
                        "usb":  { "type": "boolean", "description": "True when the address is on the switcher's own USB network link (a /30 USB Ethernet adapter on the server), false on the LAN" }
                    }
                },
                "AtemStreamTarget": {
                    "type": "object",
                    "description": "The switcher's streaming service after YouTube settings were pushed. The stream key is written to the switcher but never returned.",
                    "required": ["service", "url"],
                    "properties": {
                        "service": { "type": "string", "example": "YouTube RTMP" },
                        "url":     { "type": "string", "example": "rtmp://a.rtmp.youtube.com/live2" }
                    }
                },
                "CameraResolution": {
                    "type": "object",
                    "required": ["width", "height"],
                    "properties": {
                        "width":  { "type": "integer", "example": 6048 },
                        "height": { "type": "integer", "example": 4032 }
                    }
                },
                "CameraPlatformEntry": {
                    "type": "object",
                    "description": "The camera's active livestream platform entry.",
                    "required": ["platform", "server", "quality"],
                    "properties": {
                        "platform":   { "type": "string", "example": "YouTube RTMP" },
                        "server":     { "type": "string", "example": "Primary" },
                        "quality":    { "type": "string", "example": "Streaming High" },
                        "key":        { "type": "string", "nullable": true, "description": "Stream key. Never returned; leave it out to keep the camera's" },
                        "passphrase": { "type": "string", "nullable": true, "description": "SRT streams only. Never returned; leave it out to keep the camera's" },
                        "url":        { "type": "string", "nullable": true, "description": "Set only when the platform's URL is customizable" }
                    }
                },
                "CameraSettings": {
                    "type": "object",
                    "description": "The camera's own payloads, forwarded unchanged: record format, media working set and livestream settings.",
                    "required": ["recording", "record", "storage", "stream"],
                    "properties": {
                        "recording": { "type": "boolean", "description": "Whether the camera is recording now" },
                        "record": {
                            "type": "object",
                            "description": "`/system/format` and `/system/supportedFormats` as the camera returns them",
                            "required": ["format", "supported"],
                            "properties": {
                                "format":    { "type": "object", "additionalProperties": true },
                                "supported": { "type": "object", "additionalProperties": true }
                            }
                        },
                        "storage": {
                            "type": "object",
                            "description": "`/media/slots`, `/media/workingset` and `/media/active`",
                            "required": ["slots", "workingset", "active"],
                            "properties": {
                                "slots":      { "type": "array", "items": { "type": "object", "additionalProperties": true } },
                                "workingset": { "type": "object", "additionalProperties": true },
                                "active":     { "type": "object", "nullable": true, "additionalProperties": true }
                            }
                        },
                        "stream": {
                            "type": "object",
                            "description": "Livestream status, availability, the platform list, the active platform and that platform's servers and quality profiles",
                            "required": ["status", "available", "platforms", "active", "platform"],
                            "properties": {
                                "status":    { "type": "object", "additionalProperties": true },
                                "available": { "type": "object", "additionalProperties": true },
                                "platforms": { "type": "array", "items": { "type": "string" } },
                                "active":    { "$ref": "#/components/schemas/CameraPlatformEntry" },
                                "platform":  { "type": "object", "additionalProperties": true }
                            }
                        }
                    }
                },
                "CameraSettingsUpdate": {
                    "type": "object",
                    "description": "What to write. Both halves are optional; whichever is present is sent to the camera.",
                    "properties": {
                        "record": {
                            "type": "object",
                            "description": "The camera validates a format as a whole, so all four fields travel together.",
                            "required": ["codec", "frameRate", "recordResolution", "sensorResolution"],
                            "properties": {
                                "codec":            { "type": "string", "example": "BRaw:8_1" },
                                "frameRate":        { "type": "string", "example": "24" },
                                "recordResolution": { "$ref": "#/components/schemas/CameraResolution" },
                                "sensorResolution": { "$ref": "#/components/schemas/CameraResolution" }
                            }
                        },
                        "stream": { "$ref": "#/components/schemas/CameraPlatformEntry" }
                    }
                },
                "DiscoveredCamera": {
                    "type": "object",
                    "description": "A Blackmagic camera found by mDNS. `host` is what the connector config's `host` field takes.",
                    "required": ["host", "hostname", "addresses", "port", "deviceName", "productName", "uniqueId", "softwareVersion"],
                    "properties": {
                        "host":            { "type": "string", "example": "http://Cinema-Camera-6K.local" },
                        "hostname":        { "type": "string", "example": "Cinema-Camera-6K.local." },
                        "addresses":       { "type": "array", "items": { "type": "string" } },
                        "port":            { "type": "integer" },
                        "deviceName":      { "type": "string" },
                        "productName":     { "type": "string", "example": "Cinema Camera 6K" },
                        "uniqueId":        { "type": "string", "description": "Stable across renames and DHCP" },
                        "softwareVersion": { "type": "string", "example": "10.2.2" }
                    }
                },
                "DiscoveredMiddlecontrol": {
                    "type": "object",
                    "description": "A Middle Control endpoint that sent a valid feedback frame during discovery.",
                    "required": ["host", "port"],
                    "properties": {
                        "host": { "type": "string", "example": "localhost" },
                        "port": { "type": "integer", "minimum": 1, "maximum": 65535, "example": 11584 }
                    }
                },
                "ObsStreamSettings": {
                    "type": "object",
                    "description": "The RTMP destination OBS streams to.",
                    "required": ["server", "key"],
                    "properties": {
                        "serviceType": { "type": "string", "description": "OBS service type, e.g. `rtmp_custom` (response only)" },
                        "server":      { "type": "string", "example": "rtmp://a.rtmp.youtube.com/live2" },
                        "key":         { "type": "string" }
                    }
                },
                "ChannelSource": {
                    "type": "string",
                    "enum": ["either", "desk", "transmitter"],
                    "default": "either",
                    "description": "Which mute counts when a channel trigger decides the channel is live. `either`: live only while both the desk and the Wireless PRO transmitter are unmuted (so a pastor who mutes on the transmitter goes silent). `desk`: only the desk button, or Metocast's own mute toggle. `transmitter`: only the Wireless PRO button."
                },
                "AutomationTrigger": {
                    "description": "What fires a rule; a rule fires when ANY of its triggers happens. Tagged by `kind`. Nothing fires on server start, on a reconnect or resync, or when a rule is saved: only a change that happens while Metocast is watching.",
                    "oneOf": [
                        {
                            "type": "object",
                            "title": "middlecontrol.preset",
                            "description": "A Middle Control preset's move FINISHED, not when it was clicked, wherever it was clicked (Middle Control's window, the APC-R, a Stream Deck). Metocast follows `PRES_ACTIVE`, which carries the running preset's number on Middle Control 4.0.4, and counts a move as finished when it returns to 0 after at least 90% of the transition time. A move stopped early, one that was running when Metocast connected, a camera switch mid-move and anything during Middle Control's own preset sequence do not count.",
                            "required": ["kind", "preset"],
                            "properties": {
                                "kind": { "type": "string", "const": "middlecontrol.preset" },
                                "preset": { "type": "integer", "minimum": 1, "maximum": 12 },
                                "cameraId": { "type": ["integer", "null"], "minimum": 1, "maximum": 99, "description": "null: any camera" }
                            }
                        },
                        {
                            "type": "object",
                            "title": "rodecaster.channel",
                            "description": "A RØDECaster channel went live or silent, and stayed so for `holdSeconds`.",
                            "required": ["kind", "channel", "live"],
                            "properties": {
                                "kind": { "type": "string", "const": "rodecaster.channel" },
                                "channel": { "type": "integer", "minimum": 0, "maximum": 63, "description": "The zero-based fader slot, as in `rodecaster.mute.set`" },
                                "label": { "type": "string", "maxLength": 80, "description": "A snapshot of the channel's name, for display only" },
                                "live": { "type": "boolean", "description": "`true` fires when the channel goes live, `false` when it goes silent" },
                                "source": { "$ref": "#/components/schemas/ChannelSource" },
                                "holdSeconds": { "type": "number", "minimum": 0, "maximum": 60, "default": 0, "description": "Fire only if the channel is still so after this long (the channel is read again from the profile Metocast keeps). A cough-mute that is undone inside the hold stops nothing; a channel that changes and changes back starts the wait over." }
                            }
                        }
                    ]
                },
                "RecordTarget": {
                    "description": "What a `record.start` or `record.stop` action drives, tagged by `device`. The names match the connectors'.",
                    "oneOf": [
                        { "type": "object", "title": "obs", "required": ["device"], "properties": { "device": { "type": "string", "const": "obs" } } },
                        { "type": "object", "title": "atem", "required": ["device"], "properties": { "device": { "type": "string", "const": "atem" } } },
                        { "type": "object", "title": "blackmagic-camera", "description": "The camera's own recorder, over its REST API", "required": ["device"], "properties": { "device": { "type": "string", "const": "blackmagic-camera" } } },
                        {
                            "type": "object",
                            "title": "middlecontrol",
                            "description": "Cameras recorded through Middle Control (`REC_START`). Ticking this and `blackmagic-camera` may drive the same camera twice.",
                            "required": ["device"],
                            "properties": {
                                "device": { "type": "string", "const": "middlecontrol" },
                                "cameraId": { "type": ["integer", "null"], "minimum": 1, "maximum": 99, "description": "null: every camera" }
                            }
                        },
                        { "type": "object", "title": "rodecaster", "description": "The desk's own SD or USB recorder. A start needs a card or drive: `rodecaster_no_media` otherwise.", "required": ["device"], "properties": { "device": { "type": "string", "const": "rodecaster" } } },
                        { "type": "object", "title": "rodecaster-audio", "description": "Metocast's FLAC recording of the desk's USB audio, filed under the server's current event, or untracked when there is none", "required": ["device"], "properties": { "device": { "type": "string", "const": "rodecaster-audio" } } }
                    ]
                },
                "AutomationAction": {
                    "description": "What a rule does, in the order listed, tagged by `kind`. Recorder actions next to each other start together (each with a 10 s timeout, so one dead device holds up none); a camera move runs on its own. A start rule moves the camera first so recording starts on the right shot; a stop rule stops recording first so the recording doesn't end on a camera move. Each recorder action reads its target's state first, so starting a recorder that is recording, or stopping an idle one, is `skipped`.",
                    "oneOf": [
                        {
                            "type": "object",
                            "title": "record.start",
                            "required": ["kind", "target"],
                            "properties": {
                                "kind": { "type": "string", "const": "record.start" },
                                "target": { "$ref": "#/components/schemas/RecordTarget" }
                            }
                        },
                        {
                            "type": "object",
                            "title": "record.stop",
                            "required": ["kind", "target"],
                            "properties": {
                                "kind": { "type": "string", "const": "record.stop" },
                                "target": { "$ref": "#/components/schemas/RecordTarget" }
                            }
                        },
                        {
                            "type": "object",
                            "title": "middlecontrol.preset.recall",
                            "description": "Moves a camera to a Middle Control preset. Skipped when that same preset is what fired the rule (the camera is already there). With `waitForArrival`, the actions after it wait until the camera arrives: the preset's number in `PRES_ACTIVE` drops back to 0; a camera whose pan and tilt have moved less than 0.5° after 1.5 s counts as arrived (Middle Control runs the whole transition even when the camera is already there); if Middle Control starts no move within 1 s, or the move runs 5 s past its transition time, the rest goes ahead and the run notes it; for a camera that isn't the selected one the rest waits for the transition time (`PRES_D`). If the recall itself fails, the rest goes ahead at once and the run shows the failure.",
                            "required": ["kind", "preset"],
                            "properties": {
                                "kind": { "type": "string", "const": "middlecontrol.preset.recall" },
                                "preset": { "type": "integer", "minimum": 1, "maximum": 12 },
                                "cameraId": { "type": ["integer", "null"], "minimum": 1, "maximum": 99, "description": "null: the selected camera" },
                                "waitForArrival": { "type": "boolean", "default": true }
                            }
                        }
                    ]
                },
                "AutomationDraft": {
                    "type": "object",
                    "description": "A rule as a client writes it (`POST`, and `PUT` which replaces the whole rule). At most 50 rules, names of 1–80 characters, at least one trigger and one action (at most 20 and 40).",
                    "required": ["name", "triggers", "actions"],
                    "properties": {
                        "name": { "type": "string", "minLength": 1, "maxLength": 80, "example": "Service start" },
                        "enabled": { "type": "boolean", "default": true },
                        "triggers": { "type": "array", "items": { "$ref": "#/components/schemas/AutomationTrigger" }, "minItems": 1, "maxItems": 20 },
                        "actions": { "type": "array", "items": { "$ref": "#/components/schemas/AutomationAction" }, "minItems": 1, "maxItems": 40 },
                        "cooldownSeconds": { "type": "number", "minimum": 1, "maximum": 600, "default": 5, "description": "Re-fires of the same rule inside this window are ignored" },
                        "requireEvent": { "type": "boolean", "default": false, "description": "The rule fires only when the server has a current event today" }
                    }
                },
                "Automation": {
                    "type": "object",
                    "required": ["id", "name", "enabled", "triggers", "actions", "cooldownSeconds", "requireEvent", "createdAt", "updatedAt"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid" },
                        "name": { "type": "string" },
                        "enabled": { "type": "boolean" },
                        "triggers": { "type": "array", "items": { "$ref": "#/components/schemas/AutomationTrigger" } },
                        "actions": { "type": "array", "items": { "$ref": "#/components/schemas/AutomationAction" } },
                        "cooldownSeconds": { "type": "number" },
                        "requireEvent": { "type": "boolean" },
                        "createdAt": { "type": "string", "format": "date-time" },
                        "updatedAt": { "type": "string", "format": "date-time" }
                    },
                    "example": {
                        "id": "6f2c1f8e-9a3e-4a8b-9f6e-2d8a1b7c3e10",
                        "name": "Service start",
                        "enabled": true,
                        "triggers": [
                            { "kind": "middlecontrol.preset", "preset": 3, "cameraId": null },
                            { "kind": "rodecaster.channel", "channel": 2, "label": "Wireless 1", "live": true, "source": "either", "holdSeconds": 0 }
                        ],
                        "actions": [
                            { "kind": "middlecontrol.preset.recall", "preset": 3, "cameraId": null, "waitForArrival": true },
                            { "kind": "record.start", "target": { "device": "obs" } },
                            { "kind": "record.start", "target": { "device": "atem" } },
                            { "kind": "record.start", "target": { "device": "blackmagic-camera" } },
                            { "kind": "record.start", "target": { "device": "rodecaster" } },
                            { "kind": "record.start", "target": { "device": "rodecaster-audio" } }
                        ],
                        "cooldownSeconds": 5,
                        "requireEvent": false,
                        "createdAt": "2026-10-07T08:00:00Z",
                        "updatedAt": "2026-10-07T08:00:00Z"
                    }
                },
                "AutomationList": {
                    "type": "object",
                    "required": ["armed", "automations"],
                    "properties": {
                        "armed": { "type": "boolean", "description": "The master switch (`app_settings.automations_enabled`, on by default): nothing fires while it is off, for example during a rehearsal. A manual run ignores it." },
                        "automations": { "type": "array", "items": { "$ref": "#/components/schemas/Automation" } }
                    }
                },
                "ActionResult": {
                    "type": "object",
                    "required": ["action", "outcome"],
                    "properties": {
                        "action": { "type": "string", "description": "What the action was, in words", "example": "Start OBS" },
                        "outcome": { "type": "string", "enum": ["done", "skipped", "failed"] },
                        "message": { "type": ["string", "null"], "description": "Why it was skipped (`already recording`, `not recording`, `that preset is what fired the rule`), a note on a camera wait (`the camera was already there`, `Middle Control did not start the move`), the connector's error code (`obs_not_connected`, `atem_recording_unsupported`, `rodecaster_no_media`, …) or the device's own words" }
                    }
                },
                "AutomationRun": {
                    "type": "object",
                    "description": "One run of a rule. Every run is logged by the core, one line per action, kept in a history of the last 50 runs, and broadcast as `automation.run`; a run with a failure also sends a `notification` at level `warn`.",
                    "required": ["id", "automationId", "name", "cause", "manual", "startedAt", "results"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid" },
                        "automationId": { "type": "string", "format": "uuid" },
                        "name": { "type": "string" },
                        "cause": { "type": "string", "description": "What fired it, in words", "example": "Preset 3 finished on camera 1" },
                        "manual": { "type": "boolean", "description": "\"Run now\", which ignores the cooldown, the event check and the master switch" },
                        "startedAt": { "type": "string", "format": "date-time" },
                        "results": { "type": "array", "items": { "$ref": "#/components/schemas/ActionResult" } }
                    }
                },
                "WsAutomationsChangedMessage": {
                    "type": "object",
                    "description": "Broadcast after any write to the rules or the master switch, sent on connect to clients with the token, and the answer to the `automations.list` command.",
                    "required": ["type", "armed", "automations"],
                    "properties": {
                        "type": { "type": "string", "const": "automations.changed" },
                        "armed": { "type": "boolean" },
                        "automations": { "type": "array", "items": { "$ref": "#/components/schemas/Automation" } }
                    }
                },
                "RecordButton": {
                    "type": "object",
                    "description": "The recorders the Record button starts and stops. Unset is `rodecaster-audio`, Metocast's audio recording.",
                    "required": ["targets"],
                    "properties": {
                        "targets": { "type": "array", "items": { "$ref": "#/components/schemas/RecordTarget" } }
                    }
                },
                "RecorderRow": {
                    "type": "object",
                    "description": "One recorder as the Record button shows it.",
                    "required": ["target", "selected", "available", "recording", "lastStart"],
                    "properties": {
                        "target": { "$ref": "#/components/schemas/RecordTarget" },
                        "selected": { "type": "boolean", "description": "The Record button drives it" },
                        "available": { "type": "boolean", "description": "Connected and able to record" },
                        "recording": { "type": "boolean" },
                        "lastStart": {
                            "type": ["object", "null"],
                            "description": "The last start by the Record button or an automation, since the server started",
                            "required": ["at", "error"],
                            "properties": {
                                "at": { "type": "string", "format": "date-time" },
                                "error": { "type": ["string", "null"], "description": "The refusal code or the device's words; null when it started or already was recording", "example": "camera_no_media" }
                            }
                        }
                    }
                },
                "WsRecordersStateMessage": {
                    "type": "object",
                    "description": "Broadcast when any recorder's row changes: a recording starts or stops, a device comes or goes, a start fails, the selection changes.",
                    "required": ["type", "recorders"],
                    "properties": {
                        "type": { "type": "string", "const": "recorders.state" },
                        "recorders": { "type": "array", "items": { "$ref": "#/components/schemas/RecorderRow" } }
                    }
                },
                "WsRecorderStoppedMessage": {
                    "type": "object",
                    "description": "Broadcast whenever a recording stops, for whatever reason.",
                    "required": ["type", "target", "reason", "error"],
                    "properties": {
                        "type": { "type": "string", "const": "recorder.stopped" },
                        "target": { "$ref": "#/components/schemas/RecordTarget" },
                        "reason": { "type": "string", "enum": ["metocast", "disconnected", "device"], "description": "`metocast`: Metocast asked (the Record button, a Live page button or an automation); `disconnected`: the connection dropped; `device`: the device stopped by itself or someone stopped it there" },
                        "error": { "type": ["string", "null"], "description": "What the recorder said, e.g. `camera_no_media`" }
                    }
                },
                "WsAutomationRunMessage": {
                    "type": "object",
                    "description": "Broadcast after every run.",
                    "required": ["type", "run"],
                    "properties": {
                        "type": { "type": "string", "const": "automation.run" },
                        "run": { "$ref": "#/components/schemas/AutomationRun" }
                    }
                },
                "WsRodecasterRecordStateMessage": {
                    "type": "object",
                    "description": "The RØDECaster's OWN SD or USB recorder (as opposed to Metocast's FLAC recording of its USB audio, `rodecaster.audio.record.state`). Broadcast when it starts, pauses or stops, or a card or drive comes or goes, and sent on connect while the desk is connected.",
                    "required": ["type", "state"],
                    "properties": {
                        "type": { "type": "string", "const": "rodecaster.record.state" },
                        "state": {
                            "type": "object",
                            "required": ["status", "mediaPresent"],
                            "properties": {
                                "status": { "type": "string", "enum": ["stopped", "paused", "recording"] },
                                "mediaPresent": { "type": "boolean", "description": "An SD card or USB drive is there to write to" }
                            }
                        }
                    }
                },
                "WsConnectedMessage": {
                    "type": "object",
                    "description": "Sent once immediately after a WebSocket connection is established.",
                    "required": ["type", "serverId"],
                    "properties": {
                        "type":     { "type": "string", "enum": ["connected"] },
                        "serverId": { "type": "string", "format": "uuid", "description": "Unique server ID — regenerated on every server restart" }
                    }
                },
                "WsConnectorStatusMessage": {
                    "type": "object",
                    "description": "Pushed on connect (initial snapshot) and whenever a connector's state changes.",
                    "required": ["type", "connector", "status"],
                    "properties": {
                        "type":      { "type": "string", "enum": ["connector.status"] },
                        "connector": { "type": "string", "enum": ["obs", "vmix", "middlecontrol"] },
                        "status":    { "$ref": "#/components/schemas/ConnectorStatus" }
                    }
                },
                "WsEventChangedMessage": {
                    "type": "object",
                    "description": "Broadcast when an event row is inserted, updated, or deleted.",
                    "required": ["type", "data"],
                    "properties": {
                        "type": { "type": "string", "enum": ["event.changed"] },
                        "data": {
                            "type": "object",
                            "required": ["operation", "record"],
                            "properties": {
                                "operation": { "type": "string", "enum": ["INSERT", "UPDATE", "DELETE"] },
                                "record":    { "$ref": "#/components/schemas/Event" }
                            }
                        }
                    }
                },
                "CropRect": {
                    "type": "object",
                    "description": "A rectangle of the developed picture, in pixels.",
                    "required": ["x", "y", "width", "height"],
                    "properties": {
                        "x": { "type": "integer", "minimum": 0 },
                        "y": { "type": "integer", "minimum": 0 },
                        "width": { "type": "integer", "minimum": 1 },
                        "height": { "type": "integer", "minimum": 1 }
                    }
                },
                "AudioInfo": {
                    "type": "object",
                    "required": ["codec", "channels", "sampleRate", "bits"],
                    "properties": {
                        "codec": { "type": "string", "example": "pcm_s24le" },
                        "channels": { "type": "integer", "example": 4 },
                        "sampleRate": { "type": "integer", "example": 48000 },
                        "bits": { "type": ["integer", "null"], "example": 24 }
                    }
                },
                "RenderSettings": {
                    "type": "object",
                    "description": "One render's settings: the `media` connector defaults, possibly edited for this file. `null` ISO, white balance, tint and exposure keep what the clip was shot with.",
                    "required": ["gamma", "gamut", "iso", "kelvin", "tint", "exposure", "highlightRecovery", "gamutCompression", "lutMode", "lutPath", "aspect", "cropShiftY", "bitrateMbps", "audioChannels"],
                    "properties": {
                        "gamma": { "type": "string", "example": "Blackmagic Design Film" },
                        "gamut": { "type": "string", "example": "Blackmagic Design" },
                        "iso": { "type": ["integer", "null"] },
                        "kelvin": { "type": ["integer", "null"] },
                        "tint": { "type": ["integer", "null"] },
                        "exposure": { "type": ["number", "null"], "description": "Stops" },
                        "highlightRecovery": { "type": "boolean" },
                        "gamutCompression": { "type": "boolean" },
                        "lutMode": { "type": "string", "enum": ["embedded", "file", "sidecar", "none"], "description": "`embedded`: the LUT the camera stored in the clip. `file`: `lutPath`, such as a Resolve Generate LUT export. `sidecar`: the LUT inside `sidecarPath` (or the clip's own .sidecar)." },
                        "lutPath": { "type": ["string", "null"] },
                        "aspect": { "type": "string", "enum": ["16:9", "native"] },
                        "cropShiftY": { "type": "integer", "description": "Pixels to move a 16:9 crop down (negative: up) from centre" },
                        "bitrateMbps": { "type": "integer", "minimum": 10, "maximum": 400, "example": 100 },
                        "audioChannels": { "type": "array", "items": { "type": "integer", "minimum": 1 }, "maxItems": 2, "example": [1, 2] },
                        "sidecarPath": { "type": ["string", "null"], "description": "A Blackmagic RAW .sidecar (such as Resolve's Update Sidecar) applied before the other settings" }
                    }
                },
                "ValueRange": {
                    "type": "object",
                    "required": ["min", "max"],
                    "properties": { "min": { "type": "number" }, "max": { "type": "number" } }
                },
                "RawOptions": {
                    "type": "object",
                    "description": "What the Blackmagic RAW SDK lets this clip's settings be, and what it was shot with.",
                    "required": ["gammas", "gamuts", "isos", "kelvin", "tint", "exposure", "asShot"],
                    "properties": {
                        "gammas": { "type": "array", "items": { "type": "string" }, "example": ["Blackmagic Design Film", "Blackmagic Design Video"] },
                        "gamuts": { "type": "array", "items": { "type": "string" }, "example": ["Blackmagic Design", "Rec.709"] },
                        "isos": { "type": "array", "items": { "type": "integer" }, "example": [100, 400, 1000] },
                        "kelvin": { "oneOf": [{ "$ref": "#/components/schemas/ValueRange" }, { "type": "null" }] },
                        "tint": { "oneOf": [{ "$ref": "#/components/schemas/ValueRange" }, { "type": "null" }] },
                        "exposure": { "oneOf": [{ "$ref": "#/components/schemas/ValueRange" }, { "type": "null" }] },
                        "asShot": {
                            "type": "object",
                            "required": ["iso", "kelvin", "tint", "exposure"],
                            "properties": {
                                "iso": { "type": ["integer", "null"] },
                                "kelvin": { "type": ["integer", "null"] },
                                "tint": { "type": ["integer", "null"] },
                                "exposure": { "type": ["number", "null"] }
                            }
                        }
                    }
                },
                "PreviewRequest": {
                    "type": "object",
                    "required": ["path", "frame"],
                    "properties": {
                        "path": { "type": "string", "description": "Absolute path of a .braw, .mov or .mp4 file on the server's disk" },
                        "frame": { "type": "integer", "minimum": 0, "description": "0-based; clamped to the last frame" },
                        "settings": { "oneOf": [{ "$ref": "#/components/schemas/RenderSettings" }, { "type": "null" }], "description": "The frame as recorded (its own look, uncropped) when absent" },
                        "maxWidth": { "type": ["integer", "null"], "minimum": 160, "maximum": 3840, "description": "The JPEG's width at most; 1600 when absent" }
                    }
                },
                "PreviewFrame": {
                    "type": "object",
                    "required": ["imageJpeg", "active", "output"],
                    "properties": {
                        "imageJpeg": { "type": "string", "format": "byte", "description": "Base64 JPEG of the whole developed picture" },
                        "active": { "$ref": "#/components/schemas/CropRect" },
                        "output": { "$ref": "#/components/schemas/CropRect", "description": "The part a render keeps, in the same pixels as `active`" }
                    }
                },
                "SubtitleSource": {
                    "type": "object",
                    "required": ["path", "language"],
                    "properties": {
                        "path": { "type": "string", "description": "Absolute path of an .srt or .vtt transcript, timed to the video or to the recording" },
                        "language": { "type": "string", "pattern": "^[a-z]{2,3}$", "example": "hu" }
                    }
                },
                "SubtitleReport": {
                    "type": "object",
                    "required": ["language", "cues", "shiftSeconds", "confident", "srtPath"],
                    "properties": {
                        "language": { "type": "string" },
                        "cues": { "type": "integer" },
                        "shiftSeconds": { "type": "number", "description": "How far the transcript was moved: to meet the voice, or with the recording when the voice did not say" },
                        "confident": { "type": "boolean", "description": "The voice backed the shift clearly. Otherwise a transcript running past the program moved with the recording (by the sync), and any other was left unmoved" },
                        "srtPath": { "type": "string" },
                        "onSpeech": { "type": "boolean", "description": "Rebuilt into whole sentences, each placed where Whisper heard it spoken (English transcripts on a Mac with the bundled whisper-cli). shiftSeconds is then the offset found for the whole file, and cues the sentence captions written" },
                        "matched": { "type": "integer", "minimum": 0, "description": "With onSpeech: captions with a word matched to the speech; the rest sit between them" }
                    }
                },
                "MediaProbe": {
                    "type": "object",
                    "description": "A video file on the server's disk, the render the stored settings would make of it, and the tools the server lacks to make it.",
                    "required": ["path", "fileName", "fileSize", "kind", "width", "height", "activeArea", "fpsNum", "fpsDen", "frameCount", "durationSeconds", "defaults", "output", "missing"],
                    "properties": {
                        "path": { "type": "string" },
                        "fileName": { "type": "string" },
                        "fileSize": { "type": "integer" },
                        "kind": { "type": "string", "enum": ["braw", "video"] },
                        "camera": { "type": ["string", "null"], "example": "Blackmagic Cinema Camera 6K" },
                        "clipName": { "type": ["string", "null"] },
                        "width": { "type": "integer", "description": "The developed picture's width", "example": 6048 },
                        "height": { "type": "integer", "example": 4032 },
                        "activeArea": { "$ref": "#/components/schemas/CropRect" },
                        "fpsNum": { "type": "integer", "example": 24 },
                        "fpsDen": { "type": "integer", "example": 1 },
                        "frameCount": { "type": "integer" },
                        "durationSeconds": { "type": "number" },
                        "startTimecode": { "type": ["string", "null"], "example": "10:26:20:15" },
                        "recordedAt": { "type": ["string", "null"], "format": "date-time", "description": "By the camera's clock, which may be wrong" },
                        "hostRecordStartedAt": { "type": ["string", "null"], "format": "date-time", "description": "When Metocast saw the camera start this clip, by the server's clock" },
                        "audio": { "oneOf": [{ "$ref": "#/components/schemas/AudioInfo" }, { "type": "null" }] },
                        "gamma": { "type": ["string", "null"] },
                        "gamut": { "type": ["string", "null"] },
                        "embeddedLut": { "type": ["string", "null"], "example": "Gen 5 Film to Extended Video" },
                        "defaults": { "$ref": "#/components/schemas/RenderSettings" },
                        "output": { "$ref": "#/components/schemas/CropRect" },
                        "missing": { "type": "array", "items": { "type": "string", "enum": ["ffmpeg", "braw-decode", "blackmagic-raw-sdk"] } },
                        "rawOptions": { "oneOf": [{ "$ref": "#/components/schemas/RawOptions" }, { "type": "null" }], "description": "Blackmagic RAW only" }
                    }
                },
                "RenderRequest": {
                    "type": "object",
                    "required": ["path"],
                    "properties": {
                        "path": { "type": "string", "description": "Absolute path of a .braw, .mov or .mp4 file on the server's disk" },
                        "eventId": { "type": ["string", "null"], "format": "uuid", "description": "The event the finished file is filed under; untracked when absent" },
                        "settings": { "oneOf": [{ "$ref": "#/components/schemas/RenderSettings" }, { "type": "null" }], "description": "The stored defaults when absent" },
                        "portrait": { "oneOf": [{ "$ref": "#/components/schemas/PortraitRequest" }, { "type": "null" }], "description": "Also makes `<name> Portrait.mov`, filed as its own recording with source `portrait`; landscape only when absent" }
                    }
                },
                "PortraitRequest": {
                    "type": "object",
                    "properties": {
                        "shiftX": { "type": "integer", "description": "Pixels to move the 9:16 frame right (negative: left) from centre; it stops at the picture's edge" }
                    }
                },
                "RenderQueued": {
                    "type": "object",
                    "required": ["jobId", "outputPath"],
                    "properties": {
                        "jobId": { "type": "string", "format": "uuid" },
                        "outputPath": { "type": "string" }
                    }
                },
                "RenderProgress": {
                    "type": "object",
                    "required": ["jobId", "state", "frame", "totalFrames", "fps", "outputPath", "error"],
                    "properties": {
                        "jobId": { "type": "string", "format": "uuid" },
                        "state": { "type": "string", "enum": ["running", "succeeded", "failed", "cancelled"] },
                        "frame": { "type": "integer" },
                        "totalFrames": { "type": "integer" },
                        "fps": { "type": "number", "description": "Frames encoded per second, averaged over the render so far" },
                        "outputPath": { "type": "string" },
                        "error": { "type": ["string", "null"] }
                    }
                },
                "AudioSettings": {
                    "type": "object",
                    "description": "One audio job's settings: the `media` connector defaults, possibly edited for these files.",
                    "required": ["channels", "targetLufs"],
                    "properties": {
                        "channels": { "type": "array", "items": { "type": "integer", "minimum": 1, "maximum": 64 }, "description": "1-based source channels averaged into the mono voice track; empty mixes them all", "example": [] },
                        "targetLufs": { "type": "number", "minimum": -30, "maximum": -9, "description": "Integrated loudness of the master. jive-vocals produces −16; Metocast's finishing pass adds the difference with a limiter.", "example": -14 }
                    }
                },
                "AudioSource": {
                    "type": "object",
                    "required": ["path", "fileName", "fileSize", "audio", "durationSeconds", "recordedAt"],
                    "properties": {
                        "path": { "type": "string" },
                        "fileName": { "type": "string" },
                        "fileSize": { "type": "integer" },
                        "audio": { "$ref": "#/components/schemas/AudioInfo" },
                        "durationSeconds": { "type": "number" },
                        "recordedAt": { "type": ["string", "null"], "format": "date-time", "description": "The container's creation time, else the file's: the start of its audio by the recorder's clock" }
                    }
                },
                "AudioProbe": {
                    "type": "object",
                    "description": "Audio files on the server's disk, joined in order, and the tools the server lacks to process them.",
                    "required": ["sources", "durationSeconds", "channels", "recordedAt", "defaults", "missing", "downloads"],
                    "properties": {
                        "sources": { "type": "array", "items": { "$ref": "#/components/schemas/AudioSource" } },
                        "durationSeconds": { "type": "number", "description": "All sources back to back" },
                        "channels": { "type": "integer", "description": "The fewest channels any source has" },
                        "recordedAt": { "type": ["string", "null"], "format": "date-time" },
                        "defaults": { "$ref": "#/components/schemas/AudioSettings" },
                        "missing": { "type": "array", "items": { "type": "string", "enum": ["ffmpeg", "jive-vocals"] }, "description": "Tools the server lacks and cannot fetch (jive-vocals only when a configured path is wrong)" },
                        "downloads": { "type": "array", "items": { "type": "string", "enum": ["jive-vocals"] }, "description": "Tools the first job downloads (pinned release, checked against its SHA-256)" }
                    }
                },
                "AudioRequest": {
                    "type": "object",
                    "required": ["paths"],
                    "properties": {
                        "paths": { "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": 32, "description": "Absolute paths, in playing order: one recording, or the files a recorder split it into" },
                        "eventId": { "type": ["string", "null"], "format": "uuid", "description": "The event the master is filed under; untracked when absent" },
                        "settings": { "oneOf": [{ "$ref": "#/components/schemas/AudioSettings" }, { "type": "null" }], "description": "The stored defaults when absent" }
                    }
                },
                "AudioReport": {
                    "type": "object",
                    "required": ["inputLufs", "outputLufs", "outputTruePeakDbtp", "removedDelaySamples", "alignmentConfidence"],
                    "properties": {
                        "inputLufs": { "type": "number", "example": -41.8 },
                        "outputLufs": { "type": "number", "example": -14.1 },
                        "outputTruePeakDbtp": { "type": "number", "example": -1.2 },
                        "removedDelaySamples": { "type": "integer", "description": "The delay jive-vocals' noise reduction added, in 48 kHz samples, which Metocast removed", "example": 1582 },
                        "alignmentConfidence": { "type": "number", "minimum": 0, "maximum": 1 }
                    }
                },
                "AudioProgress": {
                    "type": "object",
                    "required": ["jobId", "state", "step", "fraction", "outputPath", "error", "report"],
                    "properties": {
                        "jobId": { "type": "string", "format": "uuid" },
                        "state": { "type": "string", "enum": ["running", "succeeded", "failed", "cancelled"] },
                        "step": { "type": ["string", "null"], "enum": ["download", "prepare", "clean", "align", "finish", null] },
                        "fraction": { "type": ["number", "null"], "description": "0–1 through the step; null while a step can't tell (jive-vocals reports none)" },
                        "outputPath": { "type": "string" },
                        "error": { "type": ["string", "null"] },
                        "report": { "oneOf": [{ "$ref": "#/components/schemas/AudioReport" }, { "type": "null" }] }
                    }
                },
                "WsMediaAudioProgressMessage": {
                    "type": "object",
                    "required": ["type", "progress"],
                    "properties": {
                        "type": { "type": "string", "const": "media.audio.progress" },
                        "progress": { "$ref": "#/components/schemas/AudioProgress" }
                    }
                },
                "VideoSound": {
                    "type": "object",
                    "required": ["hasTrack", "usable", "levelDbfs"],
                    "properties": {
                        "hasTrack": { "type": "boolean" },
                        "usable": { "type": "boolean", "description": "Loud enough to sync by. The Cinema Camera's unconnected input reads −97 dBFS" },
                        "levelDbfs": { "type": ["number", "null"], "description": "The loudest of three measured stretches", "example": -97.0 }
                    }
                },
                "ClockEstimate": {
                    "type": "object",
                    "required": ["offsetSeconds", "learned"],
                    "properties": {
                        "offsetSeconds": { "type": "number", "description": "Where the clocks put the picture's start in the joined audio; negative when the audio starts later" },
                        "learned": { "type": "boolean", "description": "Corrected by the clock difference a recent confident sync of the same camera found. Raw clocks can be minutes apart" }
                    }
                },
                "ProgramProbeRequest": {
                    "type": "object",
                    "required": ["videoPath"],
                    "properties": {
                        "videoPath": { "type": "string", "description": "Absolute path of a .braw, .mov or .mp4 file on the server's disk" },
                        "audioPaths": { "type": "array", "items": { "type": "string" }, "maxItems": 32, "description": "The recording, or the files a recorder split it into, in playing order; empty to use the video's own sound" }
                    }
                },
                "ProgramProbe": {
                    "type": "object",
                    "description": "A video and its audio, and how the program job would put them together.",
                    "required": ["video", "audio", "videoSound", "syncMethod", "clock", "picture", "warnings", "blocker", "missing", "downloads"],
                    "properties": {
                        "video": { "$ref": "#/components/schemas/MediaProbe" },
                        "audio": { "oneOf": [{ "$ref": "#/components/schemas/AudioProbe" }, { "type": "null" }], "description": "Null when the video's own sound is used" },
                        "videoSound": { "$ref": "#/components/schemas/VideoSound" },
                        "syncMethod": { "type": "string", "enum": ["none", "waveform", "lips"], "description": "How the job will find the sync: nothing to find, the video's own sound, or the speaker's lips" },
                        "clock": { "oneOf": [{ "$ref": "#/components/schemas/ClockEstimate" }, { "type": "null" }] },
                        "picture": { "type": "string", "enum": ["grade", "copy"], "description": "What the picture goes through unless the request says otherwise" },
                        "warnings": { "type": "array", "items": { "type": "string" } },
                        "blocker": { "type": ["string", "null"], "description": "Why the job can't run, when it can't" },
                        "missing": { "type": "array", "items": { "type": "string", "enum": ["ffmpeg", "braw-decode", "blackmagic-raw-sdk", "jive-vocals", "lip-reader"] } },
                        "downloads": { "type": "array", "items": { "type": "string", "enum": ["jive-vocals"] } }
                    }
                },
                "ProgramRequest": {
                    "type": "object",
                    "required": ["videoPath"],
                    "properties": {
                        "videoPath": { "type": "string" },
                        "audioPaths": { "type": "array", "items": { "type": "string" }, "maxItems": 32 },
                        "eventId": { "type": ["string", "null"], "format": "uuid", "description": "The event the program is filed under; untracked when absent" },
                        "picture": { "type": ["string", "null"], "enum": ["grade", "copy", null], "description": "The probe's choice when absent. Blackmagic RAW must be graded" },
                        "settings": { "oneOf": [{ "$ref": "#/components/schemas/RenderSettings" }, { "type": "null" }], "description": "The grade; the stored defaults when absent" },
                        "audioSettings": { "oneOf": [{ "$ref": "#/components/schemas/AudioSettings" }, { "type": "null" }], "description": "The stored defaults when absent" },
                        "startSeconds": { "type": ["number", "null"], "description": "Where in the joined audio the video starts, when the operator knows; skips the search (sync method `manual`). Negative when the audio starts after the video", "example": 346.75 },
                        "subtitles": { "type": "array", "items": { "$ref": "#/components/schemas/SubtitleSource" }, "description": "Transcripts to move onto the program's voice, write beside it as .srt and send with its YouTube and Facebook uploads (when filed under an event); one per language" },
                        "portrait": { "oneOf": [{ "$ref": "#/components/schemas/PortraitRequest" }, { "type": "null" }], "description": "Also makes `<name> Portrait.mov` with the same sound, filed as its own recording with source `portrait`; landscape only when absent" }
                    }
                },
                "SyncWindow": {
                    "type": "object",
                    "required": ["videoSeconds", "offsetSeconds", "score", "agrees"],
                    "properties": {
                        "videoSeconds": { "type": "number", "description": "Where the stretch starts in the picture" },
                        "offsetSeconds": { "type": "number", "description": "Where this stretch alone put the picture's start in the audio" },
                        "score": { "type": "number", "description": "How well it matched there (−1–1)" },
                        "agrees": { "type": "boolean" }
                    }
                },
                "SyncReport": {
                    "type": "object",
                    "required": ["method", "offsetSeconds", "driftPpm", "confidence", "confident", "peakSigma", "windows", "clockDifferenceSeconds", "clockOffsetSeconds", "manualFrames", "startFile", "startFileSeconds"],
                    "properties": {
                        "method": { "type": "string", "enum": ["none", "waveform", "lips", "clock", "manual"] },
                        "offsetSeconds": { "type": "number", "description": "Where the picture's first frame falls in the joined audio; negative when the audio starts later", "example": 346.75 },
                        "driftPpm": { "type": "number", "description": "How much faster the audio's clock runs than the picture's; 0 unless clearly measured" },
                        "confidence": { "type": "number", "minimum": 0, "maximum": 1 },
                        "confident": { "type": "boolean", "description": "False asks the operator to check the sync" },
                        "peakSigma": { "type": ["number", "null"], "description": "How far the best match stands above every other position, in standard deviations", "example": 17.0 },
                        "windows": { "type": "array", "items": { "$ref": "#/components/schemas/SyncWindow" } },
                        "clockDifferenceSeconds": { "type": ["number", "null"], "description": "The picture's clock minus the audio's (time of day), as this sync found it", "example": 229.9 },
                        "clockOffsetSeconds": { "type": ["number", "null"], "description": "Where the raw clocks alone put the start" },
                        "manualFrames": { "type": "integer", "description": "Frames moved by hand after the job; positive plays the audio later" },
                        "startFile": { "type": ["string", "null"], "example": "00015_Wireless_PRO.WAV" },
                        "startFileSeconds": { "type": ["number", "null"] }
                    }
                },
                "ProgramReport": {
                    "type": "object",
                    "required": ["sync", "audio", "audioSeconds", "coverage", "silentStartSeconds", "silentEndSeconds", "audioOutputPath"],
                    "properties": {
                        "sync": { "$ref": "#/components/schemas/SyncReport" },
                        "audio": { "$ref": "#/components/schemas/AudioReport" },
                        "audioSeconds": { "type": "number", "description": "The joined audio's length" },
                        "coverage": { "type": "number", "minimum": 0, "maximum": 1, "description": "The share of the picture the audio covers" },
                        "silentStartSeconds": { "type": "number" },
                        "silentEndSeconds": { "type": "number" },
                        "damagedFrames": { "type": "integer", "minimum": 0, "description": "Frames at the end of the clip that couldn't be read (a recording that didn't end cleanly), replaced by the last good frame held so the picture keeps its length" },
                        "audioOutputPath": { "type": "string", "description": "The program's own WAV: the picture's length, aligned to its first frame" },
                        "subtitles": { "type": "array", "items": { "$ref": "#/components/schemas/SubtitleReport" } }
                    }
                },
                "ProgramProgress": {
                    "type": "object",
                    "required": ["jobId", "state", "step", "fraction", "outputPath", "error", "report"],
                    "properties": {
                        "jobId": { "type": "string", "format": "uuid", "description": "The program job, also while it's being nudged" },
                        "state": { "type": "string", "enum": ["running", "succeeded", "failed", "cancelled"] },
                        "step": { "type": ["string", "null"], "enum": ["download", "picture", "prepare", "sync", "clean", "merge", "portrait", "captions", null] },
                        "fraction": { "type": ["number", "null"] },
                        "outputPath": { "type": "string" },
                        "error": { "type": ["string", "null"] },
                        "report": { "oneOf": [{ "$ref": "#/components/schemas/ProgramReport" }, { "type": "null" }] }
                    }
                },
                "WsMediaProgramProgressMessage": {
                    "type": "object",
                    "required": ["type", "progress"],
                    "properties": {
                        "type": { "type": "string", "const": "media.program.progress" },
                        "progress": { "$ref": "#/components/schemas/ProgramProgress" }
                    }
                },
                "SubtitlesRequest": {
                    "type": "object",
                    "required": ["subtitles"],
                    "properties": {
                        "subtitles": { "type": "array", "minItems": 1, "items": { "$ref": "#/components/schemas/SubtitleSource" }, "description": "One transcript per language" }
                    }
                },
                "NudgeRequest": {
                    "type": "object",
                    "required": ["frames"],
                    "properties": {
                        "frames": { "type": "integer", "minimum": -240, "maximum": 240, "description": "Frames to move the audio; positive plays it later", "example": 1 }
                    }
                },
                "TranscriptionState": {
                    "type": "object",
                    "required": ["status", "reason", "sessionId", "eventId", "startedAt", "youtube"],
                    "properties": {
                        "status": { "type": "string", "enum": ["notConfigured", "unavailable", "ready", "connecting", "running", "error"] },
                        "reason": { "type": ["string", "null"], "description": "`mixerOffline` while unavailable; `noCredit`, `invalidKey` or a message on error" },
                        "sessionId": { "type": ["string", "null"], "format": "uuid" },
                        "eventId": { "type": ["string", "null"], "format": "uuid" },
                        "startedAt": { "type": ["string", "null"], "format": "date-time" },
                        "youtube": {
                            "type": ["object", "null"],
                            "description": "Live YouTube caption posting, while it runs",
                            "properties": {
                                "ok": { "type": "boolean" },
                                "lastSeq": { "type": "integer" },
                                "lastError": { "type": ["string", "null"] }
                            }
                        }
                    }
                },
                "TranscriptSegment": {
                    "type": "object",
                    "required": ["seq", "startedAt", "endedAt", "text"],
                    "properties": {
                        "seq": { "type": "integer" },
                        "startedAt": { "type": "string", "format": "date-time", "description": "When the English text began arriving (it trails the speech by the configured lag)" },
                        "endedAt": { "type": "string", "format": "date-time" },
                        "text": { "type": "string", "example": "Grace to you and peace." }
                    }
                },
                "CleanupStatus": { "type": "string", "enum": ["none", "running", "succeeded", "failed"] },
                "StoredTranscript": {
                    "type": "object",
                    "required": ["id", "eventId", "eventTitle", "startedAt", "endedAt", "status", "live", "durationSeconds", "segmentCount", "cleanupStatus", "cleanupError"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid" },
                        "eventId": { "type": ["string", "null"], "format": "uuid" },
                        "eventTitle": { "type": ["string", "null"] },
                        "startedAt": { "type": "string", "format": "date-time" },
                        "endedAt": { "type": ["string", "null"], "format": "date-time", "description": "Null while it runs, or when the server stopped without closing it" },
                        "status": { "type": "string", "enum": ["running", "stopped", "stoppedOffline", "error"] },
                        "live": { "type": "boolean", "description": "The session the live transcript is writing to now" },
                        "durationSeconds": { "type": "number" },
                        "segmentCount": { "type": "integer" },
                        "cleanupStatus": { "$ref": "#/components/schemas/CleanupStatus" },
                        "cleanupError": { "type": ["string", "null"] }
                    }
                },
                "StoredSegment": {
                    "type": "object",
                    "required": ["seq", "startedAt", "endedAt", "text", "cleanText"],
                    "properties": {
                        "seq": { "type": "integer" },
                        "startedAt": { "type": "string", "format": "date-time" },
                        "endedAt": { "type": "string", "format": "date-time" },
                        "text": { "type": "string", "example": "Grace to you and and peace." },
                        "cleanText": { "type": ["string", "null"], "description": "The clean-up's version; empty when it dropped the line as filler", "example": "Grace to you and peace." }
                    }
                },
                "TranscriptDetail": {
                    "type": "object",
                    "required": ["transcript", "segments"],
                    "properties": {
                        "transcript": { "$ref": "#/components/schemas/StoredTranscript" },
                        "segments": { "type": "array", "items": { "$ref": "#/components/schemas/StoredSegment" } }
                    }
                },
                "CleanupProgress": {
                    "type": "object",
                    "required": ["sessionId", "state", "progress", "error"],
                    "properties": {
                        "sessionId": { "type": "string", "format": "uuid" },
                        "state": { "$ref": "#/components/schemas/CleanupStatus" },
                        "progress": { "type": "number", "minimum": 0, "maximum": 1 },
                        "error": { "type": ["string", "null"] }
                    }
                },
                "CreditStatus": {
                    "type": "object",
                    "required": ["balanceUsd", "toppedUpUsd", "spentUsd", "asOf", "low"],
                    "properties": {
                        "balanceUsd": { "type": "number" },
                        "toppedUpUsd": { "type": "number" },
                        "spentUsd": { "type": "number" },
                        "asOf": { "type": "string", "format": "date-time" },
                        "low": { "type": "boolean", "description": "Balance below $5.00" }
                    }
                },
                "WsMediaRenderProgressMessage": {
                    "type": "object",
                    "required": ["type", "progress"],
                    "properties": {
                        "type": { "type": "string", "const": "media.render.progress" },
                        "progress": { "$ref": "#/components/schemas/RenderProgress" }
                    }
                },
                "WsRecordingChangedMessage": {
                    "type": "object",
                    "description": "Broadcast when a recording row is inserted or updated.",
                    "required": ["type", "data"],
                    "properties": {
                        "type": { "type": "string", "enum": ["recording.changed"] },
                        "data": {
                            "type": "object",
                            "required": ["operation", "record"],
                            "properties": {
                                "operation": { "type": "string", "enum": ["INSERT", "UPDATE", "DELETE"] },
                                "record":    { "$ref": "#/components/schemas/Recording" }
                            }
                        }
                    }
                }
            }
        },
        "paths": {
            "/api/logs": {
                "get": {
                    "tags": ["Settings"],
                    "summary": "Read the core's application log",
                    "description": "Returns the tail of the log file this core writes, plus its path on the host. Lets a client device read the server's log rather than its own.",
                    "operationId": "getApplicationLog",
                    "responses": {
                        "200": {
                            "description": "Log path and contents",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "required": ["path", "content"],
                                        "properties": {
                                            "path": { "type": "string" },
                                            "content": { "type": "string" }
                                        }
                                    }
                                }
                            }
                        },
                        "401": { "description": "Unauthorized — missing or invalid token" },
                        "503": { "description": "This core has no application log" }
                    }
                },
                "delete": {
                    "tags": ["Settings"],
                    "summary": "Clear the core's application log",
                    "operationId": "clearApplicationLog",
                    "responses": {
                        "204": { "description": "Log cleared" },
                        "401": { "description": "Unauthorized — missing or invalid token" },
                        "503": { "description": "This core has no application log" }
                    }
                }
            },
            "/api/settings/title-template": {
                "get": {
                    "tags": ["Settings"],
                    "summary": "Get the event title template",
                    "description": "The template the editor renders `computedTitle` from. Never 404s — an unset key returns the built-in default.",
                    "operationId": "getTitleTemplate",
                    "responses": {
                        "200": {
                            "description": "The stored template, or the default when unset",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/TitleTemplate" }
                                }
                            }
                        },
                        "401": { "description": "Unauthorized — missing or invalid token" }
                    }
                },
                "put": {
                    "tags": ["Settings"],
                    "summary": "Replace the event title template",
                    "operationId": "setTitleTemplate",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/TitleTemplate" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "The stored template",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/TitleTemplate" }
                                }
                            }
                        },
                        "401": { "description": "Unauthorized — missing or invalid token" },
                        "500": { "description": "Database error" }
                    }
                }
            },
            "/api/settings/record-button": {
                "get": {
                    "tags": ["Settings"],
                    "summary": "Get the Record button's recorders",
                    "operationId": "getRecordButton",
                    "responses": {
                        "200": { "description": "The recorders, or the default when unset", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/RecordButton" } } } },
                        "401": { "description": "Unauthorized — missing or invalid token" }
                    }
                },
                "put": {
                    "tags": ["Settings"],
                    "summary": "Choose the Record button's recorders",
                    "description": "Every client hears the change in `recorders.state`.",
                    "operationId": "setRecordButton",
                    "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/RecordButton" } } } },
                    "responses": {
                        "200": { "description": "The stored recorders", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/RecordButton" } } } },
                        "401": { "description": "Unauthorized — missing or invalid token" },
                        "500": { "description": "Database error" }
                    }
                }
            },
            "/api/recorders": {
                "get": {
                    "tags": ["Settings"],
                    "summary": "Every recorder the Record button can drive, with its state",
                    "description": "The same rows `recorders.state` pushes. Start or stop the selected ones with the `recorders.start` and `recorders.stop` WebSocket commands.",
                    "operationId": "listRecorders",
                    "responses": {
                        "200": { "description": "One row per recorder", "content": { "application/json": { "schema": { "type": "array", "items": { "$ref": "#/components/schemas/RecorderRow" } } } } },
                        "401": { "description": "Unauthorized — missing or invalid token" }
                    }
                }
            },
            "/api/settings/slide-folder": {
                "get": {
                    "tags": ["Settings"],
                    "summary": "Get the Bible slide output folder",
                    "description": "Never 404s — an unset key returns an empty path.",
                    "operationId": "getSlideFolder",
                    "responses": {
                        "200": {
                            "description": "The stored folder, or an empty path when unset",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/SlideFolder" }
                                }
                            }
                        },
                        "401": { "description": "Unauthorized — missing or invalid token" }
                    }
                },
                "put": {
                    "tags": ["Settings"],
                    "summary": "Set the Bible slide output folder",
                    "description": "The core checks the folder exists on its own machine; an empty path clears the setting.",
                    "operationId": "setSlideFolder",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/SlideFolder" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "The stored folder",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/SlideFolder" }
                                }
                            }
                        },
                        "400": { "description": "The folder does not exist on the core's machine" },
                        "401": { "description": "Unauthorized — missing or invalid token" },
                        "500": { "description": "Database error" }
                    }
                }
            },
            "/api/settings/song-slide-folder": {
                "get": {
                    "tags": ["Settings"],
                    "summary": "Get the song slide output folder",
                    "description": "Never 404s — an unset key returns an empty path.",
                    "operationId": "getSongSlideFolder",
                    "responses": {
                        "200": {
                            "description": "The stored folder, or an empty path when unset",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/SlideFolder" }
                                }
                            }
                        },
                        "401": { "description": "Unauthorized — missing or invalid token" }
                    }
                },
                "put": {
                    "tags": ["Settings"],
                    "summary": "Set the song slide output folder",
                    "description": "The core checks the folder exists on its own machine; an empty path clears the setting.",
                    "operationId": "setSongSlideFolder",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/SlideFolder" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "The stored folder",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/SlideFolder" }
                                }
                            }
                        },
                        "400": { "description": "The folder does not exist on the core's machine" },
                        "401": { "description": "Unauthorized — missing or invalid token" },
                        "500": { "description": "Database error" }
                    }
                }
            },
            "/api/ppt/song": {
                "post": {
                    "tags": ["Presentations"],
                    "summary": "Create a song PowerPoint",
                    "description": "Starts a new slide at each empty line and at each line starting with Refr., breaks a line too long for the song size at punctuation or a space, adds a title slide and a blank last slide, and writes the deck to the song slide folder.",
                    "operationId": "createSongPresentation",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": {
                                    "type": "object",
                                    "required": ["title", "lyrics"],
                                    "properties": {
                                        "title": { "type": "string", "maxLength": 200 },
                                        "lyrics": { "type": "string", "maxLength": 50000 }
                                    }
                                }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "The written file and slide count",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "required": ["filePath", "slideCount"],
                                        "properties": {
                                            "filePath": { "type": "string" },
                                            "slideCount": { "type": "integer", "minimum": 1 }
                                        }
                                    }
                                }
                            }
                        },
                        "400": { "description": "Missing content or no song slide output folder configured" },
                        "401": { "description": "Unauthorized — missing or invalid token" },
                        "500": { "description": "Could not write the deck" }
                    }
                }
            },
            "/api/events/{id}/order": {
                "get": {
                    "tags": ["Events"],
                    "summary": "Get an event's Order of Service",
                    "description": "The service's steps in order: songs (a deck), Bible readings (the event's Textus or Lekció), announcements (their notes shown as bullet slides), text slides (such as the welcome) and blocks (no slides). Step through them live with the `order.go`, `order.next` and `order.previous` WebSocket commands; `order.state` says which step is on screen.",
                    "operationId": "getEventOrder",
                    "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }],
                    "responses": {
                        "200": { "description": "The steps", "content": { "application/json": { "schema": { "type": "object", "required": ["items"], "properties": { "items": { "type": "array", "maxItems": 200, "items": {
                "type": "object",
                "required": ["kind"],
                "properties": {
                    "id": { "type": "string", "format": "uuid", "description": "Omit for a new step; keep it so the live step survives the edit" },
                    "kind": { "type": "string", "enum": ["song", "bible", "block", "announcements", "text"], "description": "`announcements` shows its notes as bullet slides; `text` is one slide of its title and its notes as paragraphs" },
                    "title": { "type": "string", "maxLength": 200, "description": "Required for a block, the announcements and a text slide, and for a song with no deck" },
                    "deckPath": { "type": "string", "description": "Song: a deck on the server's disk. Optional: a song with none leaves the screen as it is" },
                    "bibleType": { "type": "string", "enum": ["textus", "leckio"], "description": "Bible: which of the event's readings. Leave it out for a reading of the step's own" },
                    "reference": { "type": "string", "maxLength": 200, "description": "Bible without `bibleType`: the passage, e.g. Zsolt 23. Its verses are looked up when the list is saved (a reference that finds none is refused) and stored, so showing it needs no network" },
                    "translation": { "type": "string", "description": "The translation of `reference`, e.g. RUF_v2 (the default) or KG" },
                    "notes": { "type": "string", "maxLength": 2000, "description": "For the people leading. Announcements: the bullet points, one per line. Text: optional paragraphs, one per line" }
                }
            } } } } } } },
                        "401": { "description": "Unauthorized — missing or invalid token" },
                        "404": { "description": "No such event" }
                    }
                },
                "put": {
                    "tags": ["Events"],
                    "summary": "Replace an event's Order of Service",
                    "description": "Replaces the whole list and broadcasts `event.order.changed`. A step sent with its `id` keeps it, so the step on screen stays current; `order.state` is re-sent when this event is on screen, and reports no step if that step was removed.",
                    "operationId": "setEventOrder",
                    "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }],
                    "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object", "required": ["items"], "properties": { "items": { "type": "array", "maxItems": 200, "items": {
                "type": "object",
                "required": ["kind"],
                "properties": {
                    "id": { "type": "string", "format": "uuid", "description": "Omit for a new step; keep it so the live step survives the edit" },
                    "kind": { "type": "string", "enum": ["song", "bible", "block", "announcements", "text"], "description": "`announcements` shows its notes as bullet slides; `text` is one slide of its title and its notes as paragraphs" },
                    "title": { "type": "string", "maxLength": 200, "description": "Required for a block" },
                    "deckPath": { "type": "string", "description": "Song: a deck on the server's disk" },
                    "bibleType": { "type": "string", "enum": ["textus", "leckio"], "description": "Bible: which of the event's readings. Leave it out for a reading of the step's own" },
                    "reference": { "type": "string", "maxLength": 200, "description": "Bible without `bibleType`: the passage, e.g. Zsolt 23. Its verses are looked up when the list is saved (a reference that finds none is refused) and stored, so showing it needs no network" },
                    "translation": { "type": "string", "description": "The translation of `reference`, e.g. RUF_v2 (the default) or KG" },
                    "notes": { "type": "string", "maxLength": 2000, "description": "For the people leading. Announcements: the bullet points, one per line. Text: optional paragraphs, one per line" }
                }
            } } } } } } },
                    "responses": {
                        "200": { "description": "The stored steps, with ids", "content": { "application/json": { "schema": { "type": "object", "required": ["items"], "properties": { "items": { "type": "array", "maxItems": 200, "items": {
                "type": "object",
                "required": ["kind"],
                "properties": {
                    "id": { "type": "string", "format": "uuid", "description": "Omit for a new step; keep it so the live step survives the edit" },
                    "kind": { "type": "string", "enum": ["song", "bible", "block", "announcements", "text"], "description": "`announcements` shows its notes as bullet slides; `text` is one slide of its title and its notes as paragraphs" },
                    "title": { "type": "string", "maxLength": 200, "description": "Required for a block" },
                    "deckPath": { "type": "string", "description": "Song: a deck on the server's disk" },
                    "bibleType": { "type": "string", "enum": ["textus", "leckio"], "description": "Bible: which of the event's readings. Leave it out for a reading of the step's own" },
                    "reference": { "type": "string", "maxLength": 200, "description": "Bible without `bibleType`: the passage, e.g. Zsolt 23. Its verses are looked up when the list is saved (a reference that finds none is refused) and stored, so showing it needs no network" },
                    "translation": { "type": "string", "description": "The translation of `reference`, e.g. RUF_v2 (the default) or KG" },
                    "notes": { "type": "string", "maxLength": 2000, "description": "For the people leading. Announcements: the bullet points, one per line. Text: optional paragraphs, one per line" }
                }
            } } } } } } },
                        "400": { "description": "A step is missing what it needs (a song's title or deck, a reading's type, a block's title), or an id repeats" },
                        "401": { "description": "Unauthorized — missing or invalid token" },
                        "404": { "description": "No such event" }
                    }
                }
            },
            "/api/events/{id}/slides": {
                "post": {
                    "tags": ["Events"],
                    "summary": "Generate Bible slide decks for an event",
                    "description": "Writes one `.pptx` per Bible reference into the configured slide folder, paginated the same way the web presenter shows them. Names are fixed — `textus.pptx` and `lekcio.pptx` — so regenerating replaces the previous deck.",
                    "operationId": "createEventSlides",
                    "parameters": [
                        {
                            "name": "id",
                            "in": "path",
                            "required": true,
                            "schema": { "type": "string", "format": "uuid" }
                        }
                    ],
                    "responses": {
                        "200": {
                            "description": "Paths of the written files",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "required": ["files"],
                                        "properties": {
                                            "files": { "type": "array", "items": { "type": "string" } }
                                        }
                                    }
                                }
                            }
                        },
                        "400": { "description": "No slide folder configured, or the event has no Bible references with verses" },
                        "401": { "description": "Unauthorized — missing or invalid token" },
                        "404": { "description": "Event not found" },
                        "500": { "description": "Could not write the deck" }
                    }
                }
            },
            "/api/events": {
                "get": {
                    "tags": ["Events"],
                    "summary": "List events",
                    "description": "Returns all events ordered by date (newest first). Each item includes a recording count but omits large text fields — use *Get event* to fetch the full record.",
                    "operationId": "listEvents",
                    "responses": {
                        "200": {
                            "description": "Array of event summaries",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "array",
                                        "items": { "$ref": "#/components/schemas/EventSummary" }
                                    }
                                }
                            }
                        },
                        "401": { "description": "Unauthorized — missing or invalid token" },
                        "500": { "description": "Database error" }
                    }
                },
                "post": {
                    "tags": ["Events"],
                    "summary": "Create event",
                    "description": "Creates the event with the default Order of Service: the welcome slide (a text step named after the welcome screen's greeting), Song 1, Song 2, Lekció, Song 3, Textus, Preaching, Song 4, Hirdetések (the announcements), Áldás, Song 4 last part. The songs have no deck until one is picked; read and change the steps with `/api/events/{id}/order`.",
                    "operationId": "createEvent",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/CreateEventRequest" },
                                "example": {
                                    "title": "Sunday Service",
                                    "date_time": "2025-01-19T10:00:00Z",
                                    "speaker": "Pastor Smith",
                                    "bible_references": [
                                        { "type": "textus", "reference": "John 3:16", "translation": "UF" }
                                    ]
                                }
                            }
                        }
                    },
                    "responses": {
                        "201": {
                            "description": "Event created — returns the full event record",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/Event" }
                                }
                            }
                        },
                        "401": { "description": "Unauthorized" },
                        "500": { "description": "Database error" }
                    }
                }
            },
            "/api/events/{id}": {
                "parameters": [
                    {
                        "name": "id",
                        "in": "path",
                        "required": true,
                        "description": "Event UUID",
                        "schema": { "type": "string", "format": "uuid" }
                    }
                ],
                "get": {
                    "tags": ["Events"],
                    "summary": "Get event",
                    "description": "Returns the complete event record including all text fields.",
                    "operationId": "getEvent",
                    "responses": {
                        "200": {
                            "description": "Full event record",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/Event" }
                                }
                            }
                        },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "Event not found" },
                        "500": { "description": "Database error" }
                    }
                },
                "put": {
                    "tags": ["Events"],
                    "summary": "Update event",
                    "description": "Replaces all fields of an existing event. This is a full replacement — omitted optional fields revert to their defaults (empty string, `\"UF\"`, `\"private\"`, `false`). A Lekció or Textus that is set or changed gets a step in the Order of Service if it has none (the Lekció before the Textus); `event.order.changed` is broadcast when that happens.",
                    "operationId": "updateEvent",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/CreateEventRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Updated event record",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/Event" }
                                }
                            }
                        },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "Event not found" },
                        "500": { "description": "Database error" }
                    }
                }
            },
            "/api/events/{id}/recordings": {
                "parameters": [
                    {
                        "name": "id",
                        "in": "path",
                        "required": true,
                        "description": "Event UUID",
                        "schema": { "type": "string", "format": "uuid" }
                    }
                ],
                "get": {
                    "tags": ["Recordings"],
                    "summary": "List recordings",
                    "description": "Returns all recording files attached to an event, ordered by detection time (newest first).",
                    "operationId": "listRecordings",
                    "responses": {
                        "200": {
                            "description": "Array of recordings",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "array",
                                        "items": { "$ref": "#/components/schemas/Recording" }
                                    }
                                }
                            }
                        },
                        "401": { "description": "Unauthorized" },
                        "500": { "description": "Database error" }
                    }
                },
                "post": {
                    "tags": ["Recordings"],
                    "summary": "Add recording",
                    "description": "Registers a new recording file for an event. The file must already exist on the server's filesystem.",
                    "operationId": "createRecording",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/CreateRecordingRequest" },
                                "example": {
                                    "file_path": "/recordings/service-2025-01-19.mp4",
                                    "file_name": "service-2025-01-19.mp4",
                                    "file_size": 1073741824,
                                    "duration_seconds": 3600.0
                                }
                            }
                        }
                    },
                    "responses": {
                        "201": {
                            "description": "Recording registered — returns the full recording record",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/Recording" }
                                }
                            }
                        },
                        "401": { "description": "Unauthorized" },
                        "500": { "description": "Database error" }
                    }
                }
            },
            "/api/presenter/welcome": {
                "get": {
                    "tags": ["Presenter"],
                    "summary": "Read the welcome screen settings",
                    "description": "The welcome screen is the slide a **text** Order of Service step shows in the web presenter: a photo, the logo top right, the step's title (the greeting), its notes or else `subtitle`, and the church name with the event's date. Each month has its photo: the event's month picks it (photo 1 is January). White text over a dark overlay sized from each photo's brightness keeps at least 4.5:1 contrast (WCAG AAA for large text).",
                    "operationId": "getWelcomeSettings",
                    "responses": {
                        "200": { "description": "The settings", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/WelcomeSettings" } } } },
                        "401": { "description": "Unauthorized" }
                    }
                },
                "put": {
                    "tags": ["Presenter"],
                    "summary": "Set the welcome screen settings",
                    "description": "`hasLogo` and `replacedPhotos` are ignored; they come from the uploads.",
                    "operationId": "setWelcomeSettings",
                    "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/WelcomeSettings" } } } },
                    "responses": {
                        "200": { "description": "The stored settings", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/WelcomeSettings" } } } },
                        "400": { "description": "Text too long, or a language other than hu or en" },
                        "401": { "description": "Unauthorized" }
                    }
                }
            },
            "/api/presenter/welcome/photos/{number}": {
                "parameters": [{ "name": "number", "in": "path", "required": true, "schema": { "type": "integer", "minimum": 1, "maximum": 12 } }],
                "get": {
                    "tags": ["Presenter"],
                    "summary": "A month's welcome photo",
                    "description": "The replacement if one was uploaded, else the default, which the server downloads from the Metocast repository on first use (Unsplash, see `crates/metocast-server/assets/welcome/CREDITS.md`).",
                    "operationId": "getWelcomePhoto",
                    "responses": {
                        "200": { "description": "The photo", "content": { "image/jpeg": { "schema": { "type": "string", "format": "binary" } } } },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "Not 1–12" },
                        "503": { "description": "The default photo isn't downloaded yet and can't be now (no internet)" }
                    }
                },
                "put": {
                    "tags": ["Presenter"],
                    "summary": "Replace a welcome photo",
                    "description": "The body is the image file (JPEG, PNG, WebP…, up to 40 MB). It is cropped to 16:9 and stored at 1920×1080.",
                    "operationId": "setWelcomePhoto",
                    "requestBody": { "required": true, "content": { "application/octet-stream": { "schema": { "type": "string", "format": "binary" } } } },
                    "responses": {
                        "204": { "description": "Stored" },
                        "400": { "description": "Not an image" },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "Not 1–12" }
                    }
                },
                "delete": {
                    "tags": ["Presenter"],
                    "summary": "Reset a welcome photo to its default",
                    "operationId": "resetWelcomePhoto",
                    "responses": {
                        "204": { "description": "Reset" },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "Not 1–12" }
                    }
                }
            },
            "/api/presenter/welcome/preview/{number}": {
                "parameters": [{ "name": "number", "in": "path", "required": true, "schema": { "type": "integer", "minimum": 1, "maximum": 12 } }],
                "get": {
                    "tags": ["Presenter"],
                    "summary": "Preview a month's welcome slide",
                    "description": "The welcome slide with month `number`'s photo, the logo and the saved text, dated the first Sunday of that month, as the screens would show it.",
                    "operationId": "previewWelcomeSlide",
                    "responses": {
                        "200": { "description": "The slide", "content": { "image/svg+xml": { "schema": { "type": "string" } } } },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "Not 1–12" }
                    }
                }
            },
            "/api/presenter/welcome/logo": {
                "get": {
                    "tags": ["Presenter"],
                    "summary": "The welcome screen's logo (the built-in one until one is uploaded)",
                    "operationId": "getWelcomeLogo",
                    "responses": {
                        "200": { "description": "The logo", "content": { "image/png": { "schema": { "type": "string", "format": "binary" } }, "image/svg+xml": { "schema": { "type": "string" } } } },
                        "401": { "description": "Unauthorized" }
                    }
                },
                "put": {
                    "tags": ["Presenter"],
                    "summary": "Set the welcome screen's logo",
                    "description": "The body is a PNG (transparency kept) or an SVG file, up to 5 MB. It shows top right on the welcome screen.",
                    "operationId": "setWelcomeLogo",
                    "requestBody": { "required": true, "content": { "application/octet-stream": { "schema": { "type": "string", "format": "binary" } } } },
                    "responses": {
                        "204": { "description": "Stored" },
                        "400": { "description": "Not a PNG or SVG" },
                        "401": { "description": "Unauthorized" }
                    }
                },
                "delete": {
                    "tags": ["Presenter"],
                    "summary": "Remove the welcome screen's logo",
                    "operationId": "removeWelcomeLogo",
                    "responses": { "204": { "description": "Removed" }, "401": { "description": "Unauthorized" } }
                }
            },
            "/api/presenter/parse": {
                "post": {
                    "tags": ["Presenter"],
                    "summary": "Parse a .pptx file",
                    "description": "Opens a `.pptx` file from the local filesystem, extracts text content from every slide, and returns the structured data. Only Open XML (`.pptx`) format is supported; legacy binary `.ppt` files must be re-saved as `.pptx` first.",
                    "operationId": "parsePresentation",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": {
                                    "type": "object",
                                    "required": ["filePath"],
                                    "properties": {
                                        "filePath": { "type": "string", "example": "/Users/admin/Presentations/sunday-service.pptx" }
                                    }
                                }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Parsed presentation data",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "success": { "type": "boolean", "example": true },
                                            "data": { "$ref": "#/components/schemas/ParsedPresentation" }
                                        }
                                    }
                                }
                            }
                        },
                        "401": { "description": "Unauthorized" },
                        "422": { "description": "File not found, not a valid .pptx, or no slides found" }
                    }
                }
            },
            "/api/bible/verses": {
                "get": {
                    "tags": ["Bible"],
                    "summary": "Look up a Bible passage",
                    "description": "Fetches a passage from the upstream Bible API that matches the translation (`*_v2` codes use the V2 API, everything else szentiras.eu) and returns it in one normalised shape. The core performs the upstream request, so UIs need no CORS workaround. szentiras.eu lookups send the API key from the `szentiras` connector config; without a valid key they fail with 502.",
                    "operationId": "getBiblePassage",
                    "parameters": [
                        {
                            "name": "reference",
                            "in": "query",
                            "required": true,
                            "description": "Passage reference, e.g. `Jn 3,16`",
                            "schema": { "type": "string" }
                        },
                        {
                            "name": "translation",
                            "in": "query",
                            "required": true,
                            "description": "Translation code",
                            "schema": { "type": "string", "enum": ["UF_v2", "RUF_v2", "RUF", "KG", "KNB", "SZIT", "BD", "STL"] }
                        }
                    ],
                    "responses": {
                        "200": {
                            "description": "The passage",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/BiblePassage" }
                                }
                            }
                        },
                        "400": { "description": "Missing reference or translation" },
                        "401": { "description": "Unauthorized" },
                        "502": { "description": "The upstream Bible API failed or returned something unparseable" }
                    }
                }
            },
            "/api/bible/suggest": {
                "get": {
                    "tags": ["Bible"],
                    "summary": "Autocomplete Bible references",
                    "description": "Reference suggestions from szentiras.eu. This endpoint is public upstream, so it works without an API key. Terms shorter than 2 characters return an empty list without calling upstream.",
                    "operationId": "getBibleSuggestions",
                    "parameters": [
                        {
                            "name": "term",
                            "in": "query",
                            "required": true,
                            "description": "Partial reference the user has typed",
                            "schema": { "type": "string" }
                        }
                    ],
                    "responses": {
                        "200": {
                            "description": "Matching references",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "array",
                                        "items": { "$ref": "#/components/schemas/BibleSuggestion" }
                                    }
                                }
                            }
                        },
                        "400": { "description": "Missing term" },
                        "401": { "description": "Unauthorized" },
                        "502": { "description": "The upstream Bible API failed or returned something unparseable" }
                    }
                }
            },
            "/api/connectors/status": {
                "get": {
                    "tags": ["Connectors"],
                    "summary": "Get connector statuses",
                    "description": "Returns the current connection state of every connector. Each value is a discriminated union on `type` — see the `ConnectorStatus` schema.",
                    "operationId": "getConnectorStatuses",
                    "responses": {
                        "200": {
                            "description": "Current connector statuses",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/ConnectorStatuses" },
                                    "example": {
                                        "obs":  { "type": "connected" },
                                        "vmix": { "type": "disconnected" }
                                    }
                                }
                            }
                        },
                        "401": { "description": "Unauthorized" }
                    }
                }
            },
            "/api/connectors/{name}/config": {
                "parameters": [
                    {
                        "name": "name",
                        "in": "path",
                        "required": true,
                        "description": "Connector name",
                        "schema": { "type": "string", "enum": ["obs", "blackmagic-camera", "middlecontrol", "rodecaster", "vmix", "atem", "broadlink", "youtube", "facebook", "discord", "szentiras", "media", "mcp"] }
                    }
                ],
                "get": {
                    "tags": ["Connectors"],
                    "summary": "Get connector configuration",
                    "description": "Returns the stored configuration for one connector, or its defaults when nothing has been saved yet.\n\n**Secrets are never returned here.** `password`, `clientSecret`, `appSecret`, `apiKey` and `webhookUrl` always come back empty, with a companion boolean (`apiKeySet`, `passwordSet`, …) telling you whether one is stored. The host running the server can read them back through `/api/connectors/{name}/config/secrets`.",
                    "operationId": "getConnectorConfig",
                    "responses": {
                        "200": {
                            "description": "Stored configuration",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/ConnectorConfig" },
                                    "example": { "enabled": true, "host": "localhost", "port": 4455, "password": null }
                                }
                            }
                        },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "Unknown connector" }
                    }
                },
                "put": {
                    "tags": ["Connectors"],
                    "summary": "Save connector configuration",
                    "description": "Persists the configuration and applies it: OBS, Blackmagic Camera, and Middle Control reconnect (or disconnect when `enabled` is false); YouTube and Facebook refresh the config used by the OAuth routes and stop when disabled.\n\n**Secret handling:** send a non-empty secret to replace it, leave it empty or omit it to keep the stored one, or send `\"<field>Set\": false` to clear it. This lets a client save a config whose secrets it was never allowed to read.",
                    "operationId": "putConnectorConfig",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/ConnectorConfig" }
                            }
                        }
                    },
                    "responses": {
                        "204": { "description": "Saved" },
                        "400": { "description": "Body does not match the connector's config shape" },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "Unknown connector" }
                    }
                }
            },
            "/api/connectors/{name}/config/secrets": {
                "get": {
                    "tags": ["Connectors"],
                    "summary": "Read a connector's stored secrets (host only)",
                    "description": "Returns the connector config **including** its secrets.\n\nRestricted to the desktop app hosting this server: it requires the normal auth token, an `X-Admin-Token` header matching the running server's admin token (regenerated every run, delivered to the host window over Tauri IPC, never over the network), and a request originating from loopback. Remote clients cannot obtain the admin token, and a leaked one is unusable off-host.\n\nConnectors that store no credentials (`vmix`, `atem`, `middlecontrol`, `broadlink`, `rodecaster`) return 204.",
                    "operationId": "revealConnectorSecrets",
                    "parameters": [
                        {
                            "name": "name",
                            "in": "path",
                            "required": true,
                            "schema": { "type": "string", "enum": ["obs", "blackmagic-camera", "youtube", "facebook", "discord", "szentiras"] }
                        },
                        {
                            "name": "X-Admin-Token",
                            "in": "header",
                            "required": true,
                            "description": "The running server's admin token",
                            "schema": { "type": "string" }
                        }
                    ],
                    "responses": {
                        "200": {
                            "description": "The config with secrets in the clear",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/ConnectorConfig" }
                                }
                            }
                        },
                        "204": { "description": "This connector stores no secrets" },
                        "401": { "description": "Unauthorized" },
                        "403": { "description": "Missing/invalid admin token, or the request did not come from loopback" },
                        "404": { "description": "Unknown connector" }
                    }
                }
            },
            "/api/connectors/blackmagic-camera/stream/youtube": {
                "post": {
                    "tags": ["Connectors"],
                    "summary": "Push YouTube settings to the camera",
                    "description": "Copies the channel's RTMP ingestion address and stream key into the camera's livestream settings, preferring the camera's own YouTube platform entry (which needs only the key) and falling back to a custom RTMP URL on models that list no YouTube platform.\n\nSets the destination only — the camera does not go live. Requires a connected camera (409 otherwise) and a linked YouTube account.",
                    "operationId": "pushBlackmagicCameraYouTubeSettings",
                    "responses": {
                        "200": {
                            "description": "Where the camera's livestream now points",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/CameraStreamTarget" }
                                }
                            }
                        },
                        "409": { "description": "No camera connected" },
                        "502": { "description": "The camera or the YouTube API rejected the request" }
                    }
                }
            },
            "/api/connectors/atem/discover": {
                "post": {
                    "tags": ["Connectors"],
                    "summary": "Find ATEM switchers",
                    "description": "Browses mDNS for `_switcher_ctrl._udp` for three seconds. Switchers plugged into the server by USB are found through the USB Ethernet link they create; switchers on the LAN are found on the network. The scan does not save or connect anything; put the returned host and port in the `atem` config. Networks that block multicast need manual host entry.",
                    "operationId": "discoverAtem",
                    "responses": {
                        "200": {
                            "description": "Switchers found, possibly none",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "required": ["devices"],
                                        "properties": {
                                            "devices": {
                                                "type": "array",
                                                "items": { "$ref": "#/components/schemas/DiscoveredAtem" }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        "502": { "description": "mDNS could not be started on this machine" }
                    }
                }
            },
            "/api/connectors/atem/stream/youtube": {
                "post": {
                    "tags": ["Connectors"],
                    "summary": "Push YouTube settings to the ATEM switcher",
                    "description": "Writes the channel's RTMP ingestion address and stream key into the switcher's streaming service.\n\nSets the destination only — the switcher does not go live. Requires a connected switcher with a streaming engine (409 when none is connected) and a linked YouTube account.",
                    "operationId": "pushAtemYouTubeSettings",
                    "responses": {
                        "200": {
                            "description": "Where the switcher's stream now points",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/AtemStreamTarget" }
                                }
                            }
                        },
                        "409": { "description": "No switcher connected" },
                        "502": { "description": "The switcher or the YouTube API rejected the request" }
                    }
                }
            },
            "/api/connectors/blackmagic-camera/settings": {
                "get": {
                    "tags": ["Connectors"],
                    "summary": "Read the camera's storage, record format and livestream settings",
                    "description": "One pass over the camera's own control API: record format plus every supported format, the media slots and working set, and the livestream status, platform list, active platform and that platform's servers and quality profiles.\n\nPayloads are the camera's, forwarded unchanged. Only the active platform's profile list is fetched — the others cost a round trip each.",
                    "operationId": "getBlackmagicCameraSettings",
                    "responses": {
                        "200": {
                            "description": "Camera settings",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/CameraSettings" }
                                }
                            }
                        },
                        "409": { "description": "No camera connected" },
                        "502": { "description": "The camera rejected the request" }
                    }
                },
                "put": {
                    "tags": ["Connectors"],
                    "summary": "Write the camera's record format, livestream platform, or both",
                    "description": "Both halves are optional; whichever is present is written. `record` is the camera's own `FormatRequest` — codec, frameRate and both resolutions travel together, as the camera validates the combination as a whole. `stream` is the full active-platform entry, so send back the one from GET with the fields you changed. GET never returns `key` or `passphrase`; left out, the camera keeps its own while the platform is unchanged.\n\nAnswers with the settings read back from the camera.",
                    "operationId": "putBlackmagicCameraSettings",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/CameraSettingsUpdate" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Settings after the write",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/CameraSettings" }
                                }
                            }
                        },
                        "409": { "description": "No camera connected" },
                        "502": { "description": "The camera rejected the format or platform" }
                    }
                }
            },
            "/api/connectors/blackmagic-camera/discover": {
                "post": {
                    "tags": ["Connectors"],
                    "summary": "Scan the LAN for Blackmagic cameras",
                    "description": "mDNS scan (5s). Every connected WebSocket client also receives the result as `blackmagic-camera.discovered`.\n\nWhen no camera is configured yet, the first one found is stored and connected — a scan is how a camera gets connected. An already-configured camera is never repointed.\n\nAn empty list is a normal outcome: mDNS does not cross VLANs, and on macOS the app needs Local Network permission. Adding a camera by host works without discovery.",
                    "operationId": "discoverBlackmagicCameras",
                    "responses": {
                        "200": {
                            "description": "Cameras found, possibly none",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "required": ["cameras"],
                                        "properties": {
                                            "cameras": {
                                                "type": "array",
                                                "items": { "$ref": "#/components/schemas/DiscoveredCamera" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "/api/connectors/middlecontrol/discover": {
                "post": {
                    "tags": ["Connectors"],
                    "summary": "Find Middle Control",
                    "description": "Checks localhost and the Metocast server's local /24 network on ports 11584, 11581 and 11580. A TCP listener is returned only after it sends a valid Middle Control feedback frame. The scan does not save or reconnect the connector; use the returned host and port in its config. Routed and VLAN networks may require manual host entry.",
                    "operationId": "discoverMiddlecontrol",
                    "responses": {
                        "200": {
                            "description": "Middle Control endpoints found, possibly none",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "required": ["devices"],
                                        "properties": {
                                            "devices": {
                                                "type": "array",
                                                "items": { "$ref": "#/components/schemas/DiscoveredMiddlecontrol" }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        "401": { "description": "Unauthorized" }
                    }
                }
            },
            "/api/connectors/blackmagic-camera/connect": {
                "post": {
                    "tags": ["Connectors"],
                    "summary": "Connect the Blackmagic camera",
                    "description": "Starts the Blackmagic camera connector using the stored configuration.",
                    "operationId": "connectBlackmagicCamera",
                    "responses": {
                        "204": { "description": "Connection attempt started" },
                        "401": { "description": "Unauthorized" }
                    }
                }
            },
            "/api/connectors/obs/connect": {
                "post": {
                    "tags": ["Connectors"],
                    "summary": "Connect OBS",
                    "description": "Starts the OBS connector using the stored configuration.",
                    "operationId": "connectObs",
                    "responses": {
                        "204": { "description": "Connection attempt started" },
                        "401": { "description": "Unauthorized" }
                    }
                }
            },
            "/api/connectors/obs/disconnect": {
                "post": {
                    "tags": ["Connectors"],
                    "summary": "Disconnect OBS",
                    "operationId": "disconnectObs",
                    "responses": {
                        "204": { "description": "Disconnected" },
                        "401": { "description": "Unauthorized" }
                    }
                }
            },
            "/api/connectors/obs/stream-settings": {
                "get": {
                    "tags": ["Connectors"],
                    "summary": "Get OBS stream destination",
                    "operationId": "getObsStreamSettings",
                    "responses": {
                        "200": {
                            "description": "Current RTMP destination",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/ObsStreamSettings" }
                                }
                            }
                        },
                        "401": { "description": "Unauthorized" },
                        "409": { "description": "OBS is not connected" },
                        "502": { "description": "OBS rejected the request" }
                    }
                },
                "put": {
                    "tags": ["Connectors"],
                    "summary": "Set OBS stream destination",
                    "description": "Applies a custom RTMP destination (`rtmp_custom`) to OBS.",
                    "operationId": "setObsStreamSettings",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/ObsStreamSettings" }
                            }
                        }
                    },
                    "responses": {
                        "204": { "description": "Applied" },
                        "401": { "description": "Unauthorized" },
                        "409": { "description": "OBS is not connected" },
                        "502": { "description": "OBS rejected the request" }
                    }
                }
            },
            "/api/media/probe": {
                "post": {
                    "tags": ["Media"],
                    "summary": "Describe a video file",
                    "description": "Reads a `.braw`, `.mov` or `.mp4` file on the server's disk: its camera metadata, timecode and audio, the stored Media Processing defaults, the frame they produce, and which tools the server still lacks to render it.",
                    "operationId": "probeMedia",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "type": "object", "required": ["path"], "properties": { "path": { "type": "string" }, "sidecarPath": { "type": ["string", "null"], "description": "A .sidecar (such as Resolve's) whose settings become the probe's `defaults`" } } },
                                "example": { "path": "/Volumes/A002/Untitled CAM 1 05.braw" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "What the file is and how it would render",
                            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/MediaProbe" } } }
                        },
                        "400": { "description": "Not an absolute path to a supported video file, or unreadable" },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "No such file" },
                        "503": { "description": "The server has no ffmpeg (the Mac app ships its own)" }
                    }
                }
            },
            "/api/media/preview": {
                "post": {
                    "tags": ["Media"],
                    "summary": "One frame, as recorded or graded",
                    "description": "Develops one frame of a file on the server's disk as a JPEG: as recorded (its own look) without `settings`, else graded with them, plus the crop a render would keep. Runs beside the media queue, for the Mac app's viewer.",
                    "operationId": "previewMedia",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/PreviewRequest" },
                                "example": { "path": "/Volumes/A002/Untitled CAM 1 05.braw", "frame": 20000, "maxWidth": 1600 }
                            }
                        }
                    },
                    "responses": {
                        "200": { "description": "The frame", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/PreviewFrame" } } } },
                        "400": { "description": "Not a supported file, invalid settings, or the decoder failed" },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "No such file" },
                        "503": { "description": "The server lacks a tool this file needs" }
                    }
                }
            },
            "/api/media/renders": {
                "post": {
                    "tags": ["Media"],
                    "summary": "Queue a render",
                    "description": "Queues a render on the `media` job queue (one at a time; list it with `GET /api/queues/media/jobs`). The finished file is written beside the source in `Metocast Renders` (or the configured folder) and filed under the event, or as an untracked recording. Progress arrives as `media.render.progress` WebSocket messages.",
                    "operationId": "startRender",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/RenderRequest" },
                                "example": { "path": "/Volumes/A002/Untitled CAM 1 05.braw" }
                            }
                        }
                    },
                    "responses": {
                        "202": {
                            "description": "Queued",
                            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/RenderQueued" } } }
                        },
                        "400": { "description": "Invalid path or settings" },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "No such file or event" },
                        "409": { "description": "This file is already queued or rendering" },
                        "503": { "description": "Media Processing is turned off, or the server lacks ffmpeg, braw-decode or the Blackmagic RAW SDK" }
                    }
                }
            },
            "/api/media/audio/probe": {
                "post": {
                    "tags": ["Media"],
                    "summary": "Describe audio files",
                    "description": "Reads one or more files on the server's disk (WAV, FLAC, MP3, M4A, AIFF, CAF, Ogg, Opus, or a video's sound): format, length and start time, joined in the order given, plus the stored defaults and which tools are missing or will be downloaded.",
                    "operationId": "probeAudio",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "type": "object", "required": ["paths"], "properties": { "paths": { "type": "array", "items": { "type": "string" } } } },
                                "example": { "paths": ["/Volumes/WirelessPRO 1/00015_Wireless_PRO.WAV", "/Volumes/WirelessPRO 1/00016_Wireless_PRO.WAV"] }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "What the files are",
                            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/AudioProbe" } } }
                        },
                        "400": { "description": "No files, too many, a relative path, an unsupported type, or no audio" },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "No such file" },
                        "503": { "description": "The server has no ffmpeg (the Mac app ships its own)" }
                    }
                }
            },
            "/api/media/audio": {
                "post": {
                    "tags": ["Media"],
                    "summary": "Queue an audio master",
                    "description": "Queues a `process_audio` job on the `media` queue: join the files, mix the chosen channels to mono 48 kHz, clean them with jive-vocals (downloaded on first use), remove the delay its noise reduction adds so the master stays sample-aligned with the source, and lift it to the target loudness. The result is a 24-bit, 48 kHz Broadcast WAV with the voice on both channels, stamped with the recording's start time, in `Metocast Audio` in the Music folder (or the configured folder), filed under the event or as an untracked recording, with jive-vocals' report beside it. Progress arrives as `media.audio.progress`; cancel with `POST /api/media/renders/{id}/cancel`.",
                    "operationId": "startAudio",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/AudioRequest" },
                                "example": { "paths": ["/Volumes/WirelessPRO 1/00015_Wireless_PRO.WAV", "/Volumes/WirelessPRO 1/00016_Wireless_PRO.WAV"] }
                            }
                        }
                    },
                    "responses": {
                        "202": {
                            "description": "Queued",
                            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/RenderQueued" } } }
                        },
                        "400": { "description": "Invalid paths or settings" },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "No such file or event" },
                        "409": { "description": "This recording is already queued or processing" },
                        "503": { "description": "Media Processing is turned off, or the server lacks ffmpeg or a configured jive-vocals" }
                    }
                }
            },
            "/api/media/programs/probe": {
                "post": {
                    "tags": ["Media"],
                    "summary": "Describe a video and its audio",
                    "description": "Reads a video and, optionally, its audio recording (one file or the files a recorder split it into) on the server's disk: the video's facts, the recording's, whether the video's own sound is usable or silent, how the sync will be found (`none`, `waveform` or `lips`), where the clocks put the start (raw, or corrected by a clock difference learned from a recent sync of the same camera), the picture's default treatment, warnings (such as audio that won't cover the video), anything that blocks the job, and missing tools.",
                    "operationId": "probeProgram",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/ProgramProbeRequest" },
                                "example": { "videoPath": "/Volumes/A002/Untitled CAM 1 05.braw", "audioPaths": ["/Volumes/WirelessPRO 1/00015_Wireless_PRO.WAV"] }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "What the files are and how they'd be joined",
                            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ProgramProbe" } } }
                        },
                        "400": { "description": "A relative path, an unsupported type, no audio in an audio file, or the video listed as audio too" },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "No such file" },
                        "503": { "description": "The server has no ffmpeg" }
                    }
                }
            },
            "/api/media/programs": {
                "post": {
                    "tags": ["Media"],
                    "summary": "Queue a program: a video with its audio, synced",
                    "description": "Queues a `produce_program` job on the `media` queue. The picture is graded and encoded like a render (always for Blackmagic RAW) or copied as it is. The audio files are joined and mixed to one voice track, and the job finds where in it the picture's first frame falls: by the video's own sound when it has any, else by the speaker's lips (Apple Vision face landmarks matched against the voice), else by the clocks. Only the stretch the picture needs is cleaned by jive-vocals and lifted to the target loudness. It is then placed under the picture (clock drift corrected, silence where it doesn't reach) so the audio lasts exactly as long as the picture. The result is `<name>.mov` (picture copied, AAC 320k, source timecode kept) with its own 24-bit `<name>.wav` beside it in `Metocast Renders`, filed under the event or as untracked. Progress arrives as `media.program.progress`; cancel with `POST /api/media/renders/{id}/cancel`.",
                    "operationId": "startProgram",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/ProgramRequest" },
                                "example": { "videoPath": "/Volumes/A002/Untitled CAM 1 05.braw", "audioPaths": ["/Volumes/WirelessPRO 1/00015_Wireless_PRO.WAV"] }
                            }
                        }
                    },
                    "responses": {
                        "202": {
                            "description": "Queued",
                            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/RenderQueued" } } }
                        },
                        "400": { "description": "Invalid paths or settings, Blackmagic RAW asked to be copied, or a video alone without usable sound" },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "No such file or event" },
                        "409": { "description": "This video is already queued or being produced" },
                        "503": { "description": "Media Processing is turned off, a tool is missing, or the output volume lacks space" }
                    }
                }
            },
            "/api/media/programs/{id}/nudge": {
                "parameters": [
                    { "name": "id", "in": "path", "required": true, "description": "Job id from `POST /api/media/programs`", "schema": { "type": "string", "format": "uuid" } }
                ],
                "post": {
                    "tags": ["Media"],
                    "summary": "Move a finished program's audio by whole frames",
                    "description": "Queues a `nudge_program` job: the program's WAV is shifted (keeping its length) and the picture copied again with it, which takes seconds. The recording's sync report records the move. Progress arrives as `media.program.progress` under the program's job id.",
                    "operationId": "nudgeProgram",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/NudgeRequest" },
                                "example": { "frames": 1 }
                            }
                        }
                    },
                    "responses": {
                        "202": {
                            "description": "Queued",
                            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/RenderQueued" } } }
                        },
                        "400": { "description": "Zero frames, or more than 240" },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "No finished program with that id, or its files are gone" },
                        "409": { "description": "This program is already being adjusted" }
                    }
                }
            },
            "/api/media/programs/{id}/subtitles": {
                "parameters": [
                    { "name": "id", "in": "path", "required": true, "description": "Job id from `POST /api/media/programs`", "schema": { "type": "string", "format": "uuid" } }
                ],
                "post": {
                    "tags": ["Media"],
                    "summary": "Place transcripts on a finished program",
                    "description": "Queues a `subtitle_program` job: each transcript is placed on the finished program exactly as a program job places it. An English one is rebuilt into sentences and placed on the speech with Whisper (the model downloads on first use); others are matched with the voice. It is written beside the program as `<name>.<language>.srt`, replaces that language in the program's report, and, for a program filed under an event, replaces that caption track and sends it to an upload that already finished. Progress arrives as `media.program.progress` under the program's job id (step `download`, then `captions` with a fraction).",
                    "operationId": "addProgramSubtitles",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/SubtitlesRequest" },
                                "example": { "subtitles": [{ "path": "/Volumes/A002/2026-10-18 10-35-32.en.vtt", "language": "en" }] }
                            }
                        }
                    },
                    "responses": {
                        "202": {
                            "description": "Queued",
                            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/RenderQueued" } } }
                        },
                        "400": { "description": "No transcripts, one that isn't a readable .srt or .vtt, or two in one language" },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "No finished program with that id, or its files are gone" },
                        "409": { "description": "This program is already being adjusted" }
                    }
                }
            },
            "/api/media/renders/{id}/cancel": {
                "parameters": [
                    { "name": "id", "in": "path", "required": true, "description": "Job id from `POST /api/media/renders`, `POST /api/media/audio` or `POST /api/media/programs`", "schema": { "type": "string", "format": "uuid" } }
                ],
                "post": {
                    "tags": ["Media"],
                    "summary": "Cancel a render, audio or program job",
                    "description": "Stops a queued or running media job. Its partial file is removed and the job ends dead with `cancelled` as its error.",
                    "operationId": "cancelRender",
                    "responses": {
                        "204": { "description": "Cancelled" },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "No queued or running media job with that id" }
                    }
                }
            },
            "/api/automations": {
                "get": {
                    "tags": ["Automations"],
                    "summary": "List the automations",
                    "description": "The rules and the master switch. Every client sees and edits the same rules; changes are broadcast as `automations.changed`.",
                    "operationId": "listAutomations",
                    "responses": {
                        "200": {
                            "description": "The rules, oldest first",
                            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/AutomationList" } } }
                        },
                        "401": { "description": "Unauthorized" }
                    }
                },
                "post": {
                    "tags": ["Automations"],
                    "summary": "Create an automation",
                    "operationId": "createAutomation",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/AutomationDraft" },
                                "example": {
                                    "name": "Service end",
                                    "triggers": [
                                        { "kind": "rodecaster.channel", "channel": 2, "label": "Wireless 1", "live": false, "source": "either", "holdSeconds": 5 }
                                    ],
                                    "actions": [
                                        { "kind": "record.stop", "target": { "device": "rodecaster-audio" } },
                                        { "kind": "record.stop", "target": { "device": "obs" } },
                                        { "kind": "middlecontrol.preset.recall", "preset": 1, "cameraId": null, "waitForArrival": false }
                                    ],
                                    "cooldownSeconds": 5,
                                    "requireEvent": false
                                }
                            }
                        }
                    },
                    "responses": {
                        "201": {
                            "description": "Created",
                            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Automation" } } }
                        },
                        "400": { "description": "A limit the rule breaks (preset not 1–12, camera not 1–99, hold not 0–60 s, cooldown not 1–600 s, no trigger or action, a name of 0 or more than 80 characters, more than 50 rules); `{ \"error\": \"…\" }`" },
                        "401": { "description": "Unauthorized" },
                        "422": { "description": "The body is not shaped like a rule" }
                    }
                }
            },
            "/api/automations/runs": {
                "get": {
                    "tags": ["Automations"],
                    "summary": "The last 50 runs",
                    "description": "Newest first. The history lives in memory; the core's log keeps the details across restarts.",
                    "operationId": "listAutomationRuns",
                    "responses": {
                        "200": {
                            "description": "The runs",
                            "content": { "application/json": { "schema": { "type": "array", "items": { "$ref": "#/components/schemas/AutomationRun" } } } }
                        },
                        "401": { "description": "Unauthorized" }
                    }
                }
            },
            "/api/automations/armed": {
                "put": {
                    "tags": ["Automations"],
                    "summary": "Arm or disarm every automation",
                    "description": "The master switch. While it is off nothing fires; each rule keeps its own enabled toggle. Broadcast as `automations.changed`.",
                    "operationId": "setAutomationsArmed",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "type": "object", "required": ["armed"], "properties": { "armed": { "type": "boolean" } } },
                                "example": { "armed": false }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Saved",
                            "content": { "application/json": { "schema": { "type": "object", "required": ["armed"], "properties": { "armed": { "type": "boolean" } } } } }
                        },
                        "401": { "description": "Unauthorized" }
                    }
                }
            },
            "/api/automations/{id}": {
                "parameters": [
                    { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
                ],
                "put": {
                    "tags": ["Automations"],
                    "summary": "Replace an automation",
                    "operationId": "replaceAutomation",
                    "requestBody": {
                        "required": true,
                        "content": { "application/json": { "schema": { "$ref": "#/components/schemas/AutomationDraft" } } }
                    },
                    "responses": {
                        "200": {
                            "description": "Replaced",
                            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Automation" } } }
                        },
                        "400": { "description": "A limit the rule breaks" },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "No such automation" },
                        "422": { "description": "The body is not shaped like a rule" }
                    }
                },
                "delete": {
                    "tags": ["Automations"],
                    "summary": "Delete an automation",
                    "operationId": "deleteAutomation",
                    "responses": {
                        "204": { "description": "Deleted" },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "No such automation" }
                    }
                }
            },
            "/api/automations/{id}/run": {
                "parameters": [
                    { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
                ],
                "post": {
                    "tags": ["Automations"],
                    "summary": "Run an automation now",
                    "description": "Runs the rule's actions immediately, ignoring its cooldown, `requireEvent` and the master switch. It exists to check a setup: it really records and really moves cameras. The run waits its turn behind any run in progress (runs execute one at a time) and is returned when it ends; with a camera move that waits for arrival it can take as long as the preset's transition time. The run is marked `manual`.",
                    "operationId": "runAutomation",
                    "responses": {
                        "200": {
                            "description": "The finished run, one result per action. A device that is not connected shows as a failed action with its connector's code; the other actions still ran.",
                            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/AutomationRun" } } }
                        },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "No such automation" }
                    }
                }
            },
            "/api/openai/verify": {
                "post": {
                    "tags": ["Transcription"],
                    "summary": "Verify an OpenAI API key",
                    "description": "Lists models with the key (`GET https://api.openai.com/v1/models`). A blank key checks the stored one. The key is never returned or logged.",
                    "operationId": "verifyOpenAiKey",
                    "requestBody": {
                        "required": false,
                        "content": {
                            "application/json": {
                                "schema": { "type": "object", "properties": { "apiKey": { "type": "string" } } }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "`ok`, `invalid` (OpenAI answered 401) or `noAccess` (anything else)",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "required": ["result", "message"],
                                        "properties": {
                                            "result": { "type": "string", "enum": ["ok", "invalid", "noAccess"] },
                                            "message": { "type": "string" }
                                        }
                                    }
                                }
                            }
                        },
                        "401": { "description": "Unauthorized" }
                    }
                }
            },
            "/api/recordings/{id}/captions": {
                "parameters": [
                    { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
                ],
                "post": {
                    "tags": ["Transcription"],
                    "summary": "Generate English captions for a recording",
                    "description": "Queues a `youtube.captions` job on the `captions` queue. The file is played to the realtime translation model at 1×, cue starts are snapped to speech onsets, and the WebVTT is uploaded to the recording's YouTube video as \"English\" (replacing an earlier upload). Progress arrives as `captions.job.state` WebSocket messages.",
                    "operationId": "generateRecordingCaptions",
                    "responses": {
                        "202": {
                            "description": "Queued",
                            "content": { "application/json": { "schema": { "type": "object", "properties": { "queued": { "type": "boolean" } } } } }
                        },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "No such recording" }
                    }
                }
            },
            "/api/transcripts": {
                "get": {
                    "tags": ["Transcription"],
                    "summary": "List stored live transcripts",
                    "description": "Every live-transcript session kept in the database, newest first. A session's English lines are stored as they are committed; `live` marks the one being written now.",
                    "operationId": "listTranscripts",
                    "parameters": [
                        { "name": "eventId", "in": "query", "required": false, "schema": { "type": "string", "format": "uuid" }, "description": "Only this event's sessions" }
                    ],
                    "responses": {
                        "200": { "description": "The sessions", "content": { "application/json": { "schema": { "type": "array", "items": { "$ref": "#/components/schemas/StoredTranscript" } } } } },
                        "401": { "description": "Unauthorized" }
                    }
                }
            },
            "/api/transcripts/{id}": {
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "get": {
                    "tags": ["Transcription"],
                    "summary": "Read a stored transcript",
                    "operationId": "getTranscript",
                    "responses": {
                        "200": { "description": "The session and every line, with the cleaned-up text when a clean-up ran", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/TranscriptDetail" } } } },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "No such transcript" }
                    }
                },
                "delete": {
                    "tags": ["Transcription"],
                    "summary": "Delete a stored transcript",
                    "description": "Removes the session and all its lines, raw and cleaned. Cannot be undone.",
                    "operationId": "deleteTranscript",
                    "responses": {
                        "204": { "description": "Deleted" },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "No such transcript" },
                        "409": { "description": "`transcript_live` (it is being written now) or `transcript_cleanup_running`" }
                    }
                }
            },
            "/api/transcripts/{id}/download": {
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "get": {
                    "tags": ["Transcription"],
                    "parameters": [
                        { "name": "format", "in": "query", "required": false, "schema": { "type": "string", "enum": ["txt", "srt", "vtt"], "default": "txt" } },
                        { "name": "clean", "in": "query", "required": false, "schema": { "type": "boolean", "default": false }, "description": "The cleaned-up lines instead of the raw ones; lines the clean-up dropped as filler are left out" }
                    ],
                    "summary": "Download a stored transcript",
                    "description": "Plain text, one line per sentence, or subtitles timed from the session's start. Sent as an attachment named after the start time.",
                    "operationId": "downloadTranscript",
                    "responses": {
                        "200": { "description": "The file", "content": { "text/plain": { "schema": { "type": "string" } }, "application/x-subrip": { "schema": { "type": "string" } }, "text/vtt": { "schema": { "type": "string" } } } },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "No such transcript" },
                        "409": { "description": "`transcript_not_cleaned`: `clean=true` before a clean-up succeeded" }
                    }
                }
            },
            "/api/transcripts/{id}/cleanup": {
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "post": {
                    "tags": ["Transcription"],
                    "summary": "Clean up a stored transcript with AI",
                    "description": "Sends the finished English lines to OpenAI (`gpt-5-mini`, with the key of the `openai` connector) in chunks of 40 and keeps a rewritten version of each line next to the raw one: grammar and mistranslations fixed, filler dropped, nothing added. Progress arrives as `transcript.cleanup.state` WebSocket messages; a successful pass replaces an earlier clean version, a failed one leaves it.",
                    "operationId": "cleanupTranscript",
                    "responses": {
                        "202": { "description": "Started", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/CleanupProgress" } } } },
                        "400": { "description": "`transcription_not_configured`" },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "No such transcript" },
                        "409": { "description": "`transcript_live`, `transcript_cleanup_running` or `transcript_empty`" }
                    }
                }
            },
            "/api/recordings/{id}/captions.vtt": {
                "parameters": [
                    { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
                ],
                "get": {
                    "tags": ["Transcription"],
                    "summary": "Download the English captions",
                    "operationId": "getRecordingCaptions",
                    "responses": {
                        "200": { "description": "The last generated track", "content": { "text/vtt": { "schema": { "type": "string" } } } },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "No captions generated yet" }
                    }
                }
            },
            "/ws": {
                "get": {
                    "tags": ["WebSocket"],
                    "summary": "WebSocket live stream",
                    "description": "**This endpoint performs a WebSocket upgrade — it cannot be tested with the HTTP \"Send\" button.**\n\nUse a dedicated WebSocket client instead:\n- [Hoppscotch](https://hoppscotch.io) → New request → WebSocket\n- [websocat](https://github.com/vi/websocat): `websocat 'ws://<host>/ws?token=<token>'`\n- Bruno: add a request with type `socket`\n\n---\n\n**Connection URL:** `ws://<host>/ws?token=<token>`\n\nAuthentication uses the same bearer token passed as a **query parameter** (the `Authorization` header is not available during the WebSocket handshake).\n\nA screen that shows SVG slides as JPEGs from `presenter.frame` (the Raspberry Pi receiver) adds `presenter=frames`: its `presenter.state` messages then leave each `svgSlides[].svg` empty and carry `svgOmitted: true`.\n\nEach client's messages are queued; a `presenter.state` still waiting to be sent when a newer one is broadcast is dropped, together with the `presenter.slide_changed` messages queued before the newer one, so a slow screen jumps straight to the current deck.\n\n### Initial messages (pushed immediately on connect)\n\n```json\n{ \"type\": \"connected\", \"serverId\": \"<uuid>\" }\n{ \"type\": \"connector.status\", \"connector\": \"obs\",  \"status\": { \"type\": \"connected\" } }\n{ \"type\": \"connector.status\", \"connector\": \"vmix\", \"status\": { \"type\": \"disconnected\" } }\n```\n\n### Broadcast messages (sent when data changes)\n\n| `type` | Trigger | Schema |\n|---|---|---|\n| `connector.status` | OBS or VMix connection state changes | `WsConnectorStatusMessage` |\n| `event.changed` | Event created, updated, or deleted | `WsEventChangedMessage` |\n| `recording.changed` | Recording created or updated | `WsRecordingChangedMessage` |\n| `media.render.progress` | A render's progress, about once a second, and once when it ends | `WsMediaRenderProgressMessage` |\n| `media.audio.progress` | An audio job's step and progress, and its report when it ends | `WsMediaAudioProgressMessage` |\n| `media.program.progress` | A program job's step and progress, and its report when it ends, is nudged or gets transcripts | `WsMediaProgramProgressMessage` |\n| `transcription.snapshot` | On connect: the state, the last ~20 segments, the open line and the credit line | `{ type, state: TranscriptionState, segments: TranscriptSegment[], partial, credit: CreditStatus \\| null }` |\n| `transcription.state` | The live transcript's status changes | `{ type, state: TranscriptionState }` |\n| `transcription.partial` | The open English line grows (at most every ~150 ms; empty clears it) | `{ type, text }` |\n| `transcription.segment` | A line is committed | `{ type, segment: TranscriptSegment }` |\n| `transcription.credit` | Every 10 minutes and after a session, when an admin key is set | `{ type, credit: CreditStatus }` |\n| `captions.job.state` | A final-video captions job runs, succeeds or fails | `{ type, jobId, recordingId, state, progress, error }` |\n| `transcript.cleanup.state` | A stored transcript's AI clean-up runs a chunk, succeeds or fails | `CleanupProgress` plus `type` |\n| `automations.changed` | The rules or the master switch changed; also sent on connect to clients with the token | `WsAutomationsChangedMessage` |\n| `automation.run` | A rule ran | `WsAutomationRunMessage` |\n| `rodecaster.record.state` | The RØDECaster's own SD/USB recorder started, paused or stopped, or a card or drive came or went; also sent on connect | `WsRodecasterRecordStateMessage` |\n| `connector.state` (`blackmagic-camera`) | The camera's recording, livestream, media or stream destination changed; also sent on connect | `{ type, connector, isStreaming, streamStatus, isRecording, hasMedia, destination }`; `hasMedia` is false with no card or SSD in the camera. `destination` is null on a camera without livestreaming, else `{ service, url, broadcast, matchesYoutube }`: `service` is `youtube`, `facebook`, `discord`, `middlecontrol` or `other`; `url` is scheme and host only (`rtmp://host/…`), never the stream key; `broadcast` is today's event when it has a broadcast on that service; `matchesYoutube` is true when the camera already has the channel's YouTube stream key, null when that can't be checked |\n| `connector.state` (`middlecontrol`) | Middle Control's feedback changed. `runningPreset` (1–12, or null at rest) is the preset moving the selected camera, `presetMoveActive` is derived from it, and `lastPreset { preset, cameraId, at }` is the last move Metocast watched run to its end | `{ type, connector, state: { selectedCamera, recording, recordingCameraIds, connectedCameraIds, connectedApcrIds, presetMoveActive, runningPreset, lastPreset } }` |\n| `presenter.state` | Presentation loaded or unloaded | `{ type, state: PresenterState, svgOmitted }` with `renderMode: \"text\" | \"svg\"`; `svgOmitted` is true for a `presenter=frames` client, whose `svgSlides[].svg` are empty |\n| `presenter.slide_changed` | Slide navigation | `{ type, currentSlide, totalSlides }` |\n\n### Presenter WS commands\n\n| Command | Fields | Description |\n|---|---|---|\n| `presenter.load` | `file_path`, optional `render_mode` | Load a .pptx into the presenter; defaults to text mode |\n| `presenter.load_bible_reference` | optional `event_id`, `reference_type` | Load an event Textus/Lekció Bible reference into the text presenter. If `event_id` is omitted, the backend-selected presenter event is used. |\n| `presenter.unload` | — | Clear the active presentation |\n| `presenter.next` | — | Advance one slide |\n| `presenter.prev` | — | Go back one slide |\n| `presenter.first` | — | Jump to slide 1 |\n| `presenter.last` | — | Jump to last slide |\n| `presenter.goto` | `slide` | Jump to a specific slide number |\n| `presenter.status` | optional `svg` | Reply to requesting client with `presenter.state`; a `presenter=frames` client gets the SVGs only with `svg: true` |\n| `presenter.frame` | `slide`, `width`, `height` | Reply to requesting client with one binary message: that SVG slide drawn as a JPEG for a screen that size (up to 3840×2160), or `error`. The Raspberry Pi receiver shows SVG slides this way |\n| `presenter.ack` | `slide`, `shown_at` | No reply. A screen shows `slide` since `shown_at` (RFC 3339, its own clock); logged with the delay since the server sent it while the delivery log is on |\n| `presentation.set_delivery_log` | `enabled` | Log every presenter message sent to each screen (size, queue wait, write time, decks dropped) and every `presenter.ack` under the `presenter_delivery` target; broadcasts `presentation.settings` with `deliveryLog` |\n\n### Live transcript WS commands (token required)\n\n| Command | Fields | Description |\n|---|---|---|\n| `transcription.start` | optional `eventId` | Start translating the configured RØDECaster source for an event, or for none when `eventId` is omitted. Answered by `transcription.state`, or `error` with `transcription_not_configured` / `transcription_unavailable` (no RØDECaster) |\n| `transcription.stop` | — | Stop it; answered by `ok` |\n\n### Automation and RØDECaster recorder WS commands (token required)\n\n| Command | Fields | Description |\n|---|---|---|\n| `automations.list` | — | Answered by `automations.changed` |\n| `automations.run` | `id` | Run the rule now (ignores its cooldown, event check and the master switch); answered by `ok` or `error` (`not_found`), the run arrives as `automation.run` |\n| `automations.set_armed` | `armed` | The master switch; answered by `ok`, and `automations.changed` is broadcast |\n| `rodecaster.record.start` | — | Start (or resume) the desk's own SD/USB recorder; answered by `ok` or `error` (`rodecaster_not_connected`, `rodecaster_no_media`); the result arrives as `rodecaster.record.state` |\n| `rodecaster.record.stop` | — | Stop it |\n| `blackmagic-camera.record.start`, `.record.stop`, `.stream.start`, `.stream.stop` | — | Answered by `ok` once the camera's state shows the change, or `error`: `blackmagic_camera_not_connected`, `camera_no_media` (no card or SSD), `camera_stream_held_by_middlecontrol` (the stream is Middle Control's camera feed), `camera_ignored_command` (nothing changed within 5 s) |\n| `middlecontrol.record.start`, `.record.stop`, `.record.start_all`, `.record.stop_all` | optional `camera_id` | Answered by `ok` once Middle Control reports the change, or `error`: `camera_no_media`, `middlecontrol_ignored_command` |\n\n### Event WS commands for presenter controls\n\n| Command | Fields | Description |\n|---|---|---|\n| `events.presenter_list` | — | Return backend-ordered presenter event choices plus `selectedEventId` for the current/next event |\n\n`presentation.open` uses SVG mode by default in web-presenter mode. Send `render_mode: \"text\"` to force the text renderer.",
                    "operationId": "connectWebSocket",
                    "security": [],
                    "parameters": [
                        {
                            "name": "token",
                            "in": "query",
                            "required": true,
                            "description": "Bearer auth token",
                            "schema": { "type": "string" }
                        },
                        {
                            "name": "presenter",
                            "in": "query",
                            "required": false,
                            "description": "`frames` for a screen that shows SVG slides from `presenter.frame`: its `presenter.state` messages leave the SVGs out (`svgOmitted: true`)",
                            "schema": { "type": "string", "enum": ["frames"] }
                        }
                    ],
                    "responses": {
                        "101": { "description": "Switching Protocols — WebSocket handshake accepted (only reachable via a WebSocket client)" },
                        "426": {
                            "description": "Upgrade Required — returned when this endpoint is called as a plain HTTP request instead of a WebSocket upgrade",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "error":       { "type": "string", "example": "upgrade_required" },
                                            "description": { "type": "string" },
                                            "connect":     { "type": "string", "example": "ws://<host>/ws?token=<your-token>" },
                                            "auth":        { "type": "string" }
                                        }
                                    }
                                }
                            }
                        },
                        "401": { "description": "Unauthorized — token missing or invalid" }
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_transcription_routes_are_documented() {
        let spec = super::spec();
        for path in [
            "/api/openai/verify",
            "/api/recordings/{id}/captions",
            "/api/recordings/{id}/captions.vtt",
            "/api/transcripts",
            "/api/transcripts/{id}",
            "/api/transcripts/{id}/download",
            "/api/transcripts/{id}/cleanup",
        ] {
            assert!(spec["paths"][path].is_object(), "{path} is missing");
        }
        for schema in [
            "TranscriptionState",
            "TranscriptSegment",
            "CreditStatus",
            "StoredTranscript",
            "TranscriptDetail",
            "CleanupProgress",
        ] {
            assert!(
                spec["components"]["schemas"][schema].is_object(),
                "{schema} is missing"
            );
        }
    }

    #[test]
    fn the_automation_routes_are_documented() {
        let spec = super::spec();
        for path in [
            "/api/automations",
            "/api/automations/runs",
            "/api/automations/armed",
            "/api/automations/{id}",
            "/api/automations/{id}/run",
            "/api/settings/record-button",
            "/api/recorders",
        ] {
            assert!(spec["paths"][path].is_object(), "{path} is missing");
        }
        for schema in [
            "ChannelSource",
            "AutomationTrigger",
            "RecordTarget",
            "AutomationAction",
            "AutomationDraft",
            "Automation",
            "AutomationList",
            "ActionResult",
            "AutomationRun",
            "WsAutomationsChangedMessage",
            "WsAutomationRunMessage",
            "WsRodecasterRecordStateMessage",
            "RecordButton",
            "RecorderRow",
            "WsRecordersStateMessage",
            "WsRecorderStoppedMessage",
        ] {
            assert!(
                spec["components"]["schemas"][schema].is_object(),
                "{schema} is missing"
            );
        }
    }

    /// The documented example is what the core's own types read and write.
    #[test]
    fn the_documented_automation_example_is_a_real_automation() {
        let spec = super::spec();
        let example = spec["components"]["schemas"]["Automation"]["example"].clone();
        let automation: metocast_core::automations::Automation =
            serde_json::from_value(example).expect("the example decodes");
        assert_eq!(automation.draft().validate(), Ok(()));
        let example =
            spec["paths"]["/api/automations"]["post"]["requestBody"]["content"]["application/json"]
                ["example"]
                .clone();
        let draft: metocast_core::automations::AutomationDraft =
            serde_json::from_value(example).expect("the request example decodes");
        assert_eq!(draft.validate(), Ok(()));
    }

    #[test]
    fn the_media_routes_are_documented() {
        let spec = super::spec();
        for path in [
            "/api/media/probe",
            "/api/media/preview",
            "/api/media/renders",
            "/api/media/renders/{id}/cancel",
            "/api/media/audio/probe",
            "/api/media/audio",
            "/api/media/programs/probe",
            "/api/media/programs",
            "/api/media/programs/{id}/nudge",
            "/api/media/programs/{id}/subtitles",
        ] {
            assert!(spec["paths"][path].is_object(), "{path} is missing");
        }
        for schema in [
            "MediaProbe",
            "RenderSettings",
            "RawOptions",
            "PreviewRequest",
            "PreviewFrame",
            "SubtitleSource",
            "SubtitleReport",
            "WsMediaRenderProgressMessage",
            "AudioProbe",
            "AudioRequest",
            "WsMediaAudioProgressMessage",
            "ProgramProbe",
            "ProgramRequest",
            "SyncReport",
            "WsMediaProgramProgressMessage",
            "NudgeRequest",
            "SubtitlesRequest",
        ] {
            assert!(
                spec["components"]["schemas"][schema].is_object(),
                "{schema} is missing"
            );
        }
    }
}

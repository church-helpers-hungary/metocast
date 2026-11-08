pub mod auth;
pub mod broadcast;
pub mod caption;
pub mod mcp;
pub mod mcp_content;
pub mod openapi;
pub mod openapi_routes;
pub mod order;
pub mod ppt;
pub mod presenter;
pub mod redact;
pub mod routes;
pub mod websocket;
pub mod welcome;
pub mod ws_client;
pub mod ws_events;

use crate::HostHandle;
use axum::{
    Router, middleware,
    routing::{delete, get, post, put},
};
use metocast_core::best_effort::{LogFailed, LogGone};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;
use tokio::net::TcpListener;
use tokio::sync::{RwLock, oneshot, watch};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use uuid::Uuid;

use axum::extract::ws::Message;
use serde_json::json;
use sqlx::PgPool;

#[cfg(target_os = "macos")]
use crate::connectors::keynote::KeynoteConnector;
use crate::connectors::{
    FacebookConfig, YouTubeConfig, atem::AtemConnector,
    blackmagic_camera::BlackmagicCameraConnector, broadlink::BroadlinkConnector,
    facebook::FacebookConnector, middlecontrol::MiddlecontrolConnector, obs::ObsConnector,
    rodecaster::RodecasterConnector, rodecaster_audio::RodecasterAudioRecorder,
    vmix::VmixConnector, youtube::YouTubeConnector,
};
use crate::models::event::find_current_event;
use crate::obs_devices::ObsAvailableDevices;
use crate::scheduler::CronScheduler;
use crate::uploader::UploadService;

/// Fixed port for OAuth callbacks — must match Google/Facebook Cloud Console configuration.
pub(crate) const OAUTH_CALLBACK_PORT: u16 = 8766;
/// Exact redirect URI registered in both Google and Facebook Cloud Consoles.
pub(crate) const OAUTH_REDIRECT_URI: &str = "http://127.0.0.1:8766/callback";

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub auth_token: Arc<RwLock<String>>,
    pub ws_clients: Arc<RwLock<HashMap<Uuid, crate::server::ws_client::ClientTx>>>,
    pub server_id: String,
    pub obs_connector: Arc<ObsConnector>,
    pub blackmagic_camera_connector: Arc<BlackmagicCameraConnector>,
    pub middlecontrol_connector: Arc<MiddlecontrolConnector>,
    pub atem_connector: Arc<AtemConnector>,
    pub rodecaster_connector: Arc<RodecasterConnector>,
    pub rodecaster_audio_recorder: Arc<RodecasterAudioRecorder>,
    pub vmix_connector: Arc<VmixConnector>,
    pub youtube_connector: Arc<YouTubeConnector>,
    pub facebook_connector: Arc<FacebookConnector>,
    pub broadlink_connector: Arc<BroadlinkConnector>,
    pub broadlink_learn_active: Arc<AtomicBool>,
    pub youtube_config: Arc<RwLock<YouTubeConfig>>,
    pub facebook_config: Arc<RwLock<FacebookConfig>>,
    /// Pending OAuth state tokens: state_string → (connector_name, created_at)
    pub oauth_states: Arc<RwLock<HashMap<String, (String, Instant)>>>,
    pub host: Option<HostHandle>,
    /// Read access to stored upstream credentials — see `routes::reveal_connector_secrets`.
    pub admin_token: Arc<String>,
    pub cron_scheduler: Arc<CronScheduler>,
    pub upload_service: Arc<UploadService>,
    /// Cached result of the last OBS device scan; `None` until first scan completes.
    pub obs_available_devices: Arc<tokio::sync::RwLock<Option<ObsAvailableDevices>>>,
    /// In-memory state for the active web-presenter session.
    pub presenter_state: Arc<tokio::sync::RwLock<presenter::PresenterState>>,
    /// Whether to use the web presenter instead of Keynote; persisted in app_settings.
    pub use_web_presenter: Arc<AtomicBool>,
    /// Visual treatment for text-mode song and Bible slides; persisted in app_settings.
    pub presenter_theme: Arc<RwLock<presenter::PresenterTheme>>,
    /// The Order of Service step on screen, if one is running. Not stored: a restart starts fresh.
    pub order_position: Arc<RwLock<Option<metocast_core::events::OrderPosition>>>,
    /// Metadata for every currently-connected WebSocket client.
    pub ws_client_info: Arc<tokio::sync::RwLock<HashMap<Uuid, websocket::WsClientInfo>>>,
    /// Wakes the job-queue worker as soon as `queue_changed` fires.
    pub queue_wake: Arc<tokio::sync::Notify>,
    /// The `media` render worker's wake-up and cancel handles.
    pub media: Arc<crate::media::MediaRuntime>,
    pub transcription: Arc<crate::transcription::TranscriptionService>,
    /// Rules that run inside the core: when one device does something, Metocast does something
    /// on others.
    pub automations: Arc<crate::automations::AutomationService>,
    /// The Record button's recorders and the last start of each.
    pub recorders: Arc<crate::recorders::Recorders>,
    #[cfg(target_os = "macos")]
    pub keynote_connector: Arc<KeynoteConnector>,
}

pub struct ServerOptions {
    pub pool: PgPool,
    pub auth_token: Arc<RwLock<String>>,
    pub connection_url: String,
    /// The server's own folder (the database lives here too); media tools download into it.
    pub data_dir: std::path::PathBuf,
    pub port: u16,
    pub static_dir: Option<String>,
    pub obs_connector: Arc<ObsConnector>,
    pub blackmagic_camera_connector: Arc<BlackmagicCameraConnector>,
    pub middlecontrol_connector: Arc<MiddlecontrolConnector>,
    pub atem_connector: Arc<AtemConnector>,
    pub rodecaster_connector: Arc<RodecasterConnector>,
    pub vmix_connector: Arc<VmixConnector>,
    pub youtube_connector: Arc<YouTubeConnector>,
    pub facebook_connector: Arc<FacebookConnector>,
    pub broadlink_connector: Arc<BroadlinkConnector>,
    pub youtube_config: Arc<RwLock<YouTubeConfig>>,
    pub facebook_config: Arc<RwLock<FacebookConfig>>,
    pub oauth_states: Arc<RwLock<HashMap<String, (String, Instant)>>>,
    pub host: Option<HostHandle>,
    pub admin_token: Arc<String>,
    pub cron_scheduler: Arc<CronScheduler>,
    pub shutdown: Option<watch::Receiver<bool>>,
    pub ready: Option<oneshot::Sender<Result<SocketAddr, String>>>,
    #[cfg(target_os = "macos")]
    pub keynote_connector: Arc<KeynoteConnector>,
}

/// Serve a file from the frontend assets embedded in the executable. The resolver itself
/// already falls back `/presenter` -> `presenter.html` -> `presenter/index.html` -> `index.html`.
fn serve_embedded_asset(host: &HostHandle, path: &str) -> axum::response::Response {
    use axum::response::IntoResponse;

    match host.asset(path) {
        Ok(Some(asset)) => (
            [(axum::http::header::CONTENT_TYPE, asset.mime_type)],
            asset.bytes,
        )
            .into_response(),
        Ok(None) | Err(_) => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

pub async fn build_and_serve(options: ServerOptions) -> anyhow::Result<()> {
    let ServerOptions {
        pool,
        auth_token,
        connection_url,
        data_dir,
        port,
        static_dir,
        obs_connector,
        blackmagic_camera_connector,
        middlecontrol_connector,
        atem_connector,
        rodecaster_connector,
        vmix_connector,
        youtube_connector,
        facebook_connector,
        broadlink_connector,
        youtube_config,
        facebook_config,
        oauth_states,
        host,
        admin_token,
        cron_scheduler,
        shutdown,
        ready,
        #[cfg(target_os = "macos")]
        keynote_connector,
    } = options;
    redact::add_secret(&auth_token.read().await);
    redact::add_secret(&admin_token);
    let ws_clients: Arc<RwLock<HashMap<Uuid, crate::server::ws_client::ClientTx>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let server_id = Uuid::new_v4().to_string();

    // Create the upload service here so it shares the ws_clients Arc.
    let upload_service = Arc::new(UploadService::new(
        pool.clone(),
        Arc::clone(&youtube_connector),
        Arc::clone(&facebook_connector),
        Arc::clone(&obs_connector),
        Arc::clone(&facebook_config),
        Arc::clone(&ws_clients),
    ));

    // Initial scheduler load — runs with the real ws_clients so broadcasts reach clients.
    {
        let pool_c = pool.clone();
        let clients_c = ws_clients.clone();
        let yt_c = youtube_connector.clone();
        let sched_c = cron_scheduler.clone();
        let us_c = upload_service.clone();
        tokio::spawn(async move {
            sched_c.reload(pool_c, clients_c, yt_c, us_c).await;
        });
    }

    let obs_available_devices: Arc<tokio::sync::RwLock<Option<ObsAvailableDevices>>> =
        Arc::new(tokio::sync::RwLock::new(None));

    let presenter_state: Arc<tokio::sync::RwLock<presenter::PresenterState>> =
        Arc::new(tokio::sync::RwLock::new(presenter::PresenterState::empty()));

    let use_web_presenter_val: bool =
        sqlx::query_scalar("SELECT value FROM app_settings WHERE key = 'use_web_presenter'")
            .fetch_optional(&pool)
            .await
            .unwrap_or(None)
            .and_then(|v: String| v.parse().ok())
            .unwrap_or(false);
    let use_web_presenter = Arc::new(AtomicBool::new(use_web_presenter_val));
    let presenter_theme = Arc::new(RwLock::new(
        sqlx::query_scalar("SELECT value FROM app_settings WHERE key = 'presenter_theme'")
            .fetch_optional(&pool)
            .await
            .unwrap_or(None)
            .and_then(|value: String| value.parse().ok())
            .unwrap_or_default(),
    ));

    let ws_client_info: Arc<tokio::sync::RwLock<HashMap<Uuid, websocket::WsClientInfo>>> =
        Arc::new(tokio::sync::RwLock::new(HashMap::new()));
    let rodecaster_audio_recorder = Arc::new(RodecasterAudioRecorder::new());

    let transcription = crate::transcription::TranscriptionService::new(
        pool.clone(),
        ws_clients.clone(),
        rodecaster_connector.clone(),
    );
    let (automations, automation_jobs) =
        crate::automations::AutomationService::new(pool.clone(), ws_clients.clone());
    let state = AppState {
        pool,
        auth_token,
        ws_clients: ws_clients.clone(),
        server_id,
        obs_connector: obs_connector.clone(),
        blackmagic_camera_connector: blackmagic_camera_connector.clone(),
        middlecontrol_connector: middlecontrol_connector.clone(),
        atem_connector: atem_connector.clone(),
        rodecaster_connector: rodecaster_connector.clone(),
        rodecaster_audio_recorder: rodecaster_audio_recorder.clone(),
        vmix_connector,
        youtube_connector: youtube_connector.clone(),
        facebook_connector: facebook_connector.clone(),
        broadlink_connector: broadlink_connector.clone(),
        broadlink_learn_active: Arc::new(AtomicBool::new(false)),
        youtube_config,
        facebook_config,
        oauth_states,
        host,
        admin_token,
        cron_scheduler,
        upload_service: upload_service.clone(),
        obs_available_devices: obs_available_devices.clone(),
        presenter_state: presenter_state.clone(),
        use_web_presenter: use_web_presenter.clone(),
        presenter_theme,
        order_position: Arc::new(RwLock::new(None)),
        ws_client_info: ws_client_info.clone(),
        queue_wake: Arc::new(tokio::sync::Notify::new()),
        media: Arc::new(crate::media::MediaRuntime::new(&data_dir)),
        transcription,
        automations,
        recorders: Arc::default(),
        #[cfg(target_os = "macos")]
        keynote_connector: keynote_connector.clone(),
    };

    crate::queue::spawn_worker(state.clone());
    broadcast::spawn_watcher(state.clone());
    crate::automations::spawn(state.clone(), automation_jobs).await;
    crate::recorders::spawn(state.clone()).await;
    crate::media::spawn_worker(state.clone());
    crate::media::spawn_camera_clip_log(state.clone());
    if let Err(e) = crate::transcription::library::fail_interrupted(&state.pool).await {
        tracing::error!("transcripts: cannot close interrupted clean-ups: {e}");
    }
    welcome::prefetch(state.media.tools_dir().to_path_buf());
    crate::transcription::captions_job::spawn_worker(state.clone());
    state.transcription.spawn_watchers();

    {
        let clients = ws_clients.clone();
        let url = connection_url.clone();
        let st = state.clone();
        tokio::spawn(async move {
            websocket::start_notify_listener(url, clients, st).await;
        });
    }

    // Forward OBS status broadcasts to all connected WS clients.
    {
        let clients = ws_clients.clone();
        let mut obs_rx = obs_connector.status_tx.subscribe();
        tokio::spawn(async move {
            while let Ok(status) = obs_rx.recv().await {
                let msg = json!({
                    "type": "connector.status",
                    "connector": "obs",
                    "status": status,
                })
                .to_string();
                let guard = clients.read().await;
                for tx in guard.values() {
                    tx.send(Message::Text(msg.clone().into())).log_gone();
                }
            }
        });
    }

    // Forward OBS streaming/recording state changes to all connected WS clients.
    {
        let clients = ws_clients.clone();
        let mut obs_state_rx = obs_connector.output_state_tx.subscribe();
        tokio::spawn(async move {
            while let Ok(state) = obs_state_rx.recv().await {
                let msg = json!({
                    "type": "connector.state",
                    "connector": "obs",
                    "isStreaming": state.is_streaming,
                    "isRecording": state.is_recording,
                })
                .to_string();
                let guard = clients.read().await;
                for tx in guard.values() {
                    tx.send(Message::Text(msg.clone().into())).log_gone();
                }
            }
        });
    }

    // Forward Blackmagic camera status and record/livestream state to WS clients.
    {
        let clients = ws_clients.clone();
        let mut status_rx = blackmagic_camera_connector.status_tx.subscribe();
        tokio::spawn(async move {
            while let Ok(status) = status_rx.recv().await {
                let msg = json!({
                    "type": "connector.status",
                    "connector": "blackmagic-camera",
                    "status": status,
                })
                .to_string();
                let guard = clients.read().await;
                for tx in guard.values() {
                    tx.send(Message::Text(msg.clone().into())).log_gone();
                }
            }
        });
    }

    {
        let clients = ws_clients.clone();
        let mut state_rx = blackmagic_camera_connector.state_tx.subscribe();
        let st = state.clone();
        tokio::spawn(async move {
            while let Ok(camera) = state_rx.recv().await {
                let mut msg = routes::camera_state_json(&st, &camera).await;
                msg["type"] = json!("connector.state");
                msg["connector"] = json!("blackmagic-camera");
                let msg = msg.to_string();
                let guard = clients.read().await;
                for tx in guard.values() {
                    tx.send(Message::Text(msg.clone().into())).log_gone();
                }
            }
        });
    }

    // Middle Control is healthy only after its first parsed feedback frame.
    {
        let clients = ws_clients.clone();
        let mut status_rx = middlecontrol_connector.status_tx.subscribe();
        tokio::spawn(async move {
            while let Ok(status) = status_rx.recv().await {
                let msg = json!({
                    "type": "connector.status",
                    "connector": "middlecontrol",
                    "status": status,
                })
                .to_string();
                let guard = clients.read().await;
                for tx in guard.values() {
                    tx.send(Message::Text(msg.clone().into())).log_gone();
                }
            }
        });
    }

    {
        let clients = ws_clients.clone();
        let mut state_rx = middlecontrol_connector.state_tx.subscribe();
        tokio::spawn(async move {
            while let Ok(state) = state_rx.recv().await {
                let msg = json!({
                    "type": "connector.state",
                    "connector": "middlecontrol",
                    "state": state,
                })
                .to_string();
                let guard = clients.read().await;
                for tx in guard.values() {
                    tx.send(Message::Text(msg.clone().into())).log_gone();
                }
            }
        });
    }

    {
        let clients = ws_clients.clone();
        let mut status_rx = atem_connector.status_tx.subscribe();
        tokio::spawn(async move {
            while let Ok(status) = status_rx.recv().await {
                let msg = json!({
                    "type": "connector.status",
                    "connector": "atem",
                    "status": status,
                })
                .to_string();
                let guard = clients.read().await;
                for tx in guard.values() {
                    tx.send(Message::Text(msg.clone().into())).log_gone();
                }
            }
        });
    }

    {
        let clients = ws_clients.clone();
        let mut state_rx = atem_connector.state_tx.subscribe();
        tokio::spawn(async move {
            while let Ok(state) = state_rx.recv().await {
                let msg = json!({
                    "type": "connector.state",
                    "connector": "atem",
                    "state": state,
                })
                .to_string();
                let guard = clients.read().await;
                for tx in guard.values() {
                    tx.send(Message::Text(msg.clone().into())).log_gone();
                }
            }
        });
    }

    {
        let clients = ws_clients.clone();
        let mut status_rx = rodecaster_connector.status_tx.subscribe();
        tokio::spawn(async move {
            while let Ok(status) = status_rx.recv().await {
                let msg = json!({
                    "type": "connector.status",
                    "connector": "rodecaster",
                    "status": status,
                })
                .to_string();
                let guard = clients.read().await;
                for tx in guard.values() {
                    tx.send(Message::Text(msg.clone().into())).log_gone();
                }
            }
        });
    }

    {
        let clients = ws_clients.clone();
        let mut profile_rx = rodecaster_connector.profile_tx.subscribe();
        tokio::spawn(async move {
            while let Ok(profile) = profile_rx.recv().await {
                let msg = json!({ "type": "rodecaster.profile", "profile": profile }).to_string();
                let guard = clients.read().await;
                for tx in guard.values() {
                    tx.send(Message::Text(msg.clone().into())).log_gone();
                }
            }
        });
    }

    {
        let clients = ws_clients.clone();
        let mut mute_rx = rodecaster_connector.mute_tx.subscribe();
        tokio::spawn(async move {
            while let Ok(event) = mute_rx.recv().await {
                let msg = json!({
                    "type": "rodecaster.mute",
                    "channel": event.channel,
                    "label": event.label,
                    "muted": event.muted,
                    "remote": event.remote,
                    "notify": event.notify,
                })
                .to_string();
                let guard = clients.read().await;
                for tx in guard.values() {
                    tx.send(Message::Text(msg.clone().into())).log_gone();
                }
            }
        });
    }

    {
        let clients = ws_clients.clone();
        let mut recorder_rx = rodecaster_audio_recorder.state_tx.subscribe();
        tokio::spawn(async move {
            while let Ok(state) = recorder_rx.recv().await {
                let msg = json!({
                    "type": "rodecaster.audio.record.state",
                    "state": state,
                })
                .to_string();
                let guard = clients.read().await;
                for tx in guard.values() {
                    tx.send(Message::Text(msg.clone().into())).log_gone();
                }
            }
        });
    }

    // The desk's own SD or USB recorder, which is not Metocast's audio recording above.
    {
        let clients = ws_clients.clone();
        let mut recorder_rx = rodecaster_connector.recorder_tx.subscribe();
        tokio::spawn(async move {
            while let Ok(recorder) = recorder_rx.recv().await {
                let msg = json!({
                    "type": "rodecaster.record.state",
                    "state": crate::connectors::rodecaster::desk_recorder(recorder),
                })
                .to_string();
                let guard = clients.read().await;
                for tx in guard.values() {
                    tx.send(Message::Text(msg.clone().into())).log_gone();
                }
            }
        });
    }

    // Forward YouTube status broadcasts to all connected WS clients.
    {
        let clients = ws_clients.clone();
        let mut yt_rx = youtube_connector.status_tx.subscribe();
        tokio::spawn(async move {
            while let Ok(status) = yt_rx.recv().await {
                let msg = json!({
                    "type": "connector.status",
                    "connector": "youtube",
                    "status": status,
                })
                .to_string();
                let guard = clients.read().await;
                for tx in guard.values() {
                    tx.send(Message::Text(msg.clone().into())).log_gone();
                }
            }
        });
    }

    // Forward Facebook status broadcasts to all connected WS clients.
    {
        let clients = ws_clients.clone();
        let mut fb_rx = facebook_connector.status_tx.subscribe();
        tokio::spawn(async move {
            while let Ok(status) = fb_rx.recv().await {
                let msg = json!({
                    "type": "connector.status",
                    "connector": "facebook",
                    "status": status,
                })
                .to_string();
                let guard = clients.read().await;
                for tx in guard.values() {
                    tx.send(Message::Text(msg.clone().into())).log_gone();
                }
            }
        });
    }

    // Forward Broadlink status broadcasts to all connected WS clients.
    {
        let clients = ws_clients.clone();
        let mut bl_rx = broadlink_connector.status_tx.subscribe();
        tokio::spawn(async move {
            while let Ok(status) = bl_rx.recv().await {
                let msg = json!({
                    "type": "connector.status",
                    "connector": "broadlink",
                    "status": status,
                })
                .to_string();
                let guard = clients.read().await;
                for tx in guard.values() {
                    tx.send(Message::Text(msg.clone().into())).log_gone();
                }
            }
        });
    }

    // Forward Broadlink learn results to all connected WS clients.
    {
        let clients = ws_clients.clone();
        let mut learn_rx = broadlink_connector.learn_tx.subscribe();
        tokio::spawn(async move {
            while let Ok(ev) = learn_rx.recv().await {
                let msg = json!({
                    "type": "broadlink.learn.result",
                    "code": ev.code,
                    "error": ev.error,
                })
                .to_string();
                let guard = clients.read().await;
                for tx in guard.values() {
                    tx.send(Message::Text(msg.clone().into())).log_gone();
                }
            }
        });
    }

    // Forward Keynote status broadcasts to all connected WS clients (macOS only).
    #[cfg(target_os = "macos")]
    {
        let clients = ws_clients.clone();
        let mut kn_rx = keynote_connector.status_tx.subscribe();
        tokio::spawn(async move {
            while let Ok(status) = kn_rx.recv().await {
                let msg = json!({
                    "type": "keynote.status",
                    "status": status,
                })
                .to_string();
                let guard = clients.read().await;
                for tx in guard.values() {
                    tx.send(Message::Text(msg.clone().into())).log_gone();
                }
            }
        });
    }

    // Start Keynote adaptive polling loop (macOS only).
    #[cfg(target_os = "macos")]
    Arc::clone(&keynote_connector).start_polling();

    // Forward OBS streaming/recording state to all connected WS clients.
    {
        let clients = ws_clients.clone();
        let mut obs_state_rx = obs_connector.state_tx.subscribe();
        tokio::spawn(async move {
            while let Ok(ev) = obs_state_rx.recv().await {
                let msg = json!({
                    "type": "obs.state",
                    "isStreaming": ev.is_streaming,
                    "isRecording": ev.is_recording,
                })
                .to_string();
                let guard = clients.read().await;
                for tx in guard.values() {
                    tx.send(Message::Text(msg.clone().into())).log_gone();
                }
            }
        });
    }

    // Detect OBS recordings and auto-assign to the current event.
    {
        let pool_c = state.pool.clone();
        let clients_c = ws_clients.clone();
        let mut recording_rx = obs_connector.recording_tx.subscribe();
        tokio::spawn(async move {
            handle_obs_recording_events(pool_c, clients_c, &mut recording_rx).await;
        });
    }

    // Rescan OBS devices whenever devices_tx fires (input added/removed/changed or initial connect).
    {
        let pool_c = state.pool.clone();
        let obs_c = obs_connector.clone();
        let devices_cache = obs_available_devices.clone();
        let clients_c = ws_clients.clone();
        let mut devices_rx = obs_connector.devices_tx.subscribe();
        tokio::spawn(async move {
            loop {
                match devices_rx.recv().await {
                    Ok(()) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
                let client_opt = obs_c.client.lock().await.clone();
                let Some(client) = client_opt else { continue };
                let scanned = crate::obs_devices::scan_obs_devices(&client).await;
                let listeners: Vec<crate::models::device_listener::DeviceListener> =
                    sqlx::query_as("SELECT * FROM device_listeners ORDER BY created_at")
                        .fetch_all(&pool_c)
                        .await
                        .unwrap_or_default();
                let statuses = crate::obs_devices::compute_listener_statuses(&scanned, &listeners);
                *devices_cache.write().await = Some(scanned.clone());
                let msg = serde_json::json!({
                    "type": "obs.devices.available",
                    "devices": scanned,
                    "listenerStatuses": statuses,
                })
                .to_string();
                let guard = clients_c.read().await;
                for tx in guard.values() {
                    tx.send(axum::extract::ws::Message::Text(msg.clone().into()))
                        .log_gone();
                }
            }
        });
    }

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // OAuth URL, callback submission and logout routes
    let oauth_routes = Router::new()
        .route("/auth/youtube/url", get(routes::youtube_auth_url))
        .route("/auth/youtube/logout", post(routes::youtube_logout))
        .route("/auth/facebook/url", get(routes::facebook_auth_url))
        .route("/auth/facebook/logout", post(routes::facebook_logout))
        .route("/auth/callback", post(routes::submit_oauth_callback));

    // PPT folder and file search routes (all platforms).
    let ppt_routes = Router::new()
        .route("/ppt/folders", get(ppt::list_folders).post(ppt::add_folder))
        .route("/ppt/folders/{id}", delete(ppt::remove_folder))
        .route("/ppt/files", get(ppt::search_files))
        .route("/ppt/song", post(routes::create_song_slides));

    // Keynote control routes (macOS only; 501 stub on other platforms).
    #[cfg(target_os = "macos")]
    let keynote_routes = Router::new()
        .route("/keynote/status", get(ppt::keynote_status))
        .route("/keynote/open", post(ppt::keynote_open))
        .route("/keynote/next", post(ppt::keynote_next))
        .route("/keynote/prev", post(ppt::keynote_prev))
        .route("/keynote/first", post(ppt::keynote_first))
        .route("/keynote/last", post(ppt::keynote_last))
        .route("/keynote/goto", post(ppt::keynote_goto))
        .route("/keynote/start", post(ppt::keynote_start))
        .route("/keynote/stop", post(ppt::keynote_stop))
        .route("/keynote/close_all", post(ppt::keynote_close_all));

    #[cfg(not(target_os = "macos"))]
    let keynote_routes = Router::new()
        .route("/keynote/status", get(ppt::keynote_not_implemented))
        .route("/keynote/open", post(ppt::keynote_not_implemented))
        .route("/keynote/next", post(ppt::keynote_not_implemented))
        .route("/keynote/prev", post(ppt::keynote_not_implemented))
        .route("/keynote/first", post(ppt::keynote_not_implemented))
        .route("/keynote/last", post(ppt::keynote_not_implemented))
        .route("/keynote/goto", post(ppt::keynote_not_implemented))
        .route("/keynote/start", post(ppt::keynote_not_implemented))
        .route("/keynote/stop", post(ppt::keynote_not_implemented))
        .route("/keynote/close_all", post(ppt::keynote_not_implemented));

    let api_routes = Router::new()
        .route(
            "/events",
            get(routes::list_events).post(routes::create_event),
        )
        .route(
            "/settings/title-template",
            get(routes::get_title_template).put(routes::set_title_template),
        )
        .route(
            "/settings/record-button",
            get(routes::get_record_button).put(routes::set_record_button),
        )
        .route("/recorders", get(routes::list_recorders))
        .route(
            "/settings/slide-folder",
            get(routes::get_slide_folder).put(routes::set_slide_folder),
        )
        .route(
            "/settings/song-slide-folder",
            get(routes::get_song_slide_folder).put(routes::set_song_slide_folder),
        )
        .route("/events/{id}/slides", post(routes::create_event_slides))
        .route(
            "/events/{id}/order",
            get(order::get_order).put(order::put_order),
        )
        .route(
            "/events/{id}",
            get(routes::get_event)
                .put(routes::update_event)
                .delete(routes::delete_event),
        )
        .route(
            "/events/{id}/recordings",
            get(routes::list_recordings).post(routes::create_recording),
        )
        .route(
            "/events/{id}/recordings/{recording_id}",
            delete(routes::delete_recording),
        )
        .route(
            "/events/{id}/activities",
            get(routes::list_event_activities).post(routes::create_event_activity),
        )
        .route(
            "/events/{id}/activities/{activity_id}",
            delete(routes::delete_event_activity),
        )
        .route("/recordings", get(routes::list_all_recordings))
        .route(
            "/recordings/untracked",
            get(routes::list_untracked_recordings),
        )
        .route(
            "/recordings/untracked/{id}/assign",
            post(routes::assign_untracked_recording),
        )
        .route(
            "/recordings/untracked/{id}",
            delete(routes::delete_untracked_recording),
        )
        .route(
            "/connectors/broadlink/status",
            get(routes::broadlink_get_status),
        )
        .route(
            "/connectors/broadlink/devices",
            get(routes::broadlink_list_devices).post(routes::broadlink_add_device),
        )
        .route(
            "/connectors/broadlink/devices/{id}",
            delete(routes::broadlink_remove_device),
        )
        .route(
            "/connectors/blackmagic-camera/stream/youtube",
            post(routes::blackmagic_camera_push_youtube),
        )
        .route(
            "/connectors/blackmagic-camera/settings",
            get(routes::blackmagic_camera_settings).put(routes::blackmagic_camera_apply_settings),
        )
        .route(
            "/connectors/blackmagic-camera/discover",
            post(routes::blackmagic_camera_discover),
        )
        .route(
            "/connectors/middlecontrol/discover",
            post(routes::middlecontrol_discover),
        )
        .route(
            "/connectors/atem/stream/youtube",
            post(routes::atem_push_youtube),
        )
        .route("/connectors/atem/discover", post(routes::atem_discover))
        .route(
            "/connectors/broadlink/discover",
            post(routes::broadlink_discover),
        )
        .route(
            "/connectors/broadlink/commands",
            get(routes::broadlink_list_commands).post(routes::broadlink_add_command),
        )
        .route(
            "/connectors/broadlink/commands/{id}",
            put(routes::broadlink_update_command).delete(routes::broadlink_remove_command),
        )
        .route(
            "/connectors/broadlink/devices/{id}/learn",
            post(routes::broadlink_start_learn),
        )
        .route(
            "/connectors/broadlink/learn/cancel",
            post(routes::broadlink_cancel_learn),
        )
        .route(
            "/connectors/broadlink/commands/{id}/send",
            post(routes::broadlink_send_command),
        )
        .route("/bible/verses", get(routes::get_bible_passage))
        .route("/bible/suggest", get(routes::get_bible_suggestions))
        .route("/connectors/state", get(routes::get_connector_state))
        .route("/connectors/status", get(routes::get_connector_statuses))
        .route(
            "/connectors/{name}/config",
            get(routes::get_connector_config).put(routes::put_connector_config),
        )
        .route(
            "/connectors/{name}/config/secrets",
            get(routes::reveal_connector_secrets),
        )
        .route(
            "/connectors/blackmagic-camera/connect",
            post(routes::blackmagic_camera_connect),
        )
        .route("/connectors/obs/connect", post(routes::obs_connect))
        .route("/connectors/obs/disconnect", post(routes::obs_disconnect))
        .route(
            "/connectors/obs/stream-settings",
            get(routes::get_obs_stream_settings).put(routes::set_obs_stream_settings),
        )
        .route(
            "/connectors/youtube/content",
            get(routes::get_youtube_content),
        )
        .route(
            "/connectors/youtube/stream-key",
            get(routes::get_youtube_stream_key),
        )
        .route(
            "/connectors/facebook/stream-key",
            get(routes::get_facebook_stream_key),
        )
        .route(
            "/events/{id}/stream/{platform}/start",
            post(routes::start_event_stream),
        )
        .route(
            "/events/{id}/stream/{platform}/stop",
            post(routes::stop_event_stream),
        )
        .route(
            "/connectors/youtube/schedule/{event_id}",
            post(routes::trigger_youtube_schedule),
        )
        .route(
            "/connectors/facebook/schedule/{event_id}",
            post(routes::trigger_facebook_schedule),
        )
        .route(
            "/cron-jobs",
            get(routes::list_cron_jobs).post(routes::create_cron_job),
        )
        .route(
            "/cron-jobs/{id}",
            put(routes::update_cron_job).delete(routes::delete_cron_job),
        )
        .route(
            "/events/{id}/recordings/flag-upload",
            post(routes::flag_upload),
        )
        .route(
            "/logs",
            get(routes::get_application_log).delete(routes::clear_application_log),
        )
        .route("/uploads/trigger", post(routes::trigger_upload_cycle))
        .route("/queues", get(crate::queue::list_queues))
        .route("/queues/{queue}/jobs", get(crate::queue::list_jobs))
        .route("/jobs/{id}/retry", post(crate::queue::retry_job))
        .route("/jobs/{id}", delete(crate::queue::purge_job))
        .route("/media/probe", post(crate::media::probe_media))
        .route("/media/preview", post(crate::media::preview_media))
        .route("/media/renders", post(crate::media::start_render))
        .route(
            "/media/renders/{id}/cancel",
            post(crate::media::cancel_render),
        )
        .route("/media/audio/probe", post(crate::media::probe_audio))
        .route("/media/audio", post(crate::media::start_audio))
        .route("/media/programs/probe", post(crate::media::probe_program))
        .route("/media/programs", post(crate::media::start_program))
        .route(
            "/media/programs/{id}/nudge",
            post(crate::media::nudge_program),
        )
        .route(
            "/media/programs/{id}/subtitles",
            post(crate::media::add_program_subtitles),
        )
        .route(
            "/automations",
            get(crate::automations::api::list).post(crate::automations::api::create),
        )
        .route("/automations/runs", get(crate::automations::api::runs))
        .route(
            "/automations/armed",
            put(crate::automations::api::set_armed),
        )
        .route(
            "/automations/{id}",
            put(crate::automations::api::replace).delete(crate::automations::api::delete),
        )
        .route("/automations/{id}/run", post(crate::automations::api::run))
        .route("/openai/verify", post(routes::openai_verify))
        .route(
            "/recordings/{id}/captions",
            post(crate::transcription::captions_job::enqueue_route),
        )
        .route(
            "/recordings/{id}/captions.vtt",
            get(crate::transcription::captions_job::vtt_route),
        )
        .route(
            "/transcripts",
            get(crate::transcription::library::list_route),
        )
        .route(
            "/transcripts/{id}",
            get(crate::transcription::library::get_route)
                .delete(crate::transcription::library::delete_route),
        )
        .route(
            "/transcripts/{id}/download",
            get(crate::transcription::library::download_route),
        )
        .route(
            "/transcripts/{id}/cleanup",
            post(crate::transcription::library::cleanup_route),
        )
        .merge(ppt_routes)
        .merge(keynote_routes)
        .route("/presenter/parse", post(presenter::parse_presentation))
        .route(
            "/presenter/welcome",
            get(welcome::get_settings).put(welcome::put_settings),
        )
        // Phone photos run to tens of megabytes; the server shrinks them to 1920×1080.
        .route(
            "/presenter/welcome/photos/{number}",
            get(welcome::get_photo)
                .put(welcome::put_photo)
                .delete(welcome::delete_photo)
                .layer(axum::extract::DefaultBodyLimit::max(40 * 1024 * 1024)),
        )
        .route(
            "/presenter/welcome/preview/{number}",
            get(welcome::get_preview),
        )
        .route(
            "/presenter/welcome/logo",
            get(welcome::get_logo)
                .put(welcome::put_logo)
                .delete(welcome::delete_logo)
                .layer(axum::extract::DefaultBodyLimit::max(5 * 1024 * 1024)),
        )
        // OAuth routes are UI-initiated (generate a login URL, log out), so they
        // sit inside the auth layer. Merging them here also puts them under `/api`
        // with everything else in this router: a client asks for
        // `/api/auth/youtube/url`, not `/auth/youtube/url`. Only the browser redirect
        // target itself (`/callback`, on the fixed OAuth port below) is reachable
        // unauthenticated.
        .merge(oauth_routes)
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::auth_middleware,
        ))
        // With METOCAST_REDACT=1, what identifies the machine is replaced on the way out.
        .layer(middleware::from_fn(redact::middleware));

    // CorsLayer must be the outermost layer so it intercepts OPTIONS preflight
    // requests before they reach the auth middleware. tower-http's CorsLayer
    // only adds response headers and never modifies request headers, so it is
    // safe to apply to all routes including /ws.
    // The MCP server's REST tools call this copy in-process, so they pass the same auth layer.
    let api_for_mcp = Router::new()
        .nest("/api", api_routes.clone())
        .with_state(state.clone());
    let mut app = Router::new()
        .route("/health", get(|| async { axum::http::StatusCode::OK }))
        .route("/caption", get(caption::caption_handler))
        .route("/caption/logo", get(caption::caption_logo_handler))
        .route("/openapi.json", get(openapi::serve_spec))
        .route("/docs", get(openapi::serve_docs))
        .route("/ws", get(websocket::ws_handler))
        .nest("/api", api_routes)
        .with_state(state.clone())
        .merge(mcp::router(state.clone(), api_for_mcp, shutdown.clone()));

    if let Some(dir) = static_dir {
        // Without this the SPA fallback answers, losing the route.
        app = app
            .route_service(
                "/presenter",
                ServeFile::new(format!("{dir}/presenter.html")),
            )
            .route_service(
                "/transcript",
                ServeFile::new(format!("{dir}/transcript.html")),
            );
        let fallback = ServeFile::new(format!("{dir}/index.html"));
        app = app.fallback_service(ServeDir::new(&dir).fallback(fallback));
    } else if let Some(host) = state.host.clone() {
        app = app.fallback(move |uri: axum::http::Uri| {
            let host = host.clone();
            async move { serve_embedded_asset(&host, uri.path()) }
        });
    }

    let app = app.layer(cors);

    // Dedicated OAuth callback listener on the fixed port 8766.
    // This keeps the redirect URI stable (matching the Cloud Console config)
    // even if the main API port is changed.
    let callback_addr = SocketAddr::from(([127, 0, 0, 1], OAUTH_CALLBACK_PORT));
    match TcpListener::bind(callback_addr).await {
        Ok(cb_listener) => {
            let cb_app = Router::new()
                .route("/callback", get(routes::oauth_callback))
                .with_state(state.clone());
            tracing::info!("OAuth callback server listening on {callback_addr}");
            let callback_shutdown = shutdown.clone();
            tokio::spawn(async move {
                let server = axum::serve(cb_listener, cb_app);
                if let Some(mut shutdown) = callback_shutdown {
                    server
                        .with_graceful_shutdown(async move {
                            while !*shutdown.borrow() && shutdown.changed().await.is_ok() {}
                        })
                        .await
                        .log_failed();
                } else {
                    server.await.log_failed();
                }
            });
        }
        Err(e) => {
            tracing::warn!("Could not bind OAuth callback port {OAUTH_CALLBACK_PORT}: {e}");
        }
    }

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = match TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(error) => {
            if let Some(ready) = ready {
                ready.send(Err(error.to_string())).log_gone();
            }
            return Err(error.into());
        }
    };
    let bound_address = listener.local_addr()?;
    if let Some(ready) = ready {
        ready.send(Ok(bound_address)).log_gone();
    }
    tracing::info!("Axum server listening on {bound_address}");
    // ConnectInfo lets the secret-reveal route verify the caller is on loopback.
    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    );
    if let Some(mut shutdown) = shutdown {
        server
            .with_graceful_shutdown(async move {
                while !*shutdown.borrow() && shutdown.changed().await.is_ok() {}
            })
            .await?;
    } else {
        server.await?;
    }

    Ok(())
}

/// Probe the video file duration via `ffprobe`. Returns 0.0 if unavailable.
async fn probe_duration(path: &std::path::Path) -> f64 {
    let out = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-of",
            "json",
            "-show_entries",
            "format=duration",
            "-i",
        ])
        .arg(path)
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout);
            serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| {
                    v["format"]["duration"]
                        .as_str()
                        .and_then(|s| s.parse::<f64>().ok())
                })
                .unwrap_or(0.0)
        }
        _ => 0.0,
    }
}

/// Receives OBS recording-stopped events and inserts the file into `recordings`
/// (if a current event exists) or `untracked_recordings`.
async fn handle_obs_recording_events(
    pool: PgPool,
    clients: Arc<RwLock<HashMap<Uuid, crate::server::ws_client::ClientTx>>>,
    recording_rx: &mut tokio::sync::broadcast::Receiver<crate::connectors::obs::ObsRecordingEvent>,
) {
    loop {
        let event = match recording_rx.recv().await {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };

        let file_path = event.output_path.to_string_lossy().to_string();
        let file_name = event
            .output_path
            .file_name()
            .map(|n: &std::ffi::OsStr| n.to_string_lossy().to_string())
            .unwrap_or_else(|| file_path.clone());

        let file_size: i64 = tokio::fs::metadata(&event.output_path)
            .await
            .map(|m| m.len() as i64)
            .unwrap_or(0);

        let duration_seconds = probe_duration(&event.output_path).await;

        match find_current_event(&pool).await {
            Ok(Some(ev)) => {
                let insert_result = sqlx::query(
                    "INSERT INTO recordings (event_id, file_path, file_name, file_size, duration_seconds, detected_at) \
                     VALUES ($1, $2, $3, $4, $5, NOW())",
                )
                .bind(ev.id)
                .bind(&file_path)
                .bind(&file_name)
                .bind(file_size)
                .bind(duration_seconds)
                .execute(&pool)
                .await;

                if let Err(e) = insert_result {
                    tracing::error!("handle_obs_recording_events: insert recording: {e}");
                    continue;
                }

                websocket::broadcast_recording_detected(&clients, &file_name, Some(&ev.title))
                    .await;
            }
            Ok(None) => {
                let insert_result = sqlx::query(
                    "INSERT INTO untracked_recordings (file_path, file_name, file_size, duration_seconds, detected_at) \
                     VALUES ($1, $2, $3, $4, NOW())",
                )
                .bind(&file_path)
                .bind(&file_name)
                .bind(file_size)
                .bind(duration_seconds)
                .execute(&pool)
                .await;

                if let Err(e) = insert_result {
                    tracing::error!("handle_obs_recording_events: insert untracked: {e}");
                    continue;
                }

                websocket::broadcast_recording_detected(&clients, &file_name, None).await;
            }
            Err(e) => {
                tracing::error!("handle_obs_recording_events: find_current_event: {e}");
            }
        }
    }
}

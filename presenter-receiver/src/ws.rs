use crate::ConnectionState;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};

// ── Message types from the server ────────────────────────────────────────────

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ParagraphContent {
    /// New format: explicit lines from the PPTX parser.
    #[serde(default)]
    pub lines: Vec<String>,
    /// Legacy format (old binaries): single text string.
    pub text: Option<String>,
    pub align: String,
    #[serde(default)]
    pub font_size_pt: f64,
}

impl ParagraphContent {
    pub fn display_text(&self) -> String {
        if !self.lines.is_empty() {
            self.lines.join("\n")
        } else {
            self.text.clone().unwrap_or_default()
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SlideContent {
    pub index: u32,
    pub paragraphs: Vec<ParagraphContent>,
}

#[derive(Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PresenterRenderMode {
    #[default]
    Text,
    Svg,
}

#[derive(Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PresenterTheme {
    #[default]
    Classic,
    Editorial,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SvgSlideContent {
    pub index: u32,
    pub svg: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PresenterState {
    pub current_slide: u32,
    #[serde(default)]
    pub render_mode: PresenterRenderMode,
    #[serde(default)]
    pub slides: Vec<SlideContent>,
    #[serde(default)]
    pub svg_slides: Vec<SvgSlideContent>,
    #[serde(default)]
    pub muted: bool,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum ServerMsg {
    #[serde(rename = "presentation.settings")]
    PresentationSettings {
        #[serde(rename = "presenterTheme", default)]
        presenter_theme: PresenterTheme,
    },
    #[serde(rename = "presenter.state")]
    PresenterState {
        state: PresenterState,
        /// True when the server left each slide's SVG out (`/ws?presenter=frames`); a server
        /// older than that never sends it and always includes them.
        #[serde(rename = "svgOmitted", default)]
        svg_omitted: bool,
    },
    #[serde(rename = "presenter.slide_changed")]
    SlideChanged {
        #[serde(rename = "currentSlide")]
        current_slide: u32,
    },
    #[serde(rename = "ping")]
    Ping {
        #[serde(rename = "pingId")]
        ping_id: i64,
    },
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(other)]
    Unknown,
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Connect to the presenter WebSocket, render slides, and send pixel frames
/// down `tx` whenever the current slide changes. Reconnects automatically.
pub async fn run(
    url: String,
    tx: std::sync::mpsc::Sender<Frame>,
    dims: DisplayDims,
    conn_state: Arc<Mutex<ConnectionState>>,
) {
    let mut fail_count = 0u32;
    loop {
        *conn_state.lock().unwrap() = ConnectionState::Connecting;
        eprintln!("[ws] Connecting to {url}...");
        match connect_and_receive(&url, &tx, dims, Arc::clone(&conn_state)).await {
            Ok(()) => {
                eprintln!("[ws] Connection closed.");
                fail_count = 0;
            }
            Err(e) => {
                eprintln!("[ws] Error: {e}");
                fail_count += 1;
            }
        }
        *conn_state.lock().unwrap() = if fail_count >= 5 {
            ConnectionState::Failed
        } else {
            ConnectionState::Reconnecting
        };
        eprintln!("[ws] Reconnecting in 3 s...");
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

// ── Frame type sent over the channel ─────────────────────────────────────────

/// Rendered pixel frame: u32 values in 0x00RRGGBB order (minifb / fb compatible).
pub type Frame = Vec<u32>;

/// Display dimensions needed by the renderer.
#[derive(Clone, Copy)]
pub struct DisplayDims {
    pub width: u32,
    pub height: u32,
}

// ── Internal connection loop ──────────────────────────────────────────────────

/// Matches `MAX_WS_MESSAGE_BYTES` in metocast-core: a deck with photos arrives as one
/// `presenter.state` message larger than tungstenite's default 16 MiB frame limit.
const MAX_WS_MESSAGE_BYTES: usize = 64 << 20;

/// How long to wait for a `presenter.frame` before drawing the slide here instead. The server
/// is asked again for the next slide: a slow reply is a busy link, not a server that can't draw.
const FRAME_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// How long the connection may stay silent; the server pings every 30 s.
const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

type Sink = std::pin::Pin<
    Box<dyn futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Send>,
>;
type WsResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// SVG slides are drawn by the server (`presenter.frame`): resvg takes seconds to draw a photo
/// on a Raspberry Pi, decoding the server's JPEG a fraction of that. Replies come in order, so
/// the last one answers the latest request.
#[derive(Debug)]
struct FrameRequests {
    /// Asked for and not yet answered, including requests that timed out: their replies still
    /// come, in order, and must not be taken for a later request's.
    pending: u32,
    /// The slide the latest request was for.
    slide: Option<u32>,
    sent_at: std::time::Instant,
    /// The latest request took longer than `FRAME_TIMEOUT` and its slide was drawn here.
    timed_out: bool,
    /// False once the server turns out not to draw frames (`unauthorized`: a server older than
    /// `presenter.frame`). A timeout never sets it.
    supported: bool,
}

impl FrameRequests {
    fn new() -> Self {
        Self {
            pending: 0,
            slide: None,
            sent_at: std::time::Instant::now(),
            timed_out: false,
            supported: true,
        }
    }

    /// True when `slide` should be asked of the server: it draws frames, and the latest
    /// request was not for this slide (that one is on its way, or timed out and was drawn here).
    fn should_ask(&self, slide: u32) -> bool {
        self.supported && self.slide != Some(slide)
    }

    fn asked(&mut self, slide: u32) {
        self.pending += 1;
        self.slide = Some(slide);
        self.sent_at = std::time::Instant::now();
        self.timed_out = false;
    }

    /// How long to wait for the next message before giving up on the latest request.
    fn wait(&self) -> std::time::Duration {
        if self.pending > 0 && !self.timed_out {
            FRAME_TIMEOUT.saturating_sub(self.sent_at.elapsed())
        } else {
            IDLE_TIMEOUT
        }
    }

    /// True while a request is waiting for its reply within `FRAME_TIMEOUT`.
    fn waiting(&self) -> bool {
        self.pending > 0 && !self.timed_out
    }

    /// The latest request took too long: its slide is drawn here, and the next slide is asked
    /// of the server again.
    fn time_out(&mut self) {
        self.timed_out = true;
    }

    /// Counts a reply in; true when it answers the latest request.
    fn answered(&mut self) -> bool {
        if self.pending == 0 {
            return false;
        }
        self.pending -= 1;
        if self.pending == 0 {
            self.timed_out = false;
            true
        } else {
            false
        }
    }

    /// Counts an `error` reply in; `unauthorized` means the server can't draw frames at all.
    /// True when it answers the latest request.
    fn refused(&mut self, message: &str) -> bool {
        self.supported &= message != "unauthorized";
        self.answered()
    }
}

/// What showing the current slide needs next.
#[derive(Debug, PartialEq, Eq)]
enum Shown {
    /// It is on the screen.
    Done,
    /// An SVG slide not drawn yet: ask the server for it. The screen keeps what it shows.
    AskServer,
    /// An SVG slide that has to be drawn here, whose SVG the server left out: ask for it.
    NeedSvg,
}

/// The connection's view of the presentation and what it has drawn.
struct Screen<'a> {
    state: PresenterState,
    /// The server left the SVGs out of `state`.
    svg_omitted: bool,
    theme: PresenterTheme,
    cache: HashMap<u32, Frame>,
    frames: FrameRequests,
    /// The slide whose SVGs were asked for to draw it here.
    svg_for: Option<u32>,
    tx: &'a std::sync::mpsc::Sender<Frame>,
    dims: DisplayDims,
}

fn websocket_config() -> WebSocketConfig {
    WebSocketConfig {
        max_message_size: Some(MAX_WS_MESSAGE_BYTES),
        max_frame_size: Some(MAX_WS_MESSAGE_BYTES),
        ..WebSocketConfig::default()
    }
}

async fn connect_and_receive(
    url: &str,
    tx: &std::sync::mpsc::Sender<Frame>,
    dims: DisplayDims,
    conn_state: Arc<Mutex<ConnectionState>>,
) -> WsResult<()> {
    let (ws_stream, _) = connect_async_with_config(url, Some(websocket_config()), false).await?;
    *conn_state.lock().unwrap() = ConnectionState::Connected;
    eprintln!("[ws] Connected.");

    let (write, mut read) = ws_stream.split();
    let mut write: Sink = Box::pin(write);

    // Announce ourselves and ask for current state immediately
    let hostname = std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    write
        .send(Message::Text(
            json!({"type": "presenter.register", "label": "Presenter Receiver", "hostname": hostname})
                .to_string(),
        ))
        .await?;
    write
        .send(Message::Text(
            json!({"type": "presenter.status"}).to_string(),
        ))
        .await?;

    let mut screen = Screen {
        state: PresenterState {
            current_slide: 0,
            render_mode: PresenterRenderMode::Text,
            slides: Vec::new(),
            svg_slides: Vec::new(),
            muted: false,
        },
        svg_omitted: false,
        theme: PresenterTheme::Classic,
        cache: HashMap::new(),
        frames: FrameRequests::new(),
        svg_for: None,
        tx,
        dims,
    };

    loop {
        let raw = match tokio::time::timeout(screen.frames.wait(), read.next()).await {
            Ok(Some(msg)) => msg?,
            Ok(None) => break, // stream ended cleanly
            Err(_) if screen.frames.waiting() => {
                eprintln!("[frame] no reply from the server in time; drawing this slide here");
                screen.frames.time_out();
                screen.draw_here(&mut write).await?;
                continue;
            }
            Err(_) => return Err("read timeout — connection lost".into()),
        };
        let text = match raw {
            Message::Text(t) => t,
            Message::Binary(jpeg) => {
                screen.frame_arrived(&jpeg, &mut write).await?;
                continue;
            }
            Message::Close(_) => break,
            _ => continue,
        };

        let msg: ServerMsg = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(_) => continue,
        };

        match msg {
            ServerMsg::PresentationSettings {
                presenter_theme: next_theme,
            } => {
                screen.theme = next_theme;
                screen.show(&mut write).await?;
            }
            ServerMsg::PresenterState { state, svg_omitted } => {
                log_presenter_state("state", &state, svg_omitted);
                let svg_for = screen.svg_for.take();
                screen.state = state;
                screen.svg_omitted = svg_omitted;
                screen.cache.clear();
                if svg_for == Some(screen.state.current_slide) && !svg_omitted {
                    // The SVGs asked for to draw this slide here.
                    screen.draw_here(&mut write).await?;
                } else {
                    screen.frames.slide = None;
                    screen.show(&mut write).await?;
                }
            }
            ServerMsg::SlideChanged {
                current_slide: new_slide,
            } => {
                screen.state.current_slide = new_slide;
                eprintln!(
                    "[presenter] slide changed: current={} mode={:?} textSlides={} svgSlides={}",
                    screen.state.current_slide,
                    screen.state.render_mode,
                    screen.state.slides.len(),
                    screen.state.svg_slides.len()
                );
                screen.show(&mut write).await?;
            }
            ServerMsg::Ping { ping_id } => {
                write
                    .send(Message::Text(
                        json!({"type": "pong", "ping_id": ping_id}).to_string(),
                    ))
                    .await?;
            }
            ServerMsg::Error { message } => {
                eprintln!("[ws] server error: {message}");
                if screen.frames.refused(&message) {
                    screen.frames.slide = None;
                    screen.draw_here(&mut write).await?;
                }
            }
            ServerMsg::Unknown => {}
        }
    }

    Ok(())
}

fn log_presenter_state(source: &str, state: &PresenterState, svg_omitted: bool) {
    eprintln!(
        "[presenter] {source}: current={} mode={:?} muted={} textSlides={} svgSlides={} svgOmitted={svg_omitted}",
        state.current_slide,
        state.render_mode,
        state.muted,
        state.slides.len(),
        state.svg_slides.len()
    );
}

// ── Render + send ─────────────────────────────────────────────────────────────

impl Screen<'_> {
    /// Shows the current slide, asking the server to draw it when it is an SVG slide not drawn
    /// yet.
    async fn show(&mut self, write: &mut Sink) -> WsResult<()> {
        let slide = self.state.current_slide;
        let ask_server = self.frames.should_ask(slide);
        match self.render(ask_server)? {
            Shown::Done => {}
            Shown::AskServer => {
                write
                    .send(Message::Text(
                        json!({
                            "type": "presenter.frame",
                            "slide": slide,
                            "width": self.dims.width,
                            "height": self.dims.height,
                        })
                        .to_string(),
                    ))
                    .await?;
                self.frames.asked(slide);
            }
            Shown::NeedSvg => self.ask_for_svg(write).await?,
        }
        Ok(())
    }

    /// Draws the current slide here, this once, asking for the SVGs first when the server left
    /// them out.
    async fn draw_here(&mut self, write: &mut Sink) -> WsResult<()> {
        if self.render(false)? == Shown::NeedSvg {
            self.ask_for_svg(write).await?;
        }
        Ok(())
    }

    async fn ask_for_svg(&mut self, write: &mut Sink) -> WsResult<()> {
        eprintln!(
            "[frame] asking for the SVGs to draw slide {} here",
            self.state.current_slide
        );
        write
            .send(Message::Text(
                json!({"type": "presenter.status", "svg": true}).to_string(),
            ))
            .await?;
        self.svg_for = Some(self.state.current_slide);
        Ok(())
    }

    /// A `presenter.frame` reply: shows it when it answers the latest request.
    async fn frame_arrived(&mut self, jpeg: &[u8], write: &mut Sink) -> WsResult<()> {
        if !self.frames.answered() {
            return Ok(());
        }
        let Some(slide) = self.frames.slide.take() else {
            return Ok(());
        };
        let started = std::time::Instant::now();
        match crate::renderer::decode_frame(jpeg, self.dims.width, self.dims.height) {
            Ok(frame) => {
                eprintln!(
                    "[frame] slide {slide}: {} bytes {:?} after asking, decoded in {:?}",
                    jpeg.len(),
                    self.frames.sent_at.elapsed(),
                    started.elapsed()
                );
                self.cache.insert(slide, frame);
                // Drawn by the server after all: the SVGs are not needed.
                self.svg_for = None;
            }
            Err(err) => eprintln!("[frame] {err}; drawing it here"),
        }
        self.draw_here(write).await
    }

    /// Sends the current slide's frame to the display. With `ask_server`, an SVG slide not drawn
    /// yet is left to the server instead. Fails when the display thread is gone and nothing is
    /// left to show the frame.
    fn render(&mut self, ask_server: bool) -> Result<Shown, std::sync::mpsc::SendError<Frame>> {
        let state = &self.state;
        let dims = self.dims;
        let blank = || vec![0u32; (dims.width * dims.height) as usize];
        if state.muted {
            self.tx.send(blank())?;
            return Ok(Shown::Done);
        }

        if self.theme == PresenterTheme::Classic && state.render_mode == PresenterRenderMode::Svg {
            let slide = state.current_slide;
            if !self.cache.contains_key(&slide) {
                if ask_server {
                    return Ok(Shown::AskServer);
                }
                let svg = state.svg_slides.iter().find(|s| s.index == slide);
                if self.svg_omitted && svg.is_none_or(|s| s.svg.is_empty()) {
                    return Ok(Shown::NeedSvg);
                }
                let frame = match svg {
                    Some(svg) => {
                        crate::renderer::render_svg_slide(&svg.svg, dims.width, dims.height)
                            .unwrap_or_else(|err| {
                                eprintln!("[render] SVG render failed: {err}");
                                blank()
                            })
                    }
                    None => {
                        eprintln!(
                            "[render] SVG slide missing: current={slide} available={}",
                            state.svg_slides.len()
                        );
                        blank()
                    }
                };
                self.cache.insert(slide, frame);
            }
            if let Some(frame) = self.cache.get(&slide) {
                self.tx.send(frame.clone())?;
            }
            return Ok(Shown::Done);
        }

        let owned: Vec<(String, String, f64)> = state
            .slides
            .iter()
            .find(|s| s.index == state.current_slide)
            .map(|s| {
                s.paragraphs
                    .iter()
                    .map(|p| (p.display_text(), p.align.clone(), p.font_size_pt))
                    .collect()
            })
            .unwrap_or_default();
        let paragraphs: Vec<(&str, &str, f64)> = owned
            .iter()
            .map(|(t, a, pt)| (t.as_str(), a.as_str(), *pt))
            .collect();

        let rgb = crate::renderer::render_slide(
            &paragraphs,
            self.theme,
            state.current_slide,
            state.slides.len() as u32,
            dims.width,
            dims.height,
        );
        self.tx.send(crate::renderer::rgb_to_u32(&rgb))?;
        Ok(Shown::Done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timeout_draws_here_once_and_asks_the_server_again_for_the_next_slide() {
        let mut frames = FrameRequests::new();
        assert!(frames.should_ask(1));
        frames.asked(1);
        assert!(frames.waiting());

        frames.time_out();
        assert!(
            !frames.waiting(),
            "slide 1 is drawn here, nothing to wait for"
        );
        assert_eq!(frames.wait(), IDLE_TIMEOUT);
        assert!(
            frames.supported,
            "a slow reply doesn't turn server drawing off"
        );
        assert!(!frames.should_ask(1), "slide 1 was drawn here");
        assert!(
            frames.should_ask(2),
            "the next slide is asked of the server again"
        );

        frames.asked(2);
        assert!(frames.waiting());
        // Slide 1's late reply comes first and must not be taken for slide 2's.
        assert!(!frames.answered());
        assert!(frames.waiting());
        assert!(frames.answered(), "the next reply answers slide 2");
        assert_eq!(frames.slide, Some(2));
        assert!(!frames.waiting());
    }

    #[test]
    fn only_unauthorized_turns_server_drawing_off() {
        let mut frames = FrameRequests::new();
        frames.asked(1);
        assert!(frames.refused("not_an_svg_slide"));
        assert!(frames.supported);

        frames.asked(2);
        assert!(frames.refused("unauthorized"));
        assert!(!frames.supported);
        assert!(!frames.should_ask(3));
    }

    #[test]
    fn a_light_state_reads_with_and_without_svg_omitted() {
        let light = r#"{"type":"presenter.state","svgOmitted":true,"state":{"currentSlide":2,"renderMode":"svg","svgSlides":[{"index":2,"svg":""}]}}"#;
        let Ok(ServerMsg::PresenterState { state, svg_omitted }) = serde_json::from_str(light)
        else {
            panic!("a presenter.state");
        };
        assert!(svg_omitted);
        assert_eq!(state.current_slide, 2);

        let old = r#"{"type":"presenter.state","state":{"currentSlide":1}}"#;
        let Ok(ServerMsg::PresenterState { svg_omitted, .. }) = serde_json::from_str(old) else {
            panic!("a presenter.state");
        };
        assert!(!svg_omitted, "an older server always sends the SVGs");
    }

    fn svg_screen(tx: &std::sync::mpsc::Sender<Frame>) -> Screen<'_> {
        Screen {
            state: PresenterState {
                current_slide: 1,
                render_mode: PresenterRenderMode::Svg,
                slides: Vec::new(),
                svg_slides: vec![SvgSlideContent {
                    index: 1,
                    svg: String::new(),
                }],
                muted: false,
            },
            svg_omitted: true,
            theme: PresenterTheme::Classic,
            cache: HashMap::new(),
            frames: FrameRequests::new(),
            svg_for: None,
            tx,
            dims: DisplayDims {
                width: 4,
                height: 2,
            },
        }
    }

    #[test]
    fn an_svg_slide_left_out_is_asked_of_the_server_or_needs_its_svg() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut screen = svg_screen(&tx);
        assert_eq!(screen.render(true), Ok(Shown::AskServer));
        assert_eq!(screen.render(false), Ok(Shown::NeedSvg));
        assert!(rx.try_recv().is_err(), "the screen keeps what it shows");

        // A server JPEG in the cache is shown without the SVG.
        screen.cache.insert(1, vec![7; 8]);
        assert_eq!(screen.render(false), Ok(Shown::Done));
        assert_eq!(rx.try_recv(), Ok(vec![7; 8]));
    }
}

mod renderer;
mod ws;

use std::sync::{Arc, Mutex};
use ws::{DisplayDims, Frame};

#[derive(Clone, Copy, PartialEq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Reconnecting,
    Failed,
}

// ── CLI ───────────────────────────────────────────────────────────────────────

fn usage() -> ! {
    eprintln!(concat!(
        "Usage: presenter-receiver <ws-url> [--token <token>]\n",
        "\n",
        "Examples:\n",
        "  presenter-receiver ws://192.168.1.10:3737/ws\n",
        "  presenter-receiver ws://192.168.1.10:3737/ws --token abc123",
    ));
    std::process::exit(1);
}

fn parse_args() -> String {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let host = args.first().unwrap_or_else(|| usage()).clone();
    if host == "--help" || host == "-h" {
        usage();
    }

    let token = args
        .windows(2)
        .find(|w| w[0] == "--token")
        .map(|w| w[1].clone());

    receiver_url(&host, token.as_deref())
}

/// The server URL with `presenter=frames`, which asks the server to leave the SVG slides out of
/// `presenter.state`: the receiver gets each slide drawn as a JPEG with `presenter.frame`.
fn receiver_url(host: &str, token: Option<&str>) -> String {
    let separator = if host.contains('?') { '&' } else { '?' };
    let mut url = format!("{host}{separator}presenter=frames");
    if let Some(token) = token {
        url.push_str("&token=");
        url.push_str(token);
    }
    url
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    let url = parse_args();
    let dims = detect_display_dims();

    eprintln!(
        "[display] {}×{} — connecting to {url}",
        dims.width, dims.height
    );

    let conn_state = Arc::new(Mutex::new(ConnectionState::Connecting));

    // Channel: WS background thread → display main thread
    let (tx, rx) = std::sync::mpsc::channel::<Frame>();

    // Spawn tokio runtime + WS loop on a background thread.
    // (On macOS, minifb requires the display loop to own the main thread.)
    {
        let url = url.clone();
        let conn_state = Arc::clone(&conn_state);
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(ws::run(url, tx, dims, conn_state));
        });
    }

    run_display(rx, dims, conn_state);
}

// ── Display dimensions ────────────────────────────────────────────────────────

fn detect_display_dims() -> DisplayDims {
    #[cfg(target_os = "linux")]
    {
        // Read actual framebuffer resolution so the renderer fills the screen exactly
        if let Ok(fb) = framebuffer::Framebuffer::new("/dev/fb0") {
            return DisplayDims {
                width: fb.var_screen_info.xres,
                height: fb.var_screen_info.yres,
            };
        }
    }

    // macOS default window size (or Linux fallback when /dev/fb0 is unavailable)
    DisplayDims {
        width: 1280,
        height: 720,
    }
}

// ── macOS display: minifb window ──────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn run_display(
    rx: std::sync::mpsc::Receiver<Frame>,
    dims: DisplayDims,
    conn_state: Arc<Mutex<ConnectionState>>,
) {
    use minifb::{Key, Window, WindowOptions};

    let w = dims.width as usize;
    let h = dims.height as usize;

    let mut window = Window::new(
        "Presenter Receiver",
        w,
        h,
        WindowOptions {
            resize: false,
            ..Default::default()
        },
    )
    .expect("Failed to open window");

    window.set_target_fps(30);

    let mut clean_frame: Frame = vec![0u32; w * h];

    while window.is_open() && !window.is_key_down(Key::Escape) {
        while let Ok(frame) = rx.try_recv() {
            clean_frame = frame;
        }
        let state = *conn_state.lock().unwrap();
        let mut display_frame = clean_frame.clone();
        renderer::draw_status_overlay(&mut display_frame, dims.width, dims.height, state);
        window.update_with_buffer(&display_frame, w, h).unwrap();
    }
}

// ── Linux display: framebuffer (/dev/fb0) ─────────────────────────────────────

/// Hides the terminal cursor and restores it when dropped.
#[cfg(target_os = "linux")]
struct CursorHide;

#[cfg(target_os = "linux")]
impl CursorHide {
    fn new() -> Self {
        print!("\x1b[?25l");
        flush_stdout();
        CursorHide
    }
}

#[cfg(target_os = "linux")]
impl Drop for CursorHide {
    fn drop(&mut self) {
        print!("\x1b[?25h");
        flush_stdout();
    }
}

#[cfg(target_os = "linux")]
fn flush_stdout() {
    use std::io::Write;
    if let Err(error) = std::io::stdout().flush() {
        eprintln!("[display] stdout could not be flushed: {error}");
    }
}

#[cfg(target_os = "linux")]
fn run_display(
    rx: std::sync::mpsc::Receiver<Frame>,
    dims: DisplayDims,
    conn_state: Arc<Mutex<ConnectionState>>,
) {
    let _cursor = CursorHide::new();
    let mut fb = framebuffer::Framebuffer::new("/dev/fb0").unwrap_or_else(|e| {
        eprintln!("[display] Cannot open /dev/fb0: {e}");
        eprintln!("[display] Ensure a display is connected via HDMI.");
        eprintln!("[display] If running headless, add 'hdmi_force_hotplug=1' to /boot/firmware/config.txt and reboot.");
        std::process::exit(1);
    });
    let fb_w = fb.var_screen_info.xres as usize;
    let fb_h = fb.var_screen_info.yres as usize;
    let bpp = fb.var_screen_info.bits_per_pixel as usize / 8;
    let src_w = dims.width as usize;
    let src_h = dims.height as usize;

    let mut clean_frame: Frame = vec![0u32; src_w * src_h];
    let mut last_state = ConnectionState::Connecting;
    // Paint the initial black frame with the connecting indicator immediately.
    let mut dirty = true;

    loop {
        // Drain all pending frames; keep only the latest.
        while let Ok(frame) = rx.try_recv() {
            clean_frame = frame;
            dirty = true;
        }

        let state = *conn_state.lock().unwrap();
        if state != last_state {
            last_state = state;
            dirty = true;
        }

        if dirty {
            dirty = false;
            let mut display_frame = clean_frame.clone();
            renderer::draw_status_overlay(&mut display_frame, dims.width, dims.height, state);

            let mut fb_frame = vec![0u8; fb_w * fb_h * bpp];
            for y in 0..src_h.min(fb_h) {
                for x in 0..src_w.min(fb_w) {
                    let pixel = display_frame[y * src_w + x];
                    let r = ((pixel >> 16) & 0xff) as u8;
                    let g = ((pixel >> 8) & 0xff) as u8;
                    let b = (pixel & 0xff) as u8;
                    let d = (y * fb_w + x) * bpp;
                    match bpp {
                        2 => {
                            let v: u16 = ((r as u16 & 0xF8) << 8)
                                | ((g as u16 & 0xFC) << 3)
                                | (b as u16 >> 3);
                            fb_frame[d] = (v & 0xFF) as u8;
                            fb_frame[d + 1] = (v >> 8) as u8;
                        }
                        3 => {
                            fb_frame[d] = b;
                            fb_frame[d + 1] = g;
                            fb_frame[d + 2] = r;
                        }
                        4 => {
                            fb_frame[d] = b;
                            fb_frame[d + 1] = g;
                            fb_frame[d + 2] = r;
                            fb_frame[d + 3] = 0xff;
                        }
                        _ => {}
                    }
                }
            }
            fb.write_frame(&fb_frame);
        }

        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::receiver_url;

    #[test]
    fn the_url_asks_for_frames_and_keeps_the_token() {
        assert_eq!(
            receiver_url("ws://10.0.0.2:3737/ws", None),
            "ws://10.0.0.2:3737/ws?presenter=frames"
        );
        assert_eq!(
            receiver_url("ws://10.0.0.2:3737/ws", Some("abc")),
            "ws://10.0.0.2:3737/ws?presenter=frames&token=abc"
        );
        assert_eq!(
            receiver_url("ws://10.0.0.2:3737/ws?token=abc", None),
            "ws://10.0.0.2:3737/ws?token=abc&presenter=frames"
        );
    }
}

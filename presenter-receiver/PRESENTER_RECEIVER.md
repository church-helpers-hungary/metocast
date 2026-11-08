# Presenter Receiver

A standalone binary that connects to the Metocast server over WebSocket and renders presentation slides directly on a display — no browser required. Designed for Raspberry Pi running Raspberry Pi OS Lite (Linux framebuffer) or as a secondary window on macOS.

---

## Supported platforms

| Platform                                              | Binary                            |
| ----------------------------------------------------- | --------------------------------- |
| Linux arm64, 64-bit OS (Raspberry Pi 3/4/5)           | `presenter-receiver-linux-arm64`  |
| Linux armhf (armv7), 32-bit OS (Raspberry Pi 2/3/4/5) | `presenter-receiver-linux-armv7`  |
| Linux x86_64                                          | `presenter-receiver-linux-x86_64` |
| macOS Apple Silicon                                   | `presenter-receiver-macos-arm64`  |

On Linux the binary must match the operating system's userland, not the CPU or kernel. 32-bit Raspberry Pi OS runs a 64-bit kernel on a Pi 3/4/5 (`uname -m` reports `aarch64`), but it can only run the 32-bit `linux-armv7` binary. Check with `dpkg --print-architecture`: `arm64` needs `linux-arm64`, `armhf` needs `linux-armv7`. The installer checks this and picks the right binary. The armv7 build needs an ARMv7 CPU, so the Pi Zero / Zero W and Pi 1 (ARMv6) are not supported.

---

## How it works

The binary runs as a single process with two concurrent parts:

**WebSocket thread** — connects to the Metocast server, sends a registration message, and listens for slide state updates. When a new slide or state arrives it renders the slide to a pixel frame and sends it to the display thread via a channel. Reconnects automatically every 3 seconds on disconnect.

**Display thread** — owns the output device and polls the channel for new frames every 100 ms. On Linux it writes pixel data directly to `/dev/fb0` (the kernel framebuffer). On macOS it drives a `minifb` window. The display thread also reads the current connection state and composites a status indicator into the top-right corner of every frame before writing to the output.

**Rendering** — slides are rendered with Cairo and Pango. White bold text (Helvetica Neue) is vertically centred on a dark background (`#0D0D14`). Font size is determined by binary search to fit all paragraphs within 80 % of the frame height. A thin accent line is drawn at the top.

**Status indicator** — a small pill in the top-right corner of the display shows the connection state at all times:

| Indicator                  | Meaning                                   |
| -------------------------- | ----------------------------------------- |
| Filled green dot + version | Connected to the server                   |
| Filled orange dot          | Connecting or reconnecting                |
| Filled red dot             | 5 or more consecutive connection failures |

**Picture mute** — when the server sets the muted state the binary renders a plain black frame instead of slide content. The status indicator remains visible.

---

## Installation

### One-line installer (recommended)

The installer script auto-detects the platform, installs required system libraries (Linux only), downloads the correct binary from the latest GitHub release, and places it in `/usr/local/bin`.

**Install only:**

```bash
curl -fsSL https://raw.githubusercontent.com/church-helpers-hungary/metocast/master/presenter-receiver/install.sh | bash
```

**Install and start immediately:**

```bash
curl -fsSL https://raw.githubusercontent.com/church-helpers-hungary/metocast/master/presenter-receiver/install.sh | bash -s -- ws://YOUR_SERVER_IP:3737/ws
```

**Install with auto-start on boot** (Linux / systemd only):

```bash
curl -fsSL https://raw.githubusercontent.com/church-helpers-hungary/metocast/master/presenter-receiver/install.sh | bash -s -- ws://YOUR_SERVER_IP:3737/ws --service
```

The install command is also available pre-filled with your server's address on the **Connect** page of the Tauri application.

### What the installer does

1. Detects OS and CPU architecture.
2. On Linux with `apt`: checks for and installs missing shared libraries (`libcairo2`, `libcairo-gobject2`, `libpango-1.0-0`, `libpangocairo-1.0-0`, `libpangoft2-1.0-0`, `libglib2.0-0`). On Debian trixie and later, including current Raspberry Pi OS, it installs the renamed `t64` package where one exists (`libglib2.0-0t64`).
3. Downloads the matching binary from the latest GitHub release.
4. Installs the binary to `/usr/local/bin/presenter-receiver` (falls back to the current directory if sudo is unavailable).
5. If `--service` is passed:
   - Removes any previously installed background service.
   - Configures getty auto-login on TTY1 so the console logs in automatically on boot (no login prompt).
   - Creates `~/.local/log/` and writes a restart loop to `~/.bash_profile` that redirects all output to `~/.local/log/presenter-receiver.log`.
   - Writes `/etc/sysctl.d/99-presenter-console.conf` with `kernel.printk = 1 4 1 3` to prevent kernel messages (undervoltage warnings, hwmon events, etc.) from overwriting the framebuffer display.
   - Restarts the getty service so the change takes effect immediately.

### Manual download

```bash
# Replace BINARY with the correct name for your platform (see table above)
curl -fsSL https://github.com/church-helpers-hungary/metocast/releases/latest/download/BINARY -o presenter-receiver
chmod +x presenter-receiver
./presenter-receiver ws://YOUR_SERVER_IP:3737/ws
```

---

## Connecting to the server

### Basic usage

```bash
presenter-receiver ws://192.168.1.10:3737/ws
```

### With authentication token

If the Metocast server requires a token (recommended for production), pass it via `--token`:

```bash
presenter-receiver ws://192.168.1.10:3737/ws --token YOUR_TOKEN
```

The token is shown on the **Connect** page of the Tauri application under **Auth Token**.

### Local development on macOS

When the Tauri app is running locally, start the receiver from source with:

```bash
cargo run --manifest-path presenter-receiver/Cargo.toml --bin presenter-receiver -- \
  ws://127.0.0.1:3737/ws --token YOUR_TOKEN
```

### Finding your server address

Open the Metocast desktop app and go to **Connect**. The **Network URL** field shows the address other devices on the same network should use. Copy it and replace `http://` with `ws://`, then append `/ws`:

```
http://192.168.1.10:3737  ->  ws://192.168.1.10:3737/ws
```

---

## Viewing logs

When installed with `--service`, all output is written to:

```
~/.local/log/presenter-receiver.log
```

To stream logs live from another machine:

```bash
ssh pi@raspberrypi
tail -f ~/.local/log/presenter-receiver.log
```

The **Connect** page of the Tauri app has an SSH Access section that generates the SSH command for any saved device and provides the log tail command ready to copy.

---

## Updating

Re-run the same install command used during the original installation. The script always downloads the latest release and overwrites the existing binary. If `--service` was used originally, include it again — the script is idempotent and will update the WebSocket URL in `~/.bash_profile` as well.

After the script completes, reboot the device (or kill the running process — the restart loop will start the new binary within 3 seconds).

---

## Required WebSocket message format

The presenter receiver speaks a subset of the Metocast WebSocket protocol. All messages are JSON text frames.

### Connection URL

The receiver adds `presenter=frames` to the URL it is given (`ws://host:3737/ws?presenter=frames&token=…`). It tells the server that this screen gets SVG slides drawn as JPEGs (`presenter.frame`), so the server leaves the SVGs out of `presenter.state`. A deck with photos is hundreds of kilobytes of SVG, too much to send a Raspberry Pi on Wi‑Fi or a 100 Mb/s port on every step of the service order. A server older than the parameter ignores it and sends the SVGs as before.

### Messages sent by the receiver (client to server)

**Registration** — sent immediately after the WebSocket handshake:

```json
{
  "type": "presenter.register",
  "label": "Presenter Receiver",
  "hostname": "raspberrypi"
}
```

`hostname` is obtained by running the system `hostname` command. It is `null` if the command fails.

**State request** — sent immediately after registration to receive the current presenter state:

```json
{ "type": "presenter.status" }
```

**SVG request** — sent only when the receiver has to draw an SVG slide itself and the server left the SVGs out (`svgOmitted: true`). The server replies with `presenter.state` including them:

```json
{ "type": "presenter.status", "svg": true }
```

**Ping reply** — sent in response to every `ping` message from the server:

```json
{ "type": "pong", "ping_id": 42 }
```

`ping_id` must be the integer value received in the `ping` message.

**Frame request** — sent for an SVG slide the receiver has not shown yet in the current state. The server draws the slide at the display's size and replies to this receiver only with one **binary** message: a JPEG of exactly `width`×`height` (up to 3840×2160), letterboxed on black. The receiver decodes it and caches it per slide until the next `presenter.state`:

```json
{ "type": "presenter.frame", "slide": 1, "width": 1920, "height": 1080 }
```

A Raspberry Pi takes seconds to draw a full-screen photo with resvg (7 s for the welcome slide on a Pi 2 at 1080p, 33 s at 4K); decoding the server's JPEG takes about 1 s on a Pi 2. Replies come in order, so only the reply to the latest request is shown, and a late reply is never taken for a later request's.

- If the server answers `unauthorized` (a server older than the command), the receiver draws every SVG slide itself for the rest of the connection.
- If it answers another `error`, or sends nothing within 10 s (a slow link), the receiver draws that one slide itself, asking for the SVGs first when they were left out, and asks the server again for the next slide.

---

### Messages consumed by the receiver (server to client)

The receiver ignores all message types it does not recognise. Unknown types do not cause an error or disconnection.

**Presentation settings** — sent on connection and whenever the operator changes the presenter design:

```json
{
  "type": "presentation.settings",
  "useWebPresenter": true,
  "presenterTheme": "editorial"
}
```

`presenterTheme` is `"classic"` or `"editorial"`. For imported decks, Classic displays the source SVG while Editorial renders the extracted song text; generated Bible slides use the matching text design. This lets a design change update the live output without reopening the deck. Older servers that omit the field are treated as Classic.

**Full presenter state** — sent by the server on load, unload, slide change, and mute toggle. Replaces the entire local state:

```json
{
  "type": "presenter.state",
  "state": {
    "loaded": true,
    "filePath": "/path/to/file.pptx",
    "currentSlide": 2,
    "totalSlides": 10,
    "renderMode": "text",
    "muted": false,
    "slides": [
      {
        "index": 1,
        "paragraphs": [
          { "text": "Title line", "align": "center" },
          { "text": "Subtitle", "align": "center" }
        ]
      },
      {
        "index": 2,
        "paragraphs": [{ "text": "Second slide", "align": "left" }]
      }
    ],
    "svgSlides": []
  },
  "svgOmitted": false
}
```

Field notes:

- `loaded` — when `false`, all other fields are at their zero values and the receiver shows a black screen.
- `currentSlide` — 1-based index into `slides`. `0` means no slide is active.
- `renderMode` — `"text"` uses the paragraph renderer; `"svg"` uses `svgSlides`.
- `muted` — when `true`, the receiver renders a black frame regardless of `currentSlide`.
- `slides` — full array of all slides including their text content. Only the slide at `currentSlide` is rendered.
- `svgSlides` — full array of self-contained SVG slides when `renderMode` is `"svg"`. With `svgOmitted: true` each entry keeps its `index` and size but its `svg` is empty.
- `svgOmitted` — `true` when the receiver connected with `presenter=frames`; older servers leave it out, which means `false`.
- `paragraphs[].align` — one of `"left"`, `"center"`, `"right"`, `"justify"`. The receiver maps these to Pango alignment values.
- `paragraphs[].text` — plain text. Hard line breaks within a paragraph are represented as `\n` characters.

**SVG presenter state** — used when the web presenter opens a normal PPTX in visual mode. The receiver asks the server for the active slide as a frame (`presenter.frame`), and draws the SVG itself only when the server can't. This is the state as the receiver gets it with `presenter=frames`:

```json
{
  "type": "presenter.state",
  "state": {
    "loaded": true,
    "filePath": "/path/to/file.pptx",
    "currentSlide": 1,
    "totalSlides": 10,
    "renderMode": "svg",
    "muted": false,
    "slides": [],
    "svgSlides": [
      {
        "index": 1,
        "svg": "",
        "widthPx": 1280,
        "heightPx": 720
      }
    ]
  },
  "svgOmitted": true
}
```

The server queues messages for each client. When a newer `presenter.state` is broadcast while an older one is still waiting to be sent to a slow receiver, the older one is dropped, along with the `presenter.slide_changed` messages queued before the newer state: the receiver jumps straight to the current deck instead of showing each one it missed.

**Incremental slide update** — sent when only the active slide number changes (navigation). The receiver updates `currentSlide` and re-renders without replacing the slide content array:

```json
{
  "type": "presenter.slide_changed",
  "currentSlide": 3,
  "totalSlides": 10
}
```

`totalSlides` is present for informational use; the receiver does not use it for rendering.

**Ping** — sent by the server as a keepalive. The receiver must reply with `pong`:

```json
{ "type": "ping", "pingId": 42 }
```

---

### Implementing a compatible server

A server is compatible with the receiver if it:

1. Accepts a WebSocket connection at any path (the receiver connects to whatever URL is passed on the command line).
2. Sends `presentation.settings` on connection, defaulting `presenterTheme` to `"classic"` when no theme setting exists.
3. Sends `presenter.state` on connection (in response to `presenter.status`) and whenever state changes.
4. Sends `presenter.slide_changed` on navigation.
5. Sends periodic `ping` messages and expects `pong` replies.
6. Accepts `presenter.register` for informational purposes (the receiver sends it but does not require a response).

The active render array must be complete in every `presenter.state` message — partial content updates are not supported. If `loaded` is `false`, `slides` and `svgSlides` should be empty and `currentSlide` should be `0`.

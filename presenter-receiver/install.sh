#!/usr/bin/env bash
# Presenter Receiver — one-line installer
#
# Install only:
#   curl -fsSL https://raw.githubusercontent.com/church-helpers-hungary/metocast/master/presenter-receiver/install.sh | bash
#
# Install and run immediately:
#   curl -fsSL https://raw.githubusercontent.com/church-helpers-hungary/metocast/master/presenter-receiver/install.sh | bash -s -- ws://YOUR_SERVER_IP:3737/ws
#
# Install and register as a systemd service (auto-starts on boot):
#   curl -fsSL https://raw.githubusercontent.com/church-helpers-hungary/metocast/master/presenter-receiver/install.sh | bash -s -- ws://YOUR_SERVER_IP:3737/ws --service
#
# Update only (what the auto-start loop runs before each start; never asks for a password):
#   curl -fsSL https://raw.githubusercontent.com/church-helpers-hungary/metocast/master/presenter-receiver/install.sh | bash -s -- --update
#
set -e

REPO="church-helpers-hungary/metocast"
INSTALL_SCRIPT_URL="https://raw.githubusercontent.com/${REPO}/master/presenter-receiver/install.sh"
DEST="${PRESENTER_INSTALL_DIR:-/usr/local/bin}/presenter-receiver"
# The release tag the installed binary came from, so an update can skip the download.
TAG_FILE="${DEST}.release-tag"
WS_URL=""
INSTALL_SERVICE=false
UPDATE_ONLY=false
for arg in "$@"; do
    case "$arg" in
        --service) INSTALL_SERVICE=true ;;
        --update)  UPDATE_ONLY=true ;;
        -*)        echo "Unknown option: $arg" >&2; exit 1 ;;
        *)         WS_URL="$arg" ;;
    esac
done

# An update runs unattended on boot: sudo must fail instead of waiting for a password.
if [ "$UPDATE_ONLY" = "true" ]; then
    SUDO="sudo -n"
else
    SUDO="sudo"
fi

# ── Detect platform ───────────────────────────────────────────────────────────
OS="$(uname -s)"
ARCH="$(uname -m)"

# On Linux, pick the binary by userland, not by `uname -m`: Raspberry Pi OS
# 32-bit runs a 64-bit kernel (aarch64) on Pi 3/4/5, but cannot start the arm64
# binary because /lib/ld-linux-aarch64.so.1 is missing. It gets the armv7 build.
if [ "$OS" = "Linux" ]; then
    if [ -n "$(command -v dpkg)" ]; then
        USERLAND="$(dpkg --print-architecture)"
    else
        # ELF class byte of /bin/sh: 1 = 32-bit, 2 = 64-bit
        case "$(od -An -t u1 -j4 -N1 /bin/sh 2>/dev/null | tr -d ' ')-${ARCH}" in
            1-aarch64|1-armv7l|1-armv8l) USERLAND="armhf" ;;
            1-*)                         USERLAND="32-bit" ;;
            *-aarch64)                   USERLAND="arm64" ;;
            *-x86_64)                    USERLAND="amd64" ;;
            *)                           USERLAND="$ARCH" ;;
        esac
    fi
    # armv6 CPUs (Pi Zero / Pi 1) report armhf too, but cannot run armv7 code.
    [ "$USERLAND" = "armhf" ] && [ "$ARCH" = "armv6l" ] && USERLAND="armv6l"
    PLATFORM="Linux-${USERLAND}"
else
    PLATFORM="${OS}-${ARCH}"
fi

case "$PLATFORM" in
    Darwin-arm64)  BINARY="presenter-receiver-macos-arm64"  ;;
    Linux-arm64)   BINARY="presenter-receiver-linux-arm64"  ;;
    Linux-armhf)   BINARY="presenter-receiver-linux-armv7"  ;;
    Linux-amd64)   BINARY="presenter-receiver-linux-x86_64" ;;
    *)
        echo "Unsupported platform: ${PLATFORM} (kernel: ${ARCH})" >&2
        echo "Supported: macOS arm64, Linux arm64, armhf (armv7, e.g. 32-bit Raspberry Pi OS) and x86_64" >&2
        exit 1
        ;;
esac

# ── Find the newest release that carries this binary ──────────────────────────
# The receiver ships in every app release (v2.11.0, …) and, up to 2.10, also in its own
# presenter-receiver-v* releases. Pick the highest version of either that has the asset, and
# remember the asset's API URL: GitHub's API download still works when the normal release
# download link times out.
RELEASES_JSON="$(curl -fsSL --connect-timeout 10 --max-time 60 \
    "https://api.github.com/repos/${REPO}/releases?per_page=50")"
RELEASE_LINE=$(printf '%s\n' "$RELEASES_JSON" | awk -v want="$BINARY" '
    /^    "tag_name": / { tag = $2; gsub(/[",]/, "", tag) }
    /^        "url": /  { url = $2; gsub(/[",]/, "", url) }
    /^        "name": / {
        name = $2; gsub(/[",]/, "", name)
        if (name == want) {
            version = tag; sub(/^presenter-receiver-v/, "", version); sub(/^v/, "", version)
            print version, tag, url
        }
    }' | sort -V -k1,1 | tail -1)
RELEASE_TAG="$(printf '%s' "$RELEASE_LINE" | cut -d' ' -f2)"
ASSET_API_URL="$(printf '%s' "$RELEASE_LINE" | cut -d' ' -f3)"

if [ -z "$RELEASE_TAG" ] || [ -z "$ASSET_API_URL" ]; then
    echo "Error: no release on GitHub carries $BINARY" >&2
    exit 1
fi

BASE_URL="https://github.com/${REPO}/releases/download/${RELEASE_TAG}"

if [ "$UPDATE_ONLY" = "true" ]; then
    if [ ! -x "$DEST" ]; then
        echo "Error: --update needs an installed receiver at $DEST; run the installer without --update first" >&2
        exit 1
    fi
    if [ -f "$TAG_FILE" ] && [ "$(cat "$TAG_FILE")" = "$RELEASE_TAG" ]; then
        echo "==> Presenter receiver is up to date (${RELEASE_TAG})"
        exit 0
    fi
fi

# ── Install Linux system dependencies ────────────────────────────────────────
if [ "$OS" = "Linux" ] && [ -n "$(command -v apt-get)" ]; then
    MISSING=()
    for lib in libcairo2 libcairo-gobject2 libpango-1.0-0 libpangocairo-1.0-0 libpangoft2-1.0-0 libglib2.0-0; do
        # Debian trixie renamed libraries for 64-bit time_t (libglib2.0-0 → libglib2.0-0t64)
        if [ "$(apt-cache pkgnames "${lib}t64" | grep -x "${lib}t64")" = "${lib}t64" ]; then
            lib="${lib}t64"
        fi
        if [ "$(dpkg-query -W -f='${db:Status-Status}' "$lib")" != installed ]; then
            MISSING+=("$lib")
        fi
    done
    if [ "${#MISSING[@]}" -gt 0 ]; then
        echo "==> Installing required system libraries: ${MISSING[*]}"
        $SUDO apt-get install -y "${MISSING[@]}"
    fi
fi

# ── Download ──────────────────────────────────────────────────────────────────
echo "==> Detected: ${PLATFORM} (kernel: ${ARCH}) → $BINARY"
echo "==> Downloading from GitHub Releases (${RELEASE_TAG})..."

TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

if ! curl -fsSL --connect-timeout 10 --max-time 300 "$BASE_URL/$BINARY" -o "$TMP"; then
    echo "==> Release download failed; trying GitHub's API download..."
    curl -fsSL --connect-timeout 10 --max-time 300 \
        -H "Accept: application/octet-stream" "$ASSET_API_URL" -o "$TMP"
fi

# ── Install ───────────────────────────────────────────────────────────────────
chmod +x "$TMP"

# Moves $1 to $2: with sudo, so /usr/local/bin and the service path stay stable, or without it
# where the folder is writable.
put_file() {
    $SUDO mv "$1" "$2" || { [ -w "$(dirname "$2")" ] && mv "$1" "$2"; }
}

if put_file "$TMP" "$DEST"; then
    echo "==> Installed to $DEST (${RELEASE_TAG})"
elif [ "$UPDATE_ONLY" = "true" ]; then
    echo "Error: cannot replace $DEST without a password; run the installer by hand" >&2
    exit 1
else
    LOCAL_DEST="./presenter-receiver"
    cp "$TMP" "$LOCAL_DEST"
    chmod +x "$LOCAL_DEST"
    DEST="$LOCAL_DEST"
    echo "==> /usr/local/bin not writable — installed to $LOCAL_DEST"
    echo "    To install globally: sudo mv $LOCAL_DEST /usr/local/bin/presenter-receiver"
fi

TAG_FILE="${DEST}.release-tag"
TAG_TMP="$(mktemp)"
printf '%s\n' "$RELEASE_TAG" > "$TAG_TMP"
chmod 644 "$TAG_TMP"
if ! put_file "$TAG_TMP" "$TAG_FILE"; then
    rm -f "$TAG_TMP"
    echo "warning: could not record the installed release in $TAG_FILE; the next update downloads it again" >&2
fi

if [ "$UPDATE_ONLY" = "true" ]; then
    exit 0
fi

# ── Auto-start via console auto-login (Linux only, opt-in via --service) ──────
if [ "$INSTALL_SERVICE" = "true" ] && [ "$OS" = "Linux" ] && [ -n "$(command -v systemctl)" ]; then
    if [ -z "$WS_URL" ]; then
        echo "Error: --service requires a WebSocket URL (e.g. ws://192.168.1.10:3737/ws)"
        exit 1
    fi

    SERVICE_USER="${SUDO_USER:-${USER}}"
    SERVICE_HOME=$(getent passwd "$SERVICE_USER" | cut -d: -f6)
    PROFILE="$SERVICE_HOME/.bash_profile"

    # Remove old background service if present (migration)
    if [ -f /etc/systemd/system/presenter-receiver.service ]; then
        echo "==> Removing old background service..."
        if ! sudo systemctl disable --now presenter-receiver; then
            echo "warning: could not disable the old presenter-receiver service, continuing" >&2
        fi
        sudo rm -f /etc/systemd/system/presenter-receiver.service
    fi

    # Configure getty auto-login on tty1 (no login prompt on boot)
    echo "==> Configuring console auto-login for '$SERVICE_USER'..."
    AUTOLOGIN_DIR="/etc/systemd/system/getty@tty1.service.d"
    sudo mkdir -p "$AUTOLOGIN_DIR"
    sudo tee "$AUTOLOGIN_DIR/autologin.conf" <<EOF
[Service]
ExecStart=
ExecStart=-/sbin/agetty --autologin $SERVICE_USER --noclear %I \$TERM
EOF

    # Create .bash_profile if it doesn't exist (source .profile/.bashrc for compatibility)
    if [ ! -f "$PROFILE" ]; then
        sudo tee "$PROFILE" <<'EOF'
# .bash_profile
[ -f ~/.profile ] && . ~/.profile
[ -f ~/.bashrc ]  && . ~/.bashrc
EOF
        sudo chown "$SERVICE_USER:$SERVICE_USER" "$PROFILE"
    fi

    # Remove any existing auto-start block (idempotent re-installs)
    MARKER="# presenter-receiver auto-start"
    if sudo test -f "$PROFILE" && sudo grep -q "$MARKER" "$PROFILE"; then
        sudo sed -i "/$MARKER/,/# end presenter-receiver/d" "$PROFILE"
    fi

    # Suppress kernel printk messages on the console (undervoltage, hwmon, etc.)
    # so they don't overwrite the framebuffer display.
    SYSCTL_FILE="/etc/sysctl.d/99-presenter-console.conf"
    if [ ! -f "$SYSCTL_FILE" ]; then
        echo "==> Suppressing kernel console messages..."
        echo 'kernel.printk = 1 4 1 3' | sudo tee "$SYSCTL_FILE"
        sudo sysctl -p "$SYSCTL_FILE"
    fi

    # Ensure log directory exists
    LOG_DIR="$SERVICE_HOME/.local/log"
    sudo mkdir -p "$LOG_DIR"
    sudo chown "$SERVICE_USER:$SERVICE_USER" "$LOG_DIR"
    LOG_FILE="$LOG_DIR/presenter-receiver.log"

    # Append the auto-start block. Before each start it fetches the current installer and runs
    # it with --update; offline or on any failure it says so in the log and starts the
    # receiver that is already installed.
    sudo tee -a "$PROFILE" <<BASHEOF

$MARKER
if [ "\$(tty)" = "/dev/tty1" ]; then
    while true; do
        presenter_updater="\$(mktemp)"
        if curl -fsSL --connect-timeout 10 --max-time 60 "$INSTALL_SCRIPT_URL" -o "\$presenter_updater" 2>> "$LOG_FILE"; then
            if ! bash "\$presenter_updater" --update >> "$LOG_FILE" 2>&1; then
                echo "[update] \$(date '+%F %T') update failed; starting the installed receiver" >> "$LOG_FILE"
            fi
        else
            echo "[update] \$(date '+%F %T') could not fetch the installer; starting the installed receiver" >> "$LOG_FILE"
        fi
        rm -f "\$presenter_updater"
        $DEST $WS_URL >> "$LOG_FILE" 2>&1
        sleep 3
    done
fi
# end presenter-receiver
BASHEOF
    sudo chown "$SERVICE_USER:$SERVICE_USER" "$PROFILE"

    sudo systemctl daemon-reload
    sudo systemctl enable getty@tty1
    sudo systemctl restart getty@tty1

    echo ""
    echo "==> Auto-start configured for user '$SERVICE_USER'."
    echo "    On next reboot the presenter launches automatically"
    echo "    on the console display — no login prompt."
    echo "    Logs: $LOG_FILE"
    echo ""
    echo "    To view logs remotely:"
    echo "      tail -f $LOG_FILE"
    echo ""
    echo "    To disable:"
    echo "      sudo rm $AUTOLOGIN_DIR/autologin.conf"
    echo "      Remove the '$MARKER' block from $PROFILE"
    exit 0
fi

# ── Run directly (no service) ─────────────────────────────────────────────────
if [ -n "$WS_URL" ]; then
    echo "==> Starting: $DEST $WS_URL"
    exec "$DEST" "$WS_URL"
else
    echo ""
    echo "Done. Usage:"
    echo "  $DEST ws://YOUR_SERVER_IP:3737/ws"
    echo ""
    echo "To auto-start on boot (Linux/systemd):"
    echo "  $DEST ws://YOUR_SERVER_IP:3737/ws --service"
fi

#!/bin/bash
set -e

export XDG_RUNTIME_DIR="/run/user/1000"
export WAYLAND_DISPLAY="wayland-1"
export WLR_BACKENDS="headless"
export WLR_NO_HARDWARE_CURSORS="1"
export WLR_LIBINPUT_NO_DEVICES="1"

mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
# a restarted container keeps its filesystem: stale sway/wayland sockets would be found first
rm -rf "${XDG_RUNTIME_DIR:?}"/*

mkdir -p /home/taboom/.config/sway
cp /opt/taboom/sway.conf /home/taboom/.config/sway/config

mkdir -p /home/taboom/Downloads

# start sway
echo "[taboom] starting sway..."
setsid sway &
SWAY_PID=$!

# wait for sway socket
SWAYSOCK=""
for i in $(seq 1 30); do
    FOUND=$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -name 'sway-ipc.*.sock' -print -quit 2>/dev/null)
    if [ -n "$FOUND" ]; then
        export SWAYSOCK="$FOUND"
        echo "[taboom] sway ready (socket: $SWAYSOCK)"
        break
    fi
    sleep 0.5
done

if [ -z "$SWAYSOCK" ]; then
    echo "[taboom] ERROR: sway did not start"
    exit 1
fi

# one persistent virtual mouse + keyboard (standard keymap) before any app starts;
# taboomd drives it over $XDG_RUNTIME_DIR/taboom-input.sock
echo "[taboom] starting vinput..."
vinput &
for i in $(seq 1 20); do
    [ -S "$XDG_RUNTIME_DIR/taboom-input.sock" ] && break
    sleep 0.25
done

# start Chrome on the active persona's profile (taboom-browser, which super+b and the bar also
# use, owns the flags, the profile folder and its first-run seeding)
echo "[taboom] starting chrome..."
swaymsg exec taboom-browser
sleep 3

# live view: VNC stays on loopback, noVNC serves it to the browser on :6080
echo "[taboom] starting live view on :6080..."
wayvnc 127.0.0.1 5900 &
websockify --web /usr/share/novnc 0.0.0.0:6080 127.0.0.1:5900 >/dev/null 2>&1 &

# ensure taboom home dirs exist
export TABOOM_HOME="${TABOOM_HOME:-/home/taboom/.taboom}"
mkdir -p "$TABOOM_HOME"/{personas,profiles,run,audit,logs,vault}

# create default persona if none exists
if [ ! -f "$TABOOM_HOME/personas/default.toml" ]; then
    cat > "$TABOOM_HOME/personas/default.toml" << 'TOML'
name = "default"
cpus = 2
ram_mb = 4096
timezone = "America/New_York"

[route]
type = "direct"

[browser]
accept_languages = "en-US,en"
download_dir = "/home/taboom/Downloads"

[hardware]
screen_width = 1920
screen_height = 1080
dpr = 1.0

[identity]
keyboard_layout = "us"
locale = "en_US.UTF-8"
languages = ["en-US", "en"]

[humanizer_style]
speed = "medium"
typo_rate = 0.02
overshoot_tendency = 0.15
TOML
    echo "[taboom] created default persona"
fi

PUBLIC_URL="${TABOOM_PUBLIC_URL:-http://localhost:3456}"
VIEW_URL="${TABOOM_VIEW_URL:-http://localhost:6080/vnc.html?autoconnect=1&resize=scale&view_only=1}"
# host ports as published, read back from the URLs so the labels match what you click
HOME_PORT="${PUBLIC_URL##*:}"; HOME_PORT="${HOME_PORT%%/*}"
VIEW_PORT="${VIEW_URL#*://*:}"; VIEW_PORT="${VIEW_PORT%%/*}"
cat <<BANNER

  ┌─ Taboom is up ────────────────────────────────────────────────────────────
  │  $(printf '%-5s' "$HOME_PORT") Home         $PUBLIC_URL/
  │  $(printf '%-5s' "$VIEW_PORT") Watch live   $VIEW_URL
  │        MCP          $PUBLIC_URL/mcp
  │        Claude Code  claude mcp add --transport http --scope user taboom $PUBLIC_URL/mcp
  │        Browser log  $TABOOM_HOME/logs/chrome.log
  └───────────────────────────────────────────────────────────────────────────

BANNER
exec taboomd

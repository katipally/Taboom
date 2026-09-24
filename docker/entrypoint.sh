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

export TABOOM_HOME="${TABOOM_HOME:-/home/taboom/.taboom}"
export TABOOM_PERSONA="${TABOOM_PERSONA:-default}"
mkdir -p "$TABOOM_HOME"/{personas,profiles,run,audit,logs,vault} /home/taboom/Downloads

# the persona file decides timezone, language, keyboard, screen, fonts and route; nothing
# starts unless it is valid and the route verifies (fail closed)
echo "[taboom] boot-check for persona $TABOOM_PERSONA..."
if ! taboomd boot-check; then
    echo "[taboom] ERROR: boot-check failed; fix $TABOOM_HOME/personas/$TABOOM_PERSONA.toml or the route and restart"
    exit 1
fi
# TZ, LANG, LANGUAGE, XKB_DEFAULT_LAYOUT, FONTCONFIG_FILE for everything started below
source "$TABOOM_HOME/run/env"

mkdir -p /home/taboom/.config/sway
cat /opt/taboom/sway.conf "$TABOOM_HOME/run/sway.conf" > /home/taboom/.config/sway/config

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

# one persistent virtual mouse + keyboard (the persona's XKB layout, from XKB_DEFAULT_LAYOUT)
# before any app starts; taboomd drives it over $XDG_RUNTIME_DIR/taboom-input.sock
echo "[taboom] starting vinput..."
vinput &
for i in $(seq 1 20); do
    [ -S "$XDG_RUNTIME_DIR/taboom-input.sock" ] && break
    sleep 0.25
done

# the daemon owns the route forwarder Chrome's proxy points at, so it comes up first
echo "[taboom] starting taboomd..."
rm -f "$TABOOM_HOME/run/taboomd.sock"
taboomd serve &
TABOOMD_PID=$!
# wait inside the trap too: the final `wait` returns as soon as a trapped signal arrives, and
# exiting then would stop the container before taboomd finalizes the session video
trap 'kill -TERM "$TABOOMD_PID" 2>/dev/null; wait "$TABOOMD_PID"' TERM INT
for i in $(seq 1 120); do
    [ -S "$TABOOM_HOME/run/taboomd.sock" ] && break
    kill -0 "$TABOOMD_PID" 2>/dev/null || { echo "[taboom] ERROR: taboomd exited"; exit 1; }
    sleep 0.25
done

# taboom-browser (also behind super+b and the bar) starts Chrome with the persona's flags
echo "[taboom] starting chrome..."
swaymsg exec taboom-browser
sleep 3

# live view: VNC stays on loopback, noVNC serves it to the browser on :6080
echo "[taboom] starting live view on :6080..."
wayvnc 127.0.0.1 5900 &
websockify --web /usr/share/novnc 0.0.0.0:6080 127.0.0.1:5900 >/dev/null 2>&1 &

PUBLIC_URL="${TABOOM_PUBLIC_URL:-http://localhost:3456}"
VIEW_URL="${TABOOM_VIEW_URL:-http://localhost:6080/vnc.html?autoconnect=1&resize=scale&view_only=1}"
# host ports as published, read back from the URLs so the labels match what you click
HOME_PORT="${PUBLIC_URL##*:}"; HOME_PORT="${HOME_PORT%%/*}"
VIEW_PORT="${VIEW_URL#*://*:}"; VIEW_PORT="${VIEW_PORT%%/*}"
cat <<BANNER

  ┌─ Taboom is up: persona $TABOOM_PERSONA ────────────────────────────────────
  │  $(printf '%-5s' "$HOME_PORT") Home         $PUBLIC_URL/
  │  $(printf '%-5s' "$VIEW_PORT") Watch live   $VIEW_URL
  │        MCP          $PUBLIC_URL/mcp
  │        Claude Code  claude mcp add --transport http --scope user taboom $PUBLIC_URL/mcp
  │        Browser log  $TABOOM_HOME/logs/chrome.log
  └───────────────────────────────────────────────────────────────────────────

BANNER
wait "$TABOOMD_PID"

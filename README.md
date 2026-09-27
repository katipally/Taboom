# Taboom

A self-hosted computer-use box for AI agents. The agent gets a real Linux desktop with a real
browser, sees it only through screenshots, and drives it only with mouse and keyboard input.
Nothing is injected into the page.

Taboom is a real MCP server. Start it once and any MCP client can connect: Claude Code, Claude
Desktop, Cursor, VS Code, your own agent loop, or plain `curl`. You can watch the agent live in
your browser, and every session is recorded step by step so you can replay or share it.

```
 ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐
 │ Claude Code │  │   Cursor    │  │  VS Code    │  │ your agent  │
 └──────┬──────┘  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘
        └────────────────┴───────┬────────┴────────────────┘
                                 │  MCP, streamable HTTP
                                 │  POST http://localhost:3456/mcp
 ┌───────────────────────────────▼──────────────────────────────────┐
 │ docker container "taboom"                                        │
 │                                                                  │
 │   taboomd  (MCP server, session lease, route check, recordings)   │
 │            :3456, and the proxy forwarder on 127.0.0.1:1080      │
 │      │                                                           │
 │      ├── grim ─────────> screenshots                             │
 │      ├── vinput ───────> one virtual mouse + keyboard            │
 │      │                   (moves and keystrokes timed by the      │
 │      │                    humanizer: curves, rhythm, typos)      │
 │      ├── wl-copy ──────> clipboard                               │
 │      └── wf-recorder ──> session video                           │
 │                                                                  │
 │   sway (headless Wayland desktop, top bar with launchers)        │
 │      ├── Google Chrome  (the persona's profile, tz, language)    │
 │      ├── foot terminal, fuzzel app launcher                      │
 │      └── wayvnc ──> noVNC live view                      :6080   │
 └──────────────────────────────────────────────────────────────────┘
```

```
 you, in any browser
   ├── http://localhost:3456/                                                  home: links + sessions
   ├── http://localhost:6080/vnc.html?autoconnect=1&resize=scale&view_only=1   watch live
   └── http://localhost:3456/recordings/<id>?exp=..&sig=..                     replay + video
```

The agent touches the desktop **only through the mouse and keyboard**, like a person. Nothing
is injected into pages, and no tool launches or controls apps behind the scenes.

> **Acceptable use:** personal and research agents acting for the person who runs them. Not for
> account farming, mass sign-ups, or dodging rate limits at scale.

**Runtime:** Docker Compose is the only supported runtime. **One container is one persona**: its
timezone, languages, keyboard layout, screen, fonts and network route are applied to that
container's desktop and Chrome at boot. More personas means more containers (see
[Several personas on one host](#several-personas-on-one-host)).

---

## Contents

1. [Quick start (Docker)](#1-quick-start-docker)
2. [Connect an agent](#2-connect-an-agent)
3. [Your first session](#3-your-first-session)
4. [Watch it live](#4-watch-it-live)
5. [Recordings](#5-recordings)
6. [Tools reference](#6-tools-reference)
7. [Configuration](#7-configuration)
8. [Auth and remote access](#8-auth-and-remote-access)
9. [Day-to-day commands](#9-day-to-day-commands)
10. [Troubleshooting](#10-troubleshooting)
11. [Developing](#11-developing)
12. [Repo map](#12-repo-map)

---

## 1. Quick start (Docker)

You need Docker with Compose v2. Nothing else. Clone this repository from GitHub or download and
extract its ZIP, then run this from the repository root:

```bash
docker compose up -d --build
```

The first build compiles the Rust code, so give it a few minutes. After that it starts in seconds.

What happens on `up`:

```
 docker compose up
   │
   ├─ 1. boot-check          reads personas/$TABOOM_PERSONA.toml (first boot writes a
   │                         default one), writes tz/lang/keyboard/screen/fonts settings,
   │                         verifies the route. Any failure: exit 1, Chrome never starts
   ├─ 2. sway starts         headless desktop at the persona's screen size and scale
   ├─ 3. vinput starts       one persistent virtual mouse + keyboard, persona's layout
   ├─ 4. taboomd starts      MCP on :3456, proxy forwarder, route re-check every 5 min
   ├─ 5. Chrome opens        on the persona's profile with its language and proxy flags
   ├─ 6. live view starts    wayvnc + noVNC on :6080
   └─ 7. links printed       "Taboom is up: persona default" banner in the logs
                                    │
                                    ▼
                          healthcheck goes "healthy"
                          (/healthz: vinput, sway and Chrome all answer)
```

Check it:

```bash
docker compose ps                  # STATUS should say (healthy)
docker compose logs taboom | grep -A6 "Taboom is up"
```

The logs print clickable links:

```
  ┌─ Taboom is up: persona default ─────────────────────────────────────────
  │  3456  Home         http://localhost:3456/
  │  6080  Watch live   http://localhost:6080/vnc.html?autoconnect=1&resize=scale&view_only=1
  │        MCP          http://localhost:3456/mcp
  │        Claude Code  claude mcp add --transport http --scope user taboom http://localhost:3456/mcp
  │        Browser log  /home/taboom/.taboom/logs/chrome.log
  └─────────────────────────────────────────────────────────────────────────
```

Or just open **http://localhost:3456/**. It has Watch live and Take over buttons, the connect
command, and your recent sessions with replay and video links.

In Docker Desktop, the two port links next to the container name do the same:

```
 3456:3456  ->  home page (links, connect command, recent sessions)
 6080:6080  ->  live view, watch only
```

Chrome's own output goes to `~/.taboom/logs/chrome.log` inside the container, so the
container logs stay readable.

### Ports and URLs

Two ports are published, both on `127.0.0.1` by default. Everything else stays inside the
container.

| Port | Path | What it is |
|---|---|---|
| **3456** | `/` | Home page: Watch live / Take over buttons, connect command, recent sessions |
| 3456 | `/mcp` | The MCP server (streamable HTTP, `POST`) |
| 3456 | `/recordings/<id>?exp=..&sig=..` | Signed replay page; `/video/video.mp4` and `/frames/<n>.png` under it |
| 3456 | `/healthz` | 200 when vinput, sway and Chrome all answer; 503 with the failing part otherwise. The Compose healthcheck uses it |
| **6080** | `/` | Redirects to the live view |
| 6080 | `/vnc.html?autoconnect=1&resize=scale&view_only=1` | Live view, watch only |
| 6080 | `/vnc.html?autoconnect=1&resize=scale` | Live view with mouse and keyboard (take over) |

Inside the container only, never published: wayvnc on `127.0.0.1:5900` (noVNC's backend) and
the input daemon's socket at `/run/user/1000/taboom-input.sock`.

To change the ports, set `TABOOM_MCP_PORT` and/or `TABOOM_VIEW_PORT` (see
[section 7](#7-configuration)). The banner, home page, `view_url`, and recording share links
follow those settings. Re-add your MCP client with the new URL.

Smoke-test the MCP endpoint with curl:

```bash
curl -s http://localhost:3456/mcp \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"curl","version":"0"}}}'
```

You should get back `"serverInfo":{"name":"taboom",...}`.

---

## 2. Connect an agent

The server speaks MCP over **streamable HTTP** at one URL:

```
 http://localhost:3456/mcp
```

Any client that supports HTTP MCP servers can use it directly.

### Claude Code

```bash
claude mcp add --transport http --scope user taboom http://localhost:3456/mcp
claude mcp list          # taboom should show as connected
```

`--scope user` makes it available in every project. Use `--scope project` to write it into this
repo's `.mcp.json` so teammates get it too.

With a token (see [section 8](#8-auth-and-remote-access)):

```bash
claude mcp add --transport http --scope user taboom http://localhost:3456/mcp \
  --header "Authorization: Bearer <your-token>"
```

### Cursor, VS Code, Claude Desktop, other JSON-config clients

Most clients take an `mcpServers` block. Paste this:

```json
{
  "mcpServers": {
    "taboom": {
      "type": "http",
      "url": "http://localhost:3456/mcp",
      "headers": { "Authorization": "Bearer <your-token>" }
    }
  }
}
```

Drop the `headers` line if you did not set tokens. Where the file lives depends on the client
(for example `.cursor/mcp.json` in Cursor, `.vscode/mcp.json` in VS Code). Check your client's
docs for the current path.

### Your own code

It is plain JSON-RPC 2.0 over POST. The flow any client follows:

```
 client                                   taboom
   │ POST initialize ───────────────────────>│
   │<───────────── protocolVersion, tools cap │
   │ POST notifications/initialized ────────>│  202, no body
   │ POST tools/list ───────────────────────>│
   │<──────────────────────── 31 tools        │
   │ POST tools/call session_start ─────────>│
   │ POST tools/call screenshot ────────────>│
   │<─────────────── image block (PNG)        │
   │ POST tools/call click / type / ... ────>│
```

Any official MCP SDK (Python, TypeScript, etc.) works with its streamable HTTP client.

---

## 3. Your first session

Every agent must **start a session** before it can touch the screen. The container is one
persona (named by `TABOOM_PERSONA`, `default` unless you set it), so there is nothing to pick.

```
 session_start ──> screenshot / click / type ... ──> session_end
       │
       ├─ one holder at a time; a second agent gets "held by '<name>'"
       └─ refused while the route check is failing
```

`persona_status` works with or without a session: which persona this is, its declared settings
next to what the runtime actually applied, and the route's health.

Try it in Claude Code after connecting:

```
Use taboom. Start a session, open https://example.com,
take a screenshot, and tell me what the page says.
```

The agent will call, in order:

| Step | Tool | Arguments |
|---|---|---|
| 1 | `session_start` | `{}` |
| 2 | `open_url` | `{"url": "https://example.com"}` |
| 3 | `wait` | `{"until_settled": true}` |
| 4 | `screenshot` | `{}` |

Screenshots come back as real MCP image blocks, so the model sees the pixels directly.

**How coordinates work.** Screenshots are scaled so the longest edge is 1280 px (1920x1080 comes
back as 1280x720). Model APIs shrink bigger images anyway, which would quietly throw clicks off.
The agent always clicks in the pixel space of its **latest full screenshot**, and Taboom maps
that to the real screen.

```
 real screen 1920x1080  <──── x / 0.667 ────  screenshot 1280x720  <── agent clicks (640, 360)
                                                                         lands at (960, 540)
```

- Clicks outside the screenshot are refused with the valid size.
- A `click` with an old `frame_id`, or any action after the screen changed size, is refused with
  "take a new screenshot".
- `zoom` (or `screenshot` with `region`) is for reading small text. It returns a full-resolution
  crop and does not change the coordinate space.

### Vault secret typing

Store credentials with the local `taboom vault add` command; it reads each value at a hidden
terminal prompt. In an MCP `type` call, refer to a saved item as `<secret>NAME</secret>` and never
include its value. Passwords and notes are typed as exact, layout-aware key presses; TOTP seeds
produce the current code. Secret calls never use paste or the clipboard, and the key sequence skips
the normal typo planner.

Taboom requires an unlocked vault, a healthy route, an allowed current domain, and a focused Chrome
window that is not fullscreen. HTTP pages are refused. An allow-list matches the exact hostname and
its subdomains; an empty list denies use. Taboom reads page targets through Chrome's private
`--remote-debugging-pipe` and the `Target.getTargets` method. The daemon's local bridge sends only
normalized HTTP(S) host candidates and their schemes; page IDs, titles, paths, and query strings
are not forwarded to the secret handler. Taboom does not attach to a page, enable `Runtime`, or
expose a debugging port. Since the target list does not identify the active tab, private OCR of the
visible Chrome omnibox selects the matching candidate host, and every open HTTP(S) tab and window
must be on that host (other sites refuse with `other_sites_open`; an about:blank, data:, blob: or
file: page anywhere refuses with `opaque_page_open`, since its opener can draw a fake address bar). A hidden scheme is accepted only when
the host has an HTTPS candidate and no same-host HTTP candidate; an explicit HTTP URL refuses
typing. Missing geometry, OCR errors, low confidence, or a host mismatch also refuse. The OCR crop and recognized
text stay in memory and are not returned, cached, or recorded. This check still cannot verify that
the focused control is a web form field rather than Chrome's address bar, or prevent a navigation
immediately after the final checks. Immediately before a secret call, take a screenshot and click
the intended page field; keep that page in place while typing. Taboom repeats the target and OCR
checks before stopping video and after finalizing it, then rechecks focus and route immediately
before input. Use `submit: true` only when the intended field and form submission are confirmed.

The recorder stores the original placeholder, never the expanded value. Taboom stops and finalizes
the current screen video before typing, waits for scheduled frame captures, then disables new
screenshots, zoom images, recording frames, and video for the rest of the persistent data volume.
That privacy marker survives a normal restart; only resetting the data volume clears it. The audit
log records the secret name and hostname for successful or refused attempts, never the value. The
live view remains visible to a person who opens it.

`~/.taboom/audit/taboomd.jsonl` is append-only and outlives recording cleanup. Besides vault
events it logs every MCP tool call: tool name, client and whether it succeeded, never the
arguments.

For better agent behavior, load the playbook in [`skills/SKILL.md`](skills/SKILL.md) into your
agent's instructions (Claude Code: copy it into a skill or your `CLAUDE.md`). It covers popups,
loop detection, secrets, and how to request manual user control through the live view. Site notes
live in `skills/sites/`.

---

## 4. Watch it live

Click **Watch live** on http://localhost:3456/, or open this in any browser while the agent works:

```
http://localhost:6080/vnc.html?autoconnect=1&resize=scale&view_only=1
```

```
 agent ──MCP──> taboomd ──> sway desktop ──> wayvnc (inside, loopback only)
                                                │
                                        websockify + noVNC :6080
                                                │
                                        your browser tab  (watch)
```

- `view_only=1` means you watch without touching. Open the control URL to use the mouse and
  keyboard yourself, for example to solve a CAPTCHA or type a 2FA code, then return to the
  watch-only link when you are done.
- `resize=scale` fits the desktop to your window at any size, phone included.
- The agent can fetch both links with the `view_url` tool (`url` to watch, `takeover_url` for
  direct control). If a task needs your input, open the control link yourself; Taboom no longer
  provides an agent-managed handoff workflow.
- It has no password. Keep it on `127.0.0.1` (the default) or behind a private network. See
  [section 8](#8-auth-and-remote-access).

---

## 5. Recordings

Every session records a **step log**. Screen video and frames are captured until a vault secret is
typed; then video is finalized before typing and all further visual recording is disabled for the
rest of the persistent data volume. A session starts at `session_start` and ends at `session_end`.

```
 session_start ───┬─ screenshot ─┬─ click ─┬─ type ─┬─ ... ─┬─ session_end
                  │              │         │        │       │
 events.jsonl     1              2         3        4       n     every call: time, tool, args,
                                                                  ok/error, duration
 frames/          -          00002.png  00003.png 00004.png   the exact image the agent saw, or
                                                              the screen 0.4 s after an action
```

On disk (inside the `taboom-data` volume):

```
 ~/.taboom/recordings/<id>/
     meta.json        persona, client, start, end
     events.jsonl     one line per tool call
     frames/          one PNG per visual step
     video.mp4        the whole session (H.264, max 1280 wide). While the session
                      runs it is video.mkv; it becomes .mp4 a few seconds after release
```

### Ask the agent for them

The agent has three tools. Just ask in plain words, for example "show me what you did in the last
session" or "give me a link to replay that run".

| Tool | What it does |
|---|---|
| `recording_list` | Sessions, newest first: id, persona, start/end, step count, video status. |
| `recording_get` | Steps from a session (default: the current one). `step: 12` returns that step plus its frame image. `frames: true` adds up to 10 images per page. Page through long sessions with `from_step` and `limit`. |
| `recording_share` | Signed links (default expiry 7 days): `url` for the replay page, `video_url` for the video file. Anyone with a link can open it. Tampered or expired links get `403`. |

The replay page plays the video at the top, then lists every step with its time, tool,
arguments, errors in red, and the frame. Images load lazily and the video streams with seeking,
so long sessions stay fast.

### Grab them by hand

```bash
docker compose cp taboom:/home/taboom/.taboom/recordings ./recordings
```

Frames take about 150-250 KB per visual step and video roughly 1-2 MB per busy minute (idle
screens add almost nothing). Set `TABOOM_RECORDINGS_MAX_GB` to cap the folder: when a session
starts, the oldest sessions are deleted until the rest fit, never the one still running. Unset,
nothing is deleted automatically.

File contents from `files_put` are not recorded (only their size). `<secret>` placeholders are
stored as the placeholder, never the value. When a secret is typed, `recording_get` will not return
frames for that data volume, including older frames; a video finalized before the secret remains
available because it ends before the secret was typed.

---

## 6. Tools reference

```
 SESSION              SEE                 MOUSE                  KEYBOARD
 ────────────────     ───────────────     ──────────────────     ─────────────────
 session_start        screenshot          click                  type
 session_end          zoom                move                   key
 persona_status       cursor_position     drag                   key_down
                                          scroll                 key_up
                                          mouse_down             hold_key
                                          mouse_up               open_url **

 RECORDINGS           OTHER
 ────────────────     ──────────────────────────────────────────────────────────
 recording_list       wait, files_put, files_get, files_list, clipboard_get,
 recording_get        clipboard_set, view_url, computer *
 recording_share

 31 tools in total.
```

`**` `open_url` is keyboard only: super+b (focuses the browser, or reopens it if closed), ctrl+t,
types the address, Enter.

`*` `computer` accepts Anthropic's computer-use tool schema (`screenshot`, the click variants,
`mouse_move`, `left_click_drag`, `left_mouse_down`, `left_mouse_up`, `type`, `key`,
`hold_key`, `scroll` with `scroll_direction` and `scroll_amount`, `wait` with `duration`,
`zoom` with `region`, `cursor_position`) and maps it onto the native tools, so existing
computer-use agent loops work unchanged.

Key arguments:

| Tool | Required args | Notes |
|---|---|---|
| `session_start` | none | Lease the desktop before using it. Starts the recording and reads the persona's keyboard layout from vinput once. Refused while another agent holds it or the route check fails. |
| `session_end` | none | Releases held keys and buttons, stops the recording, frees the desktop. |
| `persona_status` | none | Persona name, declared settings, applied values (TZ, LANG, keyboard, screen mode, `hardwareConcurrency`, `deviceMemory`), any mismatches, route health (exit IP, country, ASN, last check). No session needed. |
| `screenshot` | none | Longest edge 1280 by default; `max_edge` 200-4096. Returns `frame_id` and sizes. |
| `zoom` | `region` | `{x, y, w, h}` in screenshot coordinates. Returns a full-resolution crop. |
| `find_text` | `text` | Local OCR; returns matching lines with boxes and confidence in the latest full screenshot's pixel space. Optional `region` uses those coordinates. |
| `read_text` | none | Local OCR; returns visible lines with boxes and confidence. Optional `region` uses latest screenshot coordinates. |
| `click` | `x`, `y`, `describe` | `describe` says what you mean to do, e.g. "Submit the form". Optional `button`, `count` (1-3), `frame_id`. |
| `drag` | `from`, `to`, `describe` | Left button held and moved over about 0.4 s. |
| `scroll` | `direction`, `amount` | `up`, `down`, `left`, `right`; `amount` is wheel notches. Optional `x`, `y` (default: center). |
| `type` | `text` | `mode`: `keys` (real key presses on the persona's layout, with Shift/AltGr, human rhythm and the odd corrected typo), `paste` (clipboard + ctrl+v), or `auto` (keys, but pastes text over 400 chars or with characters the layout lacks). `<secret>NAME</secret>` uses exact key presses without paste/typo planning and requires an unlocked vault, an allow-listed hostname, a healthy route, focused non-fullscreen Chrome, and a matching, confident OCR read of the visible omnibox against Chrome's page-host candidates, with no tabs open on other sites. HTTP pages and hosts with a hidden scheme plus same-host HTTP and HTTPS candidates are refused. `submit: true` presses Enter after typing; use it only when the intended field and submission are confirmed. |
| `key` | `combo` | `ctrl+c`, `Enter`, `F5`, or a sequence: `ctrl+a Delete`. Also the desktop shortcuts below. |
| `mouse_down` / `mouse_up` | none | Hold a button (optionally after moving to `x`, `y`) until released. For drawing, custom drags, range selects. |
| `key_down` / `key_up` | `key` | Hold a key, like shift while clicking. |
| `hold_key` | `key` | Hold for `duration_ms` (default 1000, max 10000). |
| `wait` | none | `ms` sleeps. `until_settled: true` waits until the screen stops changing, or `until_text` waits for a case-insensitive visible text match; `ms` is the cap (default 3000). |
| `open_url` | `url` | Opens the URL in a new browser tab. |
| `files_put` / `files_get` / `files_list` | `name` + `data_base64` / `path` / none | Files in the browser's Downloads folder. `files_get` is capped at 20 MB and never leaves that folder. |
| `clipboard_get` / `clipboard_set` | none / `text` | The desktop clipboard. |
| `view_url` | none | `url` (watch) and `takeover_url` (direct control) for the live view. Give the control link to the user when they need to interact with the desktop. |

Every failure comes back as an MCP error with the reason, never as fake success. On
`session_end` every held key and button is let go. While the route check is failing, every
action except `session_end`, `persona_status` and the recording tools is refused.

### How input stays human

Every mouse move and keystroke is planned by `taboom-humanizer` and played through one
persistent virtual mouse and keyboard (`vinput`), in real time:

```
 aimed move   one continuous curved stroke: a primary movement that lands a little long or
              short, plus a corrective movement that starts before the first one stops.
              Speed rises fast and tails off; every move gets its own speed profile, bend and
              duration (Fitts' law); tremor fades out as the hand lands. Like a person
              aiming at a control rather than a pixel, it lands within 3 px of the point
              (sd ~1 px), so repeated clicks on one spot never hit the identical pixel
 between      the hand never freezes: it rests and drifts slowly near where the last action
 actions      happened (radius ~40 px); after typing it mostly rests, like a hand on keys
 typing       keys come from the persona's XKB layout (vinput answers `lookup` once per
              session: which key and whether Shift or AltGr), so "y" on a German layout
              is the key a German types; tempo varies per call and drifts across the
              text; longer gaps at spaces and after punctuation; digits and symbols
              slower; Shift/AltGr pressed a beat early; fast pairs overlap (rollover);
              occasional thinking pauses; typos on the physical row neighbor, sometimes
              noticed a key or two late, then backspaced
 per persona  a stable style (speed, typing habits) from the persona's `humanizer.seed`;
              fresh randomness every session, so no two sessions replay the same motion
```

The idle hand pauses while an action runs or a mouse button is held, and only runs during a
session. `wait` with `until_settled` ignores the cursor, so the drift never counts as
the page still changing.

To study the real event stream, start with `VINPUT_TRACE` set (see
[section 7](#7-configuration)); it appends JSONL records with `timestamp_us`, `event`, `a`, and
`b` fields for pointer positions, buttons, keys, and wheel notches. The eval lab also reads older
`ms kind a b` logs. Live records have no action boundaries, so their pointer-position stream
cannot distinguish movement from idle or click drift; the Rust trace exporter provides tagged
per-action samples for regression tests.

### Desktop controls

Everything a person can do on this desktop, the agent does the same way:

```
 top bar (click)     [ Browser ] [ Terminal ] [ Apps ]            clock
 ──────────────────────────────────────────────────────────────────────────
 super+b             go to the browser; reopens it if it was closed
 super+Return        terminal (a shell inside the container)
 super+d             app launcher
 super+shift+q       close the focused window
 super+f             fullscreen            super+shift+space  float
 super+arrows        move focus            super+shift+arrows move window
 super+1..4          switch workspace      super+shift+1..4   send window there
 super+w / super+e   tabbed / split layout
```

**Closed the browser by accident?** Click **Browser** in the top bar (in the live view with
take over, or have the agent click it), or press super+b. `open_url` does this on its own.

Run `tools/list` for the full JSON schemas.

---

## 7. Configuration

Set these in your shell or in a `.env` file next to `docker-compose.yml`. Compose reads `.env`
on its own.

| Variable | Default | What it does |
|---|---|---|
| `TABOOM_MCP_PORT` | `3456` | Host port for MCP, the home page and recordings. The container always listens on 3456 inside. |
| `TABOOM_VIEW_PORT` | `6080` | Host port for the live view. The container always listens on 6080 inside. |
| `TABOOM_PUBLISH` | `127.0.0.1` | Host interface for both ports. `0.0.0.0` exposes them to your network. |
| `TABOOM_PUBLIC_URL` | `http://localhost:<MCP port>` | Base of recording share links. Set it to the address other people use to reach this box. |
| `TABOOM_MCP_TOKENS` | empty | Bearer tokens, `name=token,name2=token2`. Empty means no auth. |
| `TABOOM_PERSONA` | `default` | The persona this container runs: `personas/<name>.toml` in the data volume. |
| `TABOOM_VAULT_PASSPHRASE_FILE` | `/run/secrets/taboom_vault_passphrase` | File holding the vault passphrase, read at boot when the route has `auth = "vault:<name>"`. |
| `TABOOM_ROUTE_RECHECK_S` | `300` | Seconds between route re-checks (minimum 30). |
| `TABOOM_RECORDINGS_MAX_GB` | empty | Cap on the recordings folder. Oldest finished sessions go first, checked at each session start. Empty keeps everything. |
| `TABOOM_ROUTE_ECHO_URL` | `https://api.ipify.org` | HTTPS service that answers with the caller's IP, used to find the exit IP. Not in the Compose file; add it under `environment` to change it. |
| `VINPUT_TRACE` | empty | A path inside the container, e.g. `/home/taboom/.taboom/logs/input-trace.jsonl`. Appends JSONL mouse, button, key, and wheel events. Off when empty. |

Example `.env`:

```bash
TABOOM_MCP_PORT=4456
TABOOM_VIEW_PORT=7080
TABOOM_MCP_TOKENS=claude=change-me-1,cursor=change-me-2
```

With that file the links become `http://localhost:4456/` (home and MCP at `/mcp`) and
`http://localhost:7080/` (live view). `TABOOM_PUBLIC_URL` and the live view link inside the
container are derived from these, so you only set the ports.

Apply changes with `docker compose up -d` (no rebuild needed).

### Personas

A persona is one TOML file in the data volume, `/home/taboom/.taboom/personas/<name>.toml`, and
it is the single source of truth for the container. `TABOOM_PERSONA` picks it. On first boot a
missing file is written with US defaults, a direct route, a screen drawn from common hardware
and a random humanizer seed, so `docker compose up` always has something valid to run.

```toml
name = "shop"
country = "DE"                    # unset timezone/identity fields default from it
timezone = "Europe/Berlin"        # must be one of the country's zones (tzdata zone.tab)
# cpus = 4                        # Chrome: set TABOOM_CPUSET=0-3 in .env; Fortress: override
# ram_mb = 8192                    # Chrome: must match host deviceMemory; Fortress: override

[identity]
locale = "de_DE.UTF-8"            # region must be the country
languages = ["de-DE", "de", "en"] # first one matches the locale's language
keyboard_layout = "de"            # an XKB layout

[screen]
width = 2560
height = 1440
scale = 1.25

[browser]
engine = "chrome"                 # or "fortress" in a Fortress-enabled linux/amd64 image

[route]
type = "socks5"                   # or "http", or "direct" (no url)
url = "socks5://proxy.example.net:1080"
auth = "vault:shop-proxy"         # never a password in this file
# allow_datacenter = true         # accept an exit in a hosting/cloud ASN

[humanizer]
seed = 81723                      # stable typing and mouse habits
```

Countries with built-in defaults: US, GB, DE, FR, JP, BR, IN. For others set `timezone` and all
of `[identity]`. Unknown keys (including `[hardware]`, `accept_languages`,
`humanizer_style` and plain `username`/`password`) are refused, so a stale file fails boot with
the list of problems instead of running half-applied. To start over from defaults, delete the
file and restart.

How each field reaches the runtime:

```
 field              applied by                                        checked
 ─────────────────  ────────────────────────────────────────────────  ─────────────────────────
 timezone           TZ for sway, bar and browser; Fortress              zone.tab vs country
                    also receives --uxr-timezone
 country            route exit check; Fortress --uxr-country              ISO 3166 uppercase
 locale, languages  LANG + LANGUAGE, Chrome --lang and                  locale/country/first
                    Preferences, Fortress --uxr-languages                language agree
                    before Chrome starts
 keyboard_layout    XKB_DEFAULT_LAYOUT for vinput, sway xkb_layout      XKB layout exists
 screen             sway output mode WxH scale S; Fortress              800-7680 x 600-4320, 1-3
                    --uxr-screen-width/height
 languages          fontconfig exposes only fonts that fit them         (Latin set + CJK, Arabic,
                    (the image carries more)                             Hebrew, Devanagari, Thai)
 cpus               Chrome: Compose cpuset; Fortress:                  Chrome boot fails if cpuset
                    --uxr-hw-concurrency                                differs; Fortress > 0
 ram_mb             Chrome: host value only; Fortress:                  Chrome value must map to
                    --uxr-device-memory                                 host deviceMemory; > 0
 humanizer.seed     mouse/typing style; Fortress                        stable derived seed if unset
                    --uxr-canvas-seed/--uxr-audio-seed
 browser.engine     taboom-browser and the private-pipe launcher         Chrome default; Fortress
                                                                        build and arch required
 route              see "Network route" below                           exit IP at boot + re-checks
```

Chrome's `navigator.deviceMemory` and `hardwareConcurrency` are based on the hardware visible to
the container. If a persona sets `cpus`, set `TABOOM_CPUSET` in the Compose `.env` file to the
matching host CPU ids (for example, `0-3`); CPU quotas do not change Chrome's reported count.
Current desktop Chrome rounds `deviceMemory` to the nearest power of two, resolves exact ties
downward, and clamps the result to 2–32 GB; container memory limits do not change that reading.
`persona_status` reports these values as Chrome sees them instead of pretending otherwise. An
optional `ram_mb` must map to that same Chrome value; it does not change Chrome's hardware report.
When Fortress is selected, its documented `--uxr-device-memory` override uses `ram_mb` when set,
or the host's physical memory rounded through `device_memory_gb` otherwise; Docker memory limits
do not change the fallback value.
`persona_status` also compares the declared values with what the running desktop shows and
lists any mismatch. Edit the file, then `docker compose restart taboom` to apply it.

### Optional Fortress engine

Chrome remains the default and multi-architecture image. To include Fortress, build the image on
Linux/amd64 with the explicit build argument, then set `engine = "fortress"` in the selected
persona and restart. Set `TABOOM_HOST_ARCH` in the Compose `.env` file to the Docker host's
architecture (`amd64` or `arm64`) so boot-check can reject an amd64 image running under ARM
emulation. For a remote Docker context, use the remote daemon host's architecture. If unset, the
value is `unknown` and Fortress validation fails closed; Chrome does not need this setting.

```dotenv
TABOOM_HOST_ARCH=amd64
```

```bash
TABOOM_ENGINES=chrome,fortress docker compose build
docker compose up -d
```

The build pins Fortress v150.0.7871.114 and checks the downloaded Linux x64 archive against its
official release `SHA256SUMS` entry before extraction. Unsupported engine lists and non-amd64
Fortress targets fail with a build error. `taboom persona check` and boot-check also refuse
Fortress if the running image lacks it, its runtime architecture is not x86_64, or Compose reports
a non-x86_64 Docker host.

Taboom maps country, timezone, language list, screen width/height, CPU count and physical memory
to Fortress's documented `--uxr-*` flags. With no `cpus` value, it uses the process's visible CPU
count; an explicit `cpus` value overrides that. `humanizer.seed` (or its stable name-derived
fallback) is sent as both `--uxr-canvas-seed` and `--uxr-audio-seed`. These two seeds pin canvas and
audio noise; they do not freeze every part of Fortress's generated identity. Taboom never sends
`--user-agent`, which Fortress documents as breaking consistency between the UA string and
Client-Hints. The ordinary `TZ`, `LANG`, `LANGUAGE`, XKB and persona-filtered fontconfig setup still
applies. Taboom uses Fortress's upstream launcher for its browser setup while directing its
fontconfig choice back to Taboom's existing filtered configuration; its private CDP pipe remains
owned by `taboomd`.

The Fortress release contains an opaque persona-generation component and device model data; the
public repository does not make those internals auditable. Fortress can receive page contents and
the keystrokes Taboom uses to type vault secrets. Existing vault domain checks still apply, but
only use the browser binary and destination sites you trust. Fortress's persona flags are visible
in `/proc/<pid>/cmdline` to processes with access to the container; they contain persona values,
never vault values. Its built-in identity can still vary on surfaces Taboom does not override, so a
persistent profile does not by itself guarantee one identical fingerprint across launches.

Profiles are plain local folders (`profiles/<name>`). There is no Google account: signing in to
Chrome and sync are turned off by policy (`image/chrome-policy.json`). Sites you log in to stay
logged in.

### Network route

```
 Chrome ──socks5──> taboomd forwarder 127.0.0.1:1080 ──(credentials from the vault)──> your proxy
   │                  │                                                                   │
   │ no local DNS     └─ closed when a route check fails; no direct fallback              └─> site
   │ (names resolve at the proxy; QUIC off under SOCKS5; WebRTC only through the proxy)
```

- **Direct** (`type = "direct"`, the default): no proxy. If `GeoLite2-Country.mmdb` is in the
  data volume, the exit country must match `country`; without it the check is skipped.
- **Proxy** (`socks5` or `http`): boot needs `GeoLite2-Country.mmdb` and `GeoLite2-ASN.mmdb` in
  `/home/taboom/.taboom/`, finds the exit IP through the proxy (HTTPS echo), and refuses to start
  when the exit country is not `country` or the ASN is a known datacenter
  (`crates/taboomd/data/datacenter-asns.txt`). A dead proxy means no traffic, never a direct
  connection.
- **Re-check** every `TABOOM_ROUTE_RECHECK_S`. A failure closes the forwarder to new
  connections, shows in `persona_status`, and refuses agent actions until a check passes again.

Proxy credentials live in the vault, never in the persona file. The vault is unlocked at boot
with a passphrase from `TABOOM_VAULT_PASSPHRASE_FILE`, mounted as a Compose secret. Initialize
the vault and add the proxy credential while the persona still uses the direct route:

```bash
docker compose exec -it taboom taboom vault init
docker compose exec -it taboom taboom vault add shop-proxy --type password --domains proxy.example

# 2. put the same passphrase in a file on the host (keep it out of git)
read -s -p 'Vault passphrase: ' VAULT_PASSPHRASE; printf '\n'
printf '%s' "$VAULT_PASSPHRASE" > ./vault-passphrase && chmod 600 ./vault-passphrase
unset VAULT_PASSPHRASE
```

Then mount it with a `docker-compose.override.yml`, switch the persona's `[route]` to the proxy
with `auth = "vault:shop-proxy"`, and restart:

```yaml
services:
  taboom:
    secrets: [taboom_vault_passphrase]
secrets:
  taboom_vault_passphrase:
    file: ./vault-passphrase
```

GeoLite2 databases come from MaxMind (free account). Copy them in with
`docker compose cp GeoLite2-Country.mmdb taboom:/home/taboom/.taboom/` (and the ASN one).

### Several personas on one host

Each persona is its own Compose project: its own container, its own data volume (Compose
prefixes the volume with the project name), its own ports.

```bash
TABOOM_PERSONA=shop TABOOM_MCP_PORT=3457 TABOOM_VIEW_PORT=6081 docker compose -p shop up -d
TABOOM_PERSONA=work TABOOM_MCP_PORT=3458 TABOOM_VIEW_PORT=6082 docker compose -p work up -d
```

```
 project "shop"  ->  container shop-taboom-1, volume shop_taboom-data, :3457 MCP, :6081 view
 project "work"  ->  container work-taboom-1, volume work_taboom-data, :3458 MCP, :6082 view
```

Pass the same `-p` (and variables) to every later command, e.g.
`docker compose -p shop logs -f`. To give a project its persona file before first boot, copy it
in and restart: `docker compose -p shop cp shop.toml taboom:/home/taboom/.taboom/personas/`.
Containers on one host share its hardware, which is real, so no persona pretends otherwise.

### What persists

```
 volume           mounted at                      holds
 ──────────────   ─────────────────────────────   ──────────────────────────────────────
 taboom-data      /home/taboom/.taboom            persona file, Chrome profile (profiles/),
                                                  vault, GeoLite2 databases, audit log,
                                                  recordings, logs
```

Logins survive `docker compose down` and rebuilds. Only `docker compose down -v` wipes them.

---

## 8. Auth and remote access

By default both ports are published on `127.0.0.1` only. Only programs on your own machine can
reach them.

To let other machines (or several agents with separate identities) connect:

```
 1. set tokens            TABOOM_MCP_TOKENS=claude=tok-a,cursor=tok-b
 2. publish widely        TABOOM_PUBLISH=0.0.0.0
 3. restart               docker compose up -d
 4. each client sends     Authorization: Bearer tok-a
```

Each token name is its own session holder. Without tokens every client is `anonymous` and they all
share one identity, so two agents can step on each other.

A request without a valid token gets `401 unauthorized`. The server speaks plain HTTP, so if it
crosses an untrusted network put it behind a TLS reverse proxy (Caddy, nginx) or a private
network like Tailscale.

Tokens protect `/mcp` only. The live view on `:6080` has no password, and anyone holding a
recording share link can open that replay. On `0.0.0.0`, put port 6080 behind the same private
network or proxy auth, and set `TABOOM_PUBLIC_URL` so share links point at the right host.

---

## 9. Day-to-day commands

```bash
docker compose up -d --build     # start, rebuilding if code changed
docker compose up -d             # start with the existing image
docker compose ps                # status and health
docker compose logs -f taboom    # follow logs
docker compose restart taboom    # re-apply the persona file, clear a stuck browser
docker compose down              # stop, keep data
docker compose down -v           # stop and WIPE persona file, vault, browser profile and privacy marker
docker compose exec taboom bash  # shell inside the container
```

### The `taboom` CLI

The image includes a local operator CLI. Run it with `docker compose exec taboom taboom ...`.
Vault passphrases and secret values are read at a hidden terminal prompt; use an interactive
exec (`-it`) and do not put them in command arguments, shell history, or MCP calls.

```bash
docker compose exec -it taboom taboom vault init
docker compose exec -it taboom taboom vault unlock
docker compose exec -it taboom taboom vault add github --type password --domains github.com
docker compose exec taboom taboom vault ls
docker compose exec taboom taboom vault rm github
docker compose exec taboom taboom vault lock
docker compose exec taboom taboom vault status

docker compose exec taboom taboom persona new shop --country DE
docker compose exec taboom taboom persona show
docker compose exec taboom taboom persona check
docker compose exec taboom taboom status
docker compose exec taboom taboom logs -n 100
docker compose exec taboom taboom recordings ls
docker compose exec taboom taboom recordings share RECORDING_ID
docker compose exec taboom taboom call open_url '{"url":"https://example.com"}'
docker compose exec taboom taboom gauntlet
```

`persona new` creates a validated file without replacing an existing one. Restart the container
to apply it. `vault add --domains` accepts comma-separated domain names. `taboom call` invokes the
MCP tool directly; it does not send the call through an LLM. If multiple MCP tokens are configured,
choose one stable session identity with `docker compose exec -e TABOOM_CLI_CLIENT=agent-a taboom taboom ...`.
With one configured token the CLI selects it automatically; with no tokens it uses `anonymous`.
The manual `gauntlet` command takes the desktop lease, visits CreepJS, sannysoft, BrowserScan,
and pixelscan, then saves a PNG and local OCR text for each page under `lab/gauntlet/`. It appends
persona, engine and commit metadata to `lab/scoreboard.json`. It requires screen capture and OCR;
if secret typing has disabled visual capture, it releases the session and stops before opening a
detector page. It also refuses to run while another desktop session is active. Compose mounts
`lab/` read-write and `.git` read-only; when a worktree's `.git` file points outside that mount,
set `TABOOM_GIT_COMMIT` to include the commit. Otherwise the scoreboard records `unknown` and the
command reports that Git metadata was unavailable.
Use `taboom --help` and `taboom vault --help` for the complete command list.

See what the browser sees without an agent:

```bash
docker compose exec taboom grim /tmp/s.png && docker compose cp taboom:/tmp/s.png ./screen.png
```

---

## 10. Troubleshooting

```
 symptom                               likely cause / fix
 ────────────────────────────────────  ──────────────────────────────────────────────────
 container keeps restarting            logs say "sway did not start": check that
                                       security_opt seccomp:unconfined is still set
 claude mcp list: failed to connect    container not healthy yet, or wrong port.
                                       curl the smoke test from section 1
 container exits: "boot-check failed"   the log lists every problem in the persona file,
                                       or the route failure. Fix it and restart
 "no active session; call              the agent skipped session_start. Tell it to
  session_start first"                 start a session first
 "held by 'anonymous'"                 clients without tokens all share "anonymous".
                                       Call session_end, then session_start again
 "held by '<other name>'"              another agent has the desktop. End it there, or
                                       run another persona in another project
 "route check failed"                  the exit IP moved country, hit a datacenter ASN,
                                       or the proxy died. persona_status shows why;
                                       actions resume after a passing re-check
 401 unauthorized                      tokens are set; add the Authorization header
 port 3456 or 6080 already in use      set TABOOM_MCP_PORT / TABOOM_VIEW_PORT in .env,
                                       docker compose up -d, re-add the MCP client
                                       with the new URL
 browser tabs crash / "Aw, Snap!"      raise shm_size in docker-compose.yml
 blank or black screenshots            docker compose restart taboom
 live view page loads but stays grey   docker compose logs taboom | grep wayvnc; restart
 browser window is gone                click Browser in the top bar, or key super+b
 "could not read the keyboard layout"  vinput is not running; docker compose restart
 "take a new screenshot"               the screen changed or frame_id is old. Screenshot,
                                       then use the new coordinates
 "<secret> placeholders refused"       check that the vault is unlocked, the hostname is allowed, Chrome is
                                       focused and not fullscreen, the visible omnibox matches an HTTPS page host,
                                       and no tabs on other sites or blank/data: popups are open
screenshots disabled after secret    expected: reset the persistent data volume to re-enable visual output
 Apple Silicon build is slow           expected on first build; later builds are cached
```

---

## 11. Developing

The Docker image is the supported way to run. Native builds are for working on the code.

```bash
cargo build --release            # builds the workspace binaries taboomd and taboom
cargo test                       # all Rust tests
cd lab && pip install -e ".[dev]" && pytest   # eval lab (Python)
```

After changing Rust code, rebuild the container: `docker compose up -d --build`.

The Docker-only architecture and development notes live in [`PLAN.md`](PLAN.md).

---

## 12. Repo map

```
 crates/
   taboom-core/        shared persona types, consistency checks, admin protocol
   taboom-cli/         local operator CLI (`taboom`)
   taboomd/            daemon: MCP server (src/mcp.rs), tools (src/handler.rs),
                       persona file (src/persona.rs), boot-check (src/boot.rs),
                       route forwarder + checks (src/route.rs), recordings
                       (src/recording.rs), session lease, vault, audit
   taboom-humanizer/   human-like mouse and typing models
 docker/               Dockerfile, entrypoint, sway config, vinput (virtual mouse + keyboard),
                       taboom-browser (starts Chrome with the persona's flags from run/env)
 docker-compose.yml    the one-command stack
 image/                Chrome policy installed in the container
 skills/               agent playbook (SKILL.md) and per-site notes
 lab/                  Python eval lab: recorder, detector, model fitting
 PLAN.md               Docker-only architecture and development notes
```

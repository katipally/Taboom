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
 │   taboomd  (MCP server, leases, recordings, audit log)   :3456   │
 │      │                                                           │
 │      ├── grim ─────────> screenshots                             │
 │      ├── vinput ───────> one virtual mouse + keyboard            │
 │      │                   (moves and keystrokes timed by the      │
 │      │                    humanizer: curves, rhythm, typos)      │
 │      ├── wl-copy ──────> clipboard                               │
 │      └── wf-recorder ──> session video                           │
 │                                                                  │
 │   sway (headless Wayland desktop, top bar with launchers)        │
 │      ├── Google Chrome  (one profile folder per persona)         │
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

**Runtime status:** Docker Compose is the supported end-to-end runtime. The QEMU VM crates and
the cloud-init/systemd provisioning files under `image/` are incomplete development work:
`taboom image build` is currently a placeholder, and the host-to-guest control path is not wired
end to end. Use the Docker quick start below to run Taboom; the VM architecture in `PLAN.md`
describes the intended design.

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
   ├─ 1. sway starts         headless Wayland desktop + top bar
   ├─ 2. vinput starts       one persistent virtual mouse + keyboard
   ├─ 3. Chrome opens        on the last active persona's profile
   ├─ 4. live view starts    wayvnc + noVNC on :6080
   ├─ 5. default persona     written once to the data volume
   ├─ 6. links printed       "Taboom is up" banner in the logs
   └─ 7. taboomd starts      MCP server listening on :3456
                                    │
                                    ▼
                          healthcheck goes "healthy"
```

Check it:

```bash
docker compose ps                  # STATUS should say (healthy)
docker compose logs taboom | grep -A6 "Taboom is up"
```

The logs print clickable links:

```
  ┌─ Taboom is up ──────────────────────────────────────────────────────────
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
| **6080** | `/` | Redirects to the live view |
| 6080 | `/vnc.html?autoconnect=1&resize=scale&view_only=1` | Live view, watch only |
| 6080 | `/vnc.html?autoconnect=1&resize=scale` | Live view with mouse and keyboard (take over) |

Inside the container only, never published: wayvnc on `127.0.0.1:5900` (noVNC's backend) and
the input daemon's socket at `/run/user/1000/taboom-input.sock`.

To change the ports, set `TABOOM_MCP_PORT` and/or `TABOOM_VIEW_PORT` (see
[section 7](#7-configuration)). Every link follows: the banner, the home page, `view_url`,
handoff links, and recording share links. Re-add your MCP client with the new URL.

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

Any client that supports HTTP MCP servers can use it directly. For clients that only do stdio,
use the bundled bridge (see "stdio-only clients" below).

### Claude Code

```bash
claude mcp add --transport http --scope user taboom http://localhost:3456/mcp
claude mcp list          # taboom should show as connected
```

`--scope user` makes it available in every project. Use `--scope project` to write it into this
repo's `.mcp.json` so teammates get it too.

If the `taboom` CLI is installed on your machine, this does the same thing:

```bash
taboom connect claude-code
```

With a token (see [section 8](#8-auth-and-remote-access)):

```bash
claude mcp add --transport http --scope user taboom http://localhost:3456/mcp \
  --header "Authorization: Bearer <your-token>"
```

### Cursor, VS Code, Claude Desktop, other JSON-config clients

Most clients take an `mcpServers` block. Print one with `taboom connect generic`, or paste this:

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

### stdio-only clients

Some clients can only launch a local command. `taboom stdio` reads JSON-RPC on stdin and
forwards each message to the HTTP server:

```json
{
  "mcpServers": {
    "taboom": {
      "command": "taboom",
      "args": ["stdio", "--host", "127.0.0.1", "--port", "3456"]
    }
  }
}
```

This needs the `taboom` binary on your PATH (see [section 11](#11-developing)) and `curl`.

### Your own code

It is plain JSON-RPC 2.0 over POST. The flow any client follows:

```
 client                                   taboom
   │ POST initialize ───────────────────────>│
   │<───────────── protocolVersion, tools cap │
   │ POST notifications/initialized ────────>│  202, no body
   │ POST tools/list ───────────────────────>│
   │<──────────────────────── 34 tools        │
   │ POST tools/call persona_acquire ───────>│
   │ POST tools/call screenshot ────────────>│
   │<─────────────── image block (PNG)        │
   │ POST tools/call click / type / ... ────>│
```

Any official MCP SDK (Python, TypeScript, etc.) works with its streamable HTTP client.

---

## 3. Your first session

Every agent must **hold a lease on a persona** before it can touch the screen. A persona is one
browser identity. The container ships with one called `default`.

```
 persona_list ──> persona_acquire {id} ──> screenshot / click / type ... ──> persona_release
                        │
                        └─ one holder at a time; a second agent gets "already held"
```

Try it in Claude Code after connecting:

```
Use taboom. Acquire the "default" persona, open https://example.com,
take a screenshot, and tell me what the page says.
```

The agent will call, in order:

| Step | Tool | Arguments |
|---|---|---|
| 1 | `persona_acquire` | `{"id": "default"}` |
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

For better agent behavior, load the playbook in [`skills/SKILL.md`](skills/SKILL.md) into your
agent's instructions (Claude Code: copy it into a skill or your `CLAUDE.md`). It covers popups,
loop detection, secrets, and when to hand off to a human. Site notes live in `skills/sites/`.

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

- `view_only=1` means you watch without touching. Drop it from the URL to take over the mouse and
  keyboard, for example to solve a CAPTCHA or type a 2FA code, then hand back.
- `resize=scale` fits the desktop to your window at any size, phone included.
- The agent can fetch both links itself with the `view_url` tool (`url` to watch,
  `takeover_url` to control), and `handoff_start` returns the take-over link.
- It has no password. Keep it on `127.0.0.1` (the default) or behind a private network. See
  [section 8](#8-auth-and-remote-access).

---

## 5. Recordings

Every session is recorded automatically, as a **screen video** plus a **step log with a frame
per action**. A session starts at `persona_acquire` and ends at `persona_release`.

```
 persona_acquire ─┬─ screenshot ─┬─ click ─┬─ type ─┬─ ... ─┬─ persona_release
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

Recordings are never deleted automatically. Frames take about 150-250 KB per visual step and
video roughly 1-2 MB per busy minute (idle screens add almost nothing), so clear old ones out of
`~/.taboom/recordings/` when disk gets tight.

What is not recorded: file contents from `files_put` (only their size). `<secret>` placeholders
are stored as the placeholder, never the value.

---

## 6. Tools reference

```
 SESSION              SEE                 MOUSE                  KEYBOARD
 ────────────────     ───────────────     ──────────────────     ─────────────────
 persona_list         screenshot          click                  type
 persona_create       zoom                move                   key
 persona_acquire      cursor_position     drag                   key_down
 persona_status                           scroll                 key_up
 persona_release                          mouse_down             hold_key
                                          mouse_up               open_url **

 HUMAN HANDOFF        RECORDINGS          OTHER
 ────────────────     ───────────────     ──────────────────────────────────────
 handoff_start        recording_list      wait, files_put, files_get, files_list,
 handoff_wait         recording_get       clipboard_get, clipboard_set, view_url,
 handoff_resolve      recording_share     computer *

 34 tools in total.
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
| `persona_acquire` | `id` | Lease a persona. Needed before any screen, mouse, keyboard, file or clipboard tool. One persona at a time; if its profile is not the one open, the browser closes and reopens on it (the result has `profile` and `browser`). |
| `persona_create` | `name` | New persona from Taboom's built-in defaults, usable at once. Optional `timezone`, `locale`, `keyboard_layout`, `languages`, `accept_languages`. Name is 1-64 of `A-Z a-z 0-9 _ -`; existing names are refused. Returns failed consistency checks as `warnings`. No lease needed. |
| `screenshot` | none | Longest edge 1280 by default; `max_edge` 200-4096. Returns `frame_id` and sizes. |
| `zoom` | `region` | `{x, y, w, h}` in screenshot coordinates. Returns a full-resolution crop. |
| `click` | `x`, `y`, `describe` | `describe` says what you mean to do, e.g. "Submit the form". Optional `button`, `count` (1-3), `frame_id`. |
| `drag` | `from`, `to`, `describe` | Left button held and moved over about 0.4 s. |
| `scroll` | `direction`, `amount` | `up`, `down`, `left`, `right`; `amount` is wheel notches. Optional `x`, `y` (default: center). |
| `type` | `text` | `mode`: `keys` (real key presses with human rhythm and the odd corrected typo), `paste` (clipboard + ctrl+v), or `auto` (keys, but pastes text over 400 chars or with characters the keyboard layout lacks). `submit: true` presses Enter after. |
| `key` | `combo` | `ctrl+c`, `Enter`, `F5`, or a sequence: `ctrl+a Delete`. Also the desktop shortcuts below. |
| `mouse_down` / `mouse_up` | none | Hold a button (optionally after moving to `x`, `y`) until released. For drawing, custom drags, range selects. |
| `key_down` / `key_up` | `key` | Hold a key, like shift while clicking. |
| `hold_key` | `key` | Hold for `duration_ms` (default 1000, max 10000). |
| `wait` | none | `ms` sleeps. `until_settled: true` waits until the screen stops changing (`ms` is then the cap, default 3000). |
| `open_url` | `url` | Opens the URL in a new browser tab. |
| `files_put` / `files_get` / `files_list` | `name` + `data_base64` / `path` / none | Files in the browser's Downloads folder. `files_get` is capped at 20 MB and never leaves that folder. |
| `clipboard_get` / `clipboard_set` | none / `text` | The desktop clipboard. |
| `handoff_start` | `reason` | Ask a human to take over (CAPTCHA, 2FA on a phone, payment). Pauses agent input and returns `view_url`, the take-over live view link. The agent then calls `handoff_resolve` with the returned `id` once the human says they are done (or `action: "abort"`). |
| `view_url` | none | `url` (watch) and `takeover_url` (control) for the live view. |

Every failure comes back as an MCP error with the reason, never as fake success. On
`persona_release` every held key and button is let go.

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
 typing       tempo varies per call and drifts across the text; longer gaps at spaces and
              after punctuation; digits and symbols slower; shift pressed a beat early;
              fast pairs overlap (rollover); occasional thinking pauses; typos on
              neighboring keys, sometimes noticed a key or two late, then backspaced
 per persona  a stable style (speed, typing habits) from the persona's name; fresh
              randomness every session, so no two sessions replay the same motion
```

The idle hand pauses while an action runs or a mouse button is held, and only runs during a
persona session. `wait` with `until_settled` ignores the cursor, so the drift never counts as
the page still changing.

To study the real event stream, start with `VINPUT_TRACE` set (see
[section 7](#7-configuration)); every pointer and key event is logged as `ms kind a b`.

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
| `VINPUT_TRACE` | empty | A path inside the container, e.g. `/home/taboom/.taboom/logs/input-trace.log`. Logs every mouse and key event with a timestamp. Off when empty. |

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

Personas are TOML files in the data volume at `/home/taboom/.taboom/personas/<name>.toml`. The
default one is created on first start. Add one with the `persona_create` tool (no restart):

```
 persona_create {"name": "work", "timezone": "Europe/Berlin", "languages": ["de", "en"]}
   └─> ~/.taboom/personas/work.toml        unset fields use Taboom's built-in defaults
```

In Docker mode, acquiring a persona switches Chrome to that persona's profile and selects an
input style based on its name. Other persona settings—including timezone, locale, keyboard layout,
languages, screen, CPU/RAM, hardware, humanizer style, and route—are saved in the config but are not
applied to the container runtime yet.

Hand-edited or copied TOML files are read at startup, so restart after changing one by hand.

Each persona gets its own Chrome profile folder, created the first time it is acquired:

```
 persona_acquire "work"
   ├─ Chrome open on another profile? close its windows (tabs and history are saved)
   ├─ ~/.taboom/run/active-persona  <- work
   └─ super+b  ->  Chrome on ~/.taboom/profiles/work     (cookies, logins, history, tabs)

 persona_acquire "default"   ->  back to ~/.taboom/profiles/default, its tabs restored
```

- Profiles are plain local folders. There is no Google account: signing in to Chrome and
  sync are turned off by policy (`image/chrome-policy.json`). Sites you log in to stay logged
  in, per persona.
- One container is one desktop and one browser, so only one persona is active at a time.
  Acquiring another while one is held is refused until it is released.
- All personas in a container share its IP address and its hardware and browser fingerprint.
  Separate profiles keep cookies apart; they do not make the personas look like different
  machines.

### What persists

```
 volume           mounted at                      holds
 ──────────────   ─────────────────────────────   ──────────────────────────────────────
 taboom-data      /home/taboom/.taboom            personas, Chrome profiles (profiles/),
                                                  vault, audit log, recordings, logs
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

Each token name is its own lease holder. Without tokens every client is `anonymous` and they all
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
docker compose restart taboom    # reload personas, clear a stuck browser
docker compose down              # stop, keep data
docker compose down -v           # stop and WIPE personas, vault, browser profile
docker compose exec taboom bash  # shell inside the container
```

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
 "no active lease; call                the agent skipped persona_acquire. Tell it to
  persona_acquire first"               acquire "default" first
 "already held by anonymous"           you already hold it (clients without tokens are
                                       all "anonymous"). Just keep going, or call
                                       persona_release then acquire again
 "already held by <other name>"        another agent has it. Release there, or add a
                                       second persona
 401 unauthorized                      tokens are set; add the Authorization header
 port 3456 or 6080 already in use      set TABOOM_MCP_PORT / TABOOM_VIEW_PORT in .env,
                                       docker compose up -d, re-add the MCP client
                                       with the new URL
 browser tabs crash / "Aw, Snap!"      raise shm_size in docker-compose.yml
 blank or black screenshots            docker compose restart taboom
 live view page loads but stays grey   docker compose logs taboom | grep wayvnc; restart
 browser window is gone                click Browser in the top bar, or key super+b
 "runs one persona at a time"          another persona is held. persona_release it first
 "could not open .../profiles/<name>"  Chrome would not quit (e.g. a "Leave site?"
                                       dialog). Close it through the live view, then
                                       acquire again. Chrome log: ~/.taboom/logs/chrome.log
 "take a new screenshot"               the screen changed or frame_id is old. Screenshot,
                                       then use the new coordinates
 "<secret> placeholders need the       the vault is not wired into Docker mode yet. Type
  vault"                               the value yourself through the live view
 Apple Silicon build is slow           expected on first build; later builds are cached
```

---

## 11. Developing

The Docker image is the supported way to run. Native builds are for working on the code.

```bash
cargo build --release            # builds taboomd and the taboom CLI into target/release
cargo test                       # all Rust tests
cd lab && pip install -e ".[dev]" && pytest   # eval lab (Python)
```

Install the CLI for `taboom connect` and `taboom stdio`:

```bash
cargo install --path crates/taboom-cli
```

After changing Rust code, rebuild the container: `docker compose up -d --build`.

The full design, invariants, and build plan live in [`PLAN.md`](PLAN.md).

---

## 12. Repo map

```
 crates/
   taboomd/            daemon: MCP server (src/mcp.rs), tools (src/handler.rs),
                       recordings (src/recording.rs), leases, vault, audit
   taboom-cli/         `taboom` binary: connect, stdio bridge, persona, vault, doctor
   taboom-proto/       host <-> guest protocol types
   taboom-vmm/         QEMU supervisor (incomplete VM path)
   taboom-guest/       guest service (incomplete VM path)
   taboom-humanizer/   human-like mouse and typing models
 docker/               Dockerfile, entrypoint, sway config, vinput (virtual mouse + keyboard),
                       taboom-browser (starts Chrome on the active persona's profile)
 docker-compose.yml    the one-command stack
 image/                cloud-init/systemd inputs for the incomplete QEMU path;
                       chrome-policy.json is also Docker's managed Chrome policy
 skills/               agent playbook (SKILL.md) and per-site notes
 lab/                  Python eval lab: recorder, detector, model fitting
 PLAN.md               architecture and roadmap
```

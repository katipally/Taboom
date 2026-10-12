# Taboom Tool Reference


Reference for all 39 Taboom tools, grouped by function. Every tool uses MCP `tools/call`.
Use `tools/list` for current schemas and defaults. Coordinates are in the latest full screenshot's
pixel space; crops and zoom do not establish another coordinate space.

## Tool Groups


### Session lifecycle

| Tool | Purpose |
|------|---------|
| `session_start` | Lease the desktop, start recording, load keyboard layout. Always call first. |
| `session_end` | Release desktop, stop recording, release all held keys/buttons. Always call last. |
| `persona_status` | Check persona identity, applied settings, route health. Works without a session. |
| `view_url` | Get the live VNC view URL for a human to watch or take over. |

### Persona files


| Tool | Purpose |
|------|---------|
| `persona_generate` | Create a candidate persona file with `name` and `country`; does not switch the live desktop. |
| `persona_list` | List persona files in this container's data volume. |

### Ordered action batches


| Tool | Purpose |
|------|---------|
| `batch_actions` | Execute 1–64 supported native steps from a full `frame_id`, stop on first error and return completed-step details plus a final screenshot. |

### Visual (reading the screen)

| Tool | Purpose |
|------|---------|
| `screenshot` | Capture the screen. Returns a scaled image (max_edge default 1280). All coordinates reference this image's frame_id. |
| `zoom` | Inspect a region at native scale subject to its edge cap. Does not change the coordinate space. |
| `read_text` | OCR the full screen or a region. Returns text lines with bounding boxes. |
| `find_text` | Search for specific text via OCR. Returns matching lines and their positions. |
| `cursor_position` | Get the current cursor coordinates. |

### Mouse input

| Tool | Purpose |
|------|---------|
| `click` | Click at (x, y). Requires `describe`. Optional: `button` (left/right/middle), `count` (for double-click), `frame_id`. |
| `move` | Move cursor to (x, y) without clicking. |
| `drag` | Drag from one point to another. Requires `from`, `to`, `describe`. |
| `scroll` | Scroll at position. `direction` (up/down/left/right), `amount` (wheel notches). |
| `mouse_down` | Press and hold a mouse button. For custom drags, drawing, selecting. |
| `mouse_up` | Release a held mouse button. |

### Keyboard input

| Tool | Purpose |
|------|---------|
| `type` | Type into the focused element. Default `auto`; `keys` uses layout-aware strokes and `paste` uses the clipboard. Use `<secret>name</secret>` for exact vault entry. |
| `key` | Press a key combo: `ctrl+c`, `Enter`, `F5`, `alt+Tab`. Space-separated for sequences: `ctrl+a Delete`. |
| `key_down` | Hold a key until `key_up`. For shift+click range selects. |
| `key_up` | Release a key held with `key_down`. |
| `hold_key` | Hold a key for `duration_ms` then release. Max 10000ms. |

### Waiting

| Tool | Purpose |
|------|---------|
| `wait` | Wait by time (`ms`), until screen stops changing (`until_settled: true`), or until text appears (`until_text`). |

### Navigation

| Tool | Purpose |
|------|---------|
| `open_url` | Open a URL in a new browser tab. Uses keyboard: focuses browser, opens tab, types URL, presses Enter. |

### Files and clipboard

| Tool | Purpose |
|------|---------|
| `files_put` | Upload a file (name + base64 data) for use in file dialogs. |
| `files_get` | Read a file within Downloads, by relative name or an absolute path inside that folder; max 20 MB. |
| `files_list` | List files in the downloads directory. |
| `clipboard_get` | Read clipboard contents. |
| `clipboard_set` | Set clipboard contents. |

### Challenge handling

| Tool | Purpose |
|------|---------|
| `detect_challenge` | OCR cue classification with type/suggestion. Confirm the actual challenge in a screenshot. |
| `challenge_click_hold` | Click and hold with micro-movements. For checkboxes and press-and-hold. |
| `challenge_slider` | Drag a slider with overshoot and correction. For slide-to-verify. |
| `challenge_select` | Click multiple targets in sequence with human pacing. For image grids and icon selection. |
| `challenge_handoff` | Return watch/control URLs immediately. The agent coordinates user help; no automatic waiting or lease transfer. |

### Recordings

| Tool | Purpose |
|------|---------|
| `recording_list` | List recorded sessions, newest first. |
| `recording_get` | Read a recording's steps. With `step`: one step + frame. With `frames: true`: step images. |
| `recording_share` | Signed replay/video links, default 7 days; optional `expires_in_s` is clamped to 60 seconds–90 days. |

### Compatibility

| Tool | Purpose |
|------|---------|
| `computer` | Anthropic computer-use compatible wrapper. Translates to native tools. Prefer native tools. |

## Key rules


1. **Ground a sequence in a full screenshot.** Batch predictable visible actions; observe again when the state becomes uncertain.
2. **Describe your intent.** `click` and `drag` require `describe` -- say what you are doing, not what you are clicking.
3. **Verify after acting.** Verify a meaningful sequence with its final screenshot; use batch_actions for predictable visible form fields.
4. **Secrets use placeholders.** Never type passwords directly. Use `<secret>name</secret>`.
5. **Live perception continues after secrets.** Screenshots, zoom and OCR remain available and may show visible credentials. Saved frames and video resume afterward and may contain visible credentials.
6. **Coordinates are scaled.** Screenshots scale to max_edge 1280 by default. Click in that space; Taboom maps to native resolution.
7. **One session at a time.** Only one agent can hold the desktop. `session_start` fails if another agent has it.

## Continuous flow


`batch_actions { frame_id, actions }` executes 1–64 ordered native actions and returns a final
screenshot. Steps include their `tool` name and normal arguments. Supports click, move, type, key,
scroll and wait. Plan from a fresh full screenshot, batch known visible fields, verify, then submit
separately. Stop before unknown states. On error, resume after completed actions instead of replaying.

Desktop, visual, file, clipboard, live-view and challenge tools require your active session and a
healthy route. Persona status/file tools and recording tools work without a lease. End the session
even if the route fails. `wait` can return `found: false` or `settled: false`; inspect result fields.

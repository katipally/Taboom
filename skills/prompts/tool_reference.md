# Taboom Tool Reference

Complete guide to all 36 Taboom tools, grouped by function. Every tool uses the MCP
`tools/call` method. Coordinates are always in the latest screenshot's pixel space.

## Tool Groups

### Session lifecycle
| Tool | Purpose |
|------|---------|
| `session_start` | Lease the desktop, start recording, load keyboard layout. Always call first. |
| `session_end` | Release desktop, stop recording, release all held keys/buttons. Always call last. |
| `persona_status` | Check persona identity, applied settings, route health. Works without a session. |
| `view_url` | Get the live VNC view URL for a human to watch or take over. |

### Visual (reading the screen)
| Tool | Purpose |
|------|---------|
| `screenshot` | Capture the screen. Returns a scaled image (max_edge default 1280). All coordinates reference this image's frame_id. |
| `zoom` | Crop a region at full native resolution. Use when screenshot text is too small. Does not change the coordinate space. |
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
| `type` | Type text into the focused element. Modes: `keys` (real key presses), `paste` (clipboard), `auto`. Use `<secret>name</secret>` for vault credentials. |
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
| `files_get` | Download a file by path from the container. |
| `files_list` | List files in the downloads directory. |
| `clipboard_get` | Read clipboard contents. |
| `clipboard_set` | Set clipboard contents. |

### Challenge handling
| Tool | Purpose |
|------|---------|
| `detect_challenge` | OCR-based challenge detection. Returns type + suggestion. |
| `challenge_click_hold` | Click and hold with micro-movements. For checkboxes and press-and-hold. |
| `challenge_slider` | Drag a slider with overshoot and correction. For slide-to-verify. |
| `challenge_select` | Click multiple targets in sequence with human pacing. For image grids and icon selection. |
| `challenge_handoff` | Signal that a challenge needs human intervention. Returns takeover URL. |

### Recordings
| Tool | Purpose |
|------|---------|
| `recording_list` | List recorded sessions, newest first. |
| `recording_get` | Read a recording's steps. With `step`: one step + frame. With `frames: true`: step images. |
| `recording_share` | Generate a signed, expiring link to a web replay of a recording. |

### Compatibility
| Tool | Purpose |
|------|---------|
| `computer` | Anthropic computer-use compatible wrapper. Translates to native tools. Prefer native tools. |

## Key rules

1. **Always screenshot first.** Every action needs coordinates from a fresh screenshot.
2. **Describe your intent.** `click` and `drag` require `describe` -- say what you are doing, not what you are clicking.
3. **Verify after acting.** Screenshot after every action to confirm the result.
4. **Secrets use placeholders.** Never type passwords directly. Use `<secret>name</secret>`.
5. **Visual capture stops after secrets.** After typing a secret, screenshots, zoom, OCR, and video are disabled for the data volume.
6. **Coordinates are scaled.** Screenshots scale to max_edge 1280 by default. Click in that space; Taboom maps to native resolution.
7. **One session at a time.** Only one agent can hold the desktop. `session_start` fails if another agent has it.

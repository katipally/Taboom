# Session Management Guide

How to manage Taboom sessions, personas, and recordings.

## Session lifecycle

Every Taboom interaction follows this pattern:

```
persona_status    (optional, check before starting)
       |
session_start     (required, always first)
       |
  [your task]     (screenshot, click, type, etc.)
       |
session_end       (required, always last)
```

### session_start
- Leases the desktop exclusively to your agent.
- Starts the step recording.
- Loads the persona's keyboard layout.
- Fails if another agent already holds the session or the route check is failing.
- You must call this before any mouse, keyboard, or visual tool.

### session_end
- Releases all held keys and mouse buttons.
- Stops the recording.
- Frees the desktop for other agents.
- Always call this when your task is done, even if you encountered errors.

## persona_status

Check the container's identity and health without needing a session.

**Returns:**
- Persona name and declared settings (timezone, languages, country).
- Applied settings and whether they match declared.
- Route health: exit IP, country, ASN, whether the route is healthy.
- Current session info (if any).

**When to use:**
- Before starting a task: verify the route is healthy.
- Debugging: check if settings are applied correctly.
- Checking if a session is already active.

**If the route is unhealthy:** Stop and tell the user. Do not try to work around it.

## view_url

Get the live desktop view URL. Two URLs are returned:
- **watch_url**: Read-only live view of the desktop.
- **takeover_url**: Interactive view where a human can control the desktop.

**When to use:**
- When you need a human to see what you are doing.
- When handing off to a human for challenge solving (challenge_handoff).
- When the user asks to watch the agent work.

## Recordings

Every session records tool calls. Saved frames and video pause during vault secret entry and resume afterward; live perception remains available.

### recording_list
Lists recorded sessions, newest first.

**Parameters:**
- `persona`: Filter by persona name.
- `limit`: Max results (default 20).

### recording_get
Read the steps and frames of a recording.

**Parameters:**
- `id`: Recording ID. Default: current session.
- `step`: Get one specific step plus its frame image.
- `from_step`: Start reading from this step number.
- `limit`: Max steps (default 50).
- `frames`: Include frame images (max 10 per call). Heavy on tokens.

**Tips:**
- Use without `frames` first to see the step log.
- Then use `step` to inspect specific frames you care about.
- Frames resume after secret typing and can include visible credentials; review before sharing.

### recording_share
Generate a signed URL for a web replay page that a human can open in their browser.

**Parameters:**
- `id`: Recording ID. Default: current session.
- `expires_in_s`: Link lifetime in seconds. Default 86400 (24 hours).

**When to use:**
- Sharing results with the user or team.
- Creating an audit trail of what the agent did.
- Debugging a failed task after the fact.

## Files and clipboard

### files_put
Upload a file into the container for use in browser file dialogs.

**Parameters:**
- `name`: Filename (e.g., "report.pdf").
- `data_base64`: Base64-encoded file content.

**When to use:** The browser shows a file upload dialog and you need to provide a file.

### files_get
Download a file from the downloads directory by its relative name or a path within that directory.

**When to use:** Retrieving a file the browser downloaded, or getting a screenshot/export.

### files_list
List files in the downloads directory.

**When to use:** Checking what files the browser has downloaded during the session.

### clipboard_get / clipboard_set
Read or write the system clipboard.

**When to use:**
- Extracting text from pages that are hard to OCR (copy with ctrl+c, then clipboard_get).
- Pasting data into forms (clipboard_set then ctrl+v for long text).

## Multi-agent coordination

Only one agent can hold the desktop at a time. If you get a session_start failure:
1. Check `persona_status` to see who holds the session.
2. Wait for the other agent to finish.
3. Do not try to force-start a session.

## Data volume persistence

The container's data volume persists across restarts:
- Chrome profile (cookies, logins, history, tabs).
- Vault secrets.
- Capture resumes after secret typing; legacy persistent capture restrictions are removed on upgrade.

Resetting the data volume (e.g., `docker compose down -v`) clears everything,
including the vault and Chrome profile.

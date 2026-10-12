# Docker-only architecture

Taboom runs as a single Docker Compose service. The container owns the Linux desktop, Chrome
instance, MCP server, and live view. Docker Compose builds and owns the runtime lifecycle.

The AI model and reasoning loop belong to an external MCP client. Taboom's research scope includes
both physical input behavior and guidance for how that agent observes, acts, and recovers. Input
models execute in the server; decision guidance is supplied through playbooks and MCP prompts and
depends on the client using them. The default mouse model uses a bundled human-derived path library for
50–800 px movements; timing and other inputs remain heuristic. Two participant groups are reserved
for evaluation and excluded from runtime calibration.

## Runtime flow

```text
MCP client -> taboomd -> LocalExecutor -> vinput / grim / sway / Chrome
                           |
                           +-> recordings, session lease, route monitor, and audit log

Browser -> noVNC -> wayvnc -> the same container desktop
```

- `docker/Dockerfile` builds the Rust binaries and desktop dependencies.
- `docker/entrypoint.sh` runs `taboomd boot-check`, then starts sway, virtual input,
  `taboomd serve`, Chrome and the live view.
- `docker-compose.yml` publishes the MCP and live-view ports and mounts the persistent data volume.
- `crates/taboomd` handles MCP requests and drives the container desktop through `LocalExecutor`.
- `crates/taboom-humanizer` supplies the mouse and typing motion used by that executor.

## Persona model

One container is one persona. `TABOOM_PERSONA` selects `personas/<name>.toml` in the data
volume (first boot writes a US direct-route default), and `taboomd boot-check` applies it before
anything else starts: TZ, LANG/LANGUAGE, Chrome `--lang` and `intl.accept_languages`, the XKB
layout for vinput and sway, sway's output mode and scale, a language-aware fontconfig with installed fallback fonts, and the
humanizer seed. Chrome's optional `cpus` requires a matching Compose `TABOOM_CPUSET`; optional
`ram_mb` is read-only and must map to Chrome's host `deviceMemory`. Fortress maps both values to
its overrides. Unknown fields fail boot. The consistency checker compares declared values with
the running desktop. Several personas on one host are
several Compose projects (`docker compose -p <name>`), each with its own volume and ports; they
share the host's real hardware.

Typing is layout-aware: at `session_start` taboomd asks vinput (`lookup`) which key and
Shift/AltGr level types each character, builds the table once, and plans keystrokes and
row-neighbor typos from it.

## Route

`boot-check` verifies the route before Chrome starts and exits non-zero on failure. Proxy routes
run through a forwarder inside taboomd on `127.0.0.1:1080` that holds the credentials (from the
vault, unlocked with a passphrase from a boot secret file); Chrome uses it as a SOCKS5 proxy with
no local DNS. The exit IP must match the persona's country and not be a datacenter ASN, and
GeoLite2 Country and ASN databases are required. Direct routes work without GeoLite2. The route is
re-checked periodically; a failure closes the forwarder and refuses actions until a check passes.
The in-image `taboom` operator CLI manages the vault over the local admin protocol (for example,
`docker compose exec taboom taboom vault init` and `vault add`); secrets never pass through MCP.

## Tool surface

The MCP server sends short operating instructions during initialization and exposes the agent
playbook and site notes as read-only resources (`taboom://skills/...`). Resource use is controlled
by the MCP client, so clients that do not load resources automatically may need an explicit request
to read the playbook. The packaged resources are allow-listed rather than resolved as filesystem
paths.

The workspace dashboard reads `/api/sessions` for searchable, status-filtered, paginated session
metadata. When MCP tokens are configured, that API requires a bearer token; the browser stores it
only in tab-scoped session storage. Replay and video links remain signed and expire after 24 hours.

The live view remains available for watching and direct user control. `view_url` returns those
links, but Taboom no longer has an agent-managed human handoff workflow. noVNC does not validate
per-session tokens; the session index API uses MCP bearer tokens when configured, and replay links
remain signed.

The MCP endpoint accepts Streamable HTTP POST requests, returns 405 for GET because it does not
offer server-sent event streams, validates supplied `Origin` headers, handles preflight requests
for allow-listed browser clients, and accepts only supported `MCP-Protocol-Version` values. MCP
and live-view host bindings are configured separately. Network publication of MCP requires bearer
tokens; the live view remains loopback-only by default.

## Data and lifecycle

The `taboom-data` volume stores the persona file, its Chrome profile, vault data, GeoLite2
databases, recordings, audit logs, and application logs under `/home/taboom/.taboom`.

```bash
docker compose up -d --build  # build and start
docker compose ps             # inspect health
docker compose logs -f taboom # follow logs
docker compose down           # stop while keeping data
docker compose down -v        # stop and remove persistent data
```

## Development focus

Keep desktop control and browser access inside the Compose container. The Docker `exec` operator
CLI, vault-backed MCP secret typing, local OCR, Rust trace CI gate, manual gauntlet, and the opt-in
Fortress engine are implemented. Fortress requires a Linux/amd64 image built with
`TABOOM_ENGINES=chrome,fortress` and `TABOOM_HOST_ARCH=amd64` in the Compose `.env`; Chrome remains
the default multi-architecture image. Other operational follow-up is tracked in `ROADMAP.md`.

## Vault perception and recordings

Vault input is serialized with agent perception. The daemon finalizes the active video clip and
invalidates pending frame captures before exact secret keystrokes, then resumes screenshots, OCR,
saved frames and video afterward. Resumed video uses separate clips and atomic concatenation to
preserve the earlier footage. Screen content is not redacted after typing: visible credentials can
be returned, saved and shared. Tool arguments retain placeholders and secret audit entries exclude
values. Legacy `run/secret-visuals-disabled` markers are removed automatically on upgrade.

## Ordered action batches

`batch_actions` holds the tool transaction lock while executing a bounded sequence of native actions.
Each step retains session, route and vault checks and its own recording entry. The server aborts on
the first error, reports completed steps, and returns a final screenshot after settling. Agents plan
from the latest full frame, batch predictable visible fields, and verify before separate submission.
Unknown UI transitions require a new observation. Decisions remain with the external agent.

## Experimental release verification (2026-10-12)

Local verification on a native ARM64 Docker desktop:

- Rust: 196 passed, 3 desktop-only tests ignored by the host suite.
- Python lab: 40 passed; fitted flight/rollover timing handles repeated keys and key-repeat events.
- Chrome: all 39 public MCP tools exercised against a synthetic visible form after restart.
  Three fields filled in one ordered batch, with OCR and clipboard verification without submitting.
- Vault: a dummy secret entered in a masked HTTPS password field without submitting. Screenshots,
  OCR, settling and saved post-secret frames resumed. Video preserved the 10.774-second earlier
  clip and appended resumed footage to produce a downloadable 17.021-second session video.
- Reserved-group shape regression: mean AUC 0.9514, 0.9433 and 0.9374 at 200, 250 and 330 ms,
  under the 0.97 ceiling. This still distinguishes synthetic and human paths and proves no human
  equivalence; timing and non-mouse inputs remain heuristic.
- Compose validation, workflow lint and Fortress architecture/install configuration checks passed.

Hosted CI has not run for these changes. The release workflow now waits for Rust/lab checks and
all-tool desktop smoke jobs for both Chrome and Fortress on native AMD64 runners. Fortress runtime
verification remains pending that job; this ARM host cannot run the required native AMD64 engine.
Fortress uses a fresh persona/profile in CI to avoid downgrading a Chrome-created profile.
Changes are prepared and committed locally; publication requires a successful hosted run and a
matching version tag pushed to the existing GitHub/GHCR destination.

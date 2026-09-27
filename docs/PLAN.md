# Docker-only architecture

Taboom runs as a single Docker Compose service. The container owns the Linux desktop, Chrome
instance, MCP server, and live view. Docker Compose builds and owns the runtime lifecycle.

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
layout for vinput and sway, sway's output mode and scale, a locale-coherent fontconfig, and the
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

The live view remains available for watching and direct user control. `view_url` returns those
links, but Taboom no longer has an agent-managed human handoff workflow. noVNC does not validate
per-session tokens; signed links are used for recordings only.

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

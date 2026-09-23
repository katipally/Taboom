# Docker-only architecture

Taboom runs as a single Docker Compose service. The container owns the Linux desktop, Chrome
instance, MCP server, and live view. Docker Compose builds and owns the runtime lifecycle.

## Runtime flow

```text
MCP client -> taboomd -> LocalExecutor -> vinput / grim / sway / Chrome
                           |
                           +-> recordings, leases, personas, and audit log

Browser -> noVNC -> wayvnc -> the same container desktop
```

- `docker/Dockerfile` builds the Rust binaries and desktop dependencies.
- `docker/entrypoint.sh` starts sway, virtual input, Chrome, the live view, and `taboomd`.
- `docker-compose.yml` publishes the MCP and live-view ports and mounts the persistent data volume.
- `crates/taboomd` handles MCP requests and drives the container desktop through `LocalExecutor`.
- `crates/taboom-humanizer` supplies the mouse and typing motion used by that executor.

## Persona model

One container has one desktop, one active Chrome window, one network route, and one hardware
profile shared by all personas. A persona selects a separate Chrome profile folder and input style.
Settings that the container runtime does not currently apply are stored as metadata and should not
be treated as active runtime configuration.

## Data and lifecycle

The `taboom-data` volume stores persona files, Chrome profiles, vault data, recordings, audit logs,
and application logs under `/home/taboom/.taboom`.

```bash
docker compose up -d --build  # build and start
docker compose ps             # inspect health
docker compose logs -f taboom # follow logs
docker compose down           # stop while keeping data
docker compose down -v        # stop and remove persistent data
```

## Development focus

Keep desktop control and browser access inside the Compose container. Focus follow-up work on
persona settings, secret entry, recordings, and live-view behavior within this runtime.

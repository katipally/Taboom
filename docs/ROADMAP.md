# Roadmap

Phases are in dependency order. Each one ships on its own.

## Decisions

```
 goal      undetectable automation, agent handles everything
 unit      1 container = 1 persona   (scale = more containers)
 browser   pluggable: stock Chrome default, fortress opt-in (x86_64)
 perceive  screenshots + local OCR   (no CDP for pages)
 secrets   vault over MCP, <secret>name</secret>, domain-gated
 routes    bring your own proxy, verified at boot, fail closed
 lab       CI trace gate + manual live gauntlet
 CLI       rebuilt slim, lives in the image, docker exec
 handoff   removed
```

Decided: the vault learns the current domain through option A, with B as a cross-check (Phase 3).

## Target architecture

```
 host$ TABOOM_PERSONA=shop TABOOM_MCP_PORT=3457 TABOOM_VIEW_PORT=6081 \
       docker compose -p shop up -d
                                │
 ┌──────────── container "shop" ▼ ────────────────────────────────┐
 │ entrypoint                                                      │
 │  1. taboomd boot-check shop  ─► reads personas/shop.toml        │
 │     ├ route: forwarder up, exit IP, geo/ASN, tz/lang match      │
 │     ├ writes run/env: TZ, LANG, XKB_*, chrome flags, fontconfig │
 │     └ any fail ─► exit 1 with the reason (Chrome never starts)  │
 │  2. sway (output mode + scale from persona)                     │
 │  3. vinput (XKB layout from persona)                            │
 │  4. taboom-browser (engine = chrome | fortress)                 │
 │  5. taboomd serve  ─► MCP :3456  +  admin socket                │
 └─────────────────────────────────────────────────────────────────┘
   agent ──MCP──► tools          you ──docker exec taboom──► admin
```

## Build order

```
 P0 hygiene ─► P1 persona runtime ─► P2 core+CLI ─► P3 vault ─► P4 OCR
                                                                  │
                                         P6 fortress ◄── P5 lab+gauntlet
```

P1.3 (layout-aware typing) is mandatory in P1, not optional.

## Phase 0: Hygiene

### 0.1 Fix the broken test build

`crates/taboomd/src/mcp.rs` does not compile under `cargo test`:

- Delete the second `tokens_from_env_spec`. It is an exact copy.
- Delete `screenshot_becomes_image_block`. It calls `mcp_content` with `&json!`, but the function now takes `&ToolResult`. `images_become_image_blocks` already covers the case.

Why first: every later phase needs `cargo test` to work.

### 0.2 Mark the desktop-dependent tests

`acquire_then_act`, `computer_tool_dispatched` and `session_is_recorded_from_acquire_to_release` spawn `swaymsg`/`grim`. Put them behind `#[ignore]` with a reason, and run them in the container with `cargo test -- --ignored`. Then `cargo test` passes on a Mac.

### 0.3 Remove dead code

| Remove | Why it's dead |
|---|---|
| `handoff.rs`, `handoff_start/wait/resolve` tools, `NotifyMethod`, the handoff check in `require_lease_then`, IPC `handoff-resolve` | No human in the loop |
| `liveview.rs` `LiveViewSession`, `LiveViewMode`, IPC `liveview-token` (hardcoded key) | noVNC never checks tokens. Move `sign_url`/`verify_token` into `recording.rs`, their only real user |
| IPC `derive-persona` (fixed seed 42), IPC `mcp-config`, `mcp::generate_*_config` | Debug leftovers. The entrypoint banner already prints connect info |
| `config.rs` `DaemonConfig` + `config.toml` loading | The struct has zero fields. Fold `home` into one env struct read once |
| `taboom-humanizer/src/idle.rs` `plan_idle` + its 2 tests | Replaced by `IdleMotion`, only called from tests |
| `consistency::check_pair_distinct` | Containers on one host truly share hardware. This check would push you to fake differences |
| `taboom-cli` crate (entire) | Rebuilt in Phase 2 |
| The duplicated doc line above `LocalExecutor::mouse_move` | Leftover from an edit |

Docs to update: `README.md` (handoff sections, persona tool table, `taboom connect/stdio`), `skills/SKILL.md` (Handoff section, the "vault not available" line), `docs/PLAN.md`.

## Phase 1: Container = persona runtime

Every persona field is either applied for real or deleted. Today they are all just metadata, and a declared value that isn't applied is a mismatch a detector can catch (declared Berlin, actual New York).

### 1.1 The persona file is the single source of truth

```toml
name = "shop"
country = "DE"                    # drives tz/lang/layout defaults + route check
timezone = "Europe/Berlin"

[identity]
locale = "de_DE.UTF-8"
languages = ["de-DE", "de", "en"]
keyboard_layout = "de"

[screen]
width = 2560
height = 1440
scale = 1.25

[browser]
engine = "chrome"                 # or "fortress"

[route]
type = "socks5"
url = "socks5://host:port"
auth = "vault:shop-proxy"         # never a plain-text password

[humanizer]
seed = 81723                      # stable habits; ignored before this change
```

### 1.2 How each field gets applied

| Field | Applied by | Notes |
|---|---|---|
| `timezone` | `TZ=` in Chrome's environment | Chrome's Intl and Date read TZ |
| `locale`, `languages` | `LANG`, `--lang=`, and `intl.accept_languages` written to `Default/Preferences` before launch | Add `locales-all` to the image. Check whether `--accept-lang` works outside headless before relying on it. If not, stick with the Preferences write |
| `keyboard_layout` | `XKB_DEFAULT_LAYOUT` for vinput + sway `input xkb_layout` | Needs 1.3, or typing breaks |
| `screen` | sway `output HEADLESS-1 mode WxH scale S` | Scale sets devicePixelRatio on Wayland |
| fonts | Bake a broad font set into the image. At boot, generate a fontconfig `<selectfont><rejectfont>` that exposes only a coherent set for the locale | Replaces `fonts_packages`, which can't be installed at boot |
| `cpus` | Compose `cpuset:` for chrome, `--uxr-hw-concurrency` for fortress | Stock Chrome reads the real core count, and `--cpus` doesn't change it |
| `ram_mb` | Read-only for chrome (report what Chrome will actually show), `--uxr-device-memory` for fortress | Docker memory limits don't change what Chrome reads |
| `humanizer.seed` | `LocalExecutor::set_persona` uses it instead of the name hash | Delete persona.rs's duplicate `HumanizerStyle`/`SpeedClass`, use the humanizer crate's types |

The consistency checker compares declared vs. applied values, not just in-range values.

### 1.3 Layout-aware typing

`typing.rs` hardcodes US evdev codes (`LETTER_KEYCODES`). With `de`, typing "y" sends KEY_Y, which prints "z".

- Add a vinput command `lookup SYM` that replies `ok CODE LEVEL` (vinput already has `keysym_to_code`).
- At session start, taboomd builds a `char -> (code, needs_shift, needs_altgr)` table once. That costs O(chars × keycodes × levels), about 100 × 250 × 4, one time instead of per keystroke.
- `plan_type` takes that table instead of the hardcoded US one, and `can_type` checks against it. Typo neighbors come from the same table, by physical row adjacency.

### 1.4 Remove persona switching

With one persona per container, all of this goes: `use_profile`, `stop_browser`, `browser_process`/`browser_profile`, the `run/active-persona` file, the persona check in `taboom-browser`, and MCP `persona_create`/`persona_list`/`persona_acquire {id}`.

Agents still need exclusive use of the desktop, and recordings need boundaries, so what remains is:

```
 session_start  -> lease the desktop, start recording + video, idle hand on
 session_end    -> release keys, stop recording
 persona_status -> which persona this container is, applied values, route health
```

`LeaseManager` shrinks to one `Option<Lease>`. It's a HashMap today, but only one entry can ever exist.

### 1.5 Multi-persona on one host

Document `docker compose -p <persona>` with `TABOOM_PERSONA` and port env vars. Compose prefixes the volume with the project name, so each persona gets an isolated volume without changing the compose file beyond reading `TABOOM_PERSONA`.

### 1.6 Route verification (boot-check)

```
 route url + vault creds
   │
 local forwarder in taboomd (127.0.0.1:1080, no auth)
   │   why: Chrome's --proxy-server can't carry credentials
   │   (it pops an auth dialog); the forwarder holds them
   ▼
 exit IP (HTTPS echo through the forwarder)
   ├ GeoLite2 country/ASN   (mmdb mounted in the volume; missing = refuse when the route is a proxy)
   ├ country ↔ tz ↔ languages   (network::timezone_matches_country, extended)
   ├ datacenter ASN          (DATACENTER_ASNS, move to a data file)
   └ Chrome: --proxy-server=socks5://127.0.0.1:1080
       SOCKS5 = remote DNS, and QUIC is disabled under SOCKS5
       WebRTC policy already disable_non_proxied_udp
       no direct fallback: a dead proxy = no traffic (fails closed)
```

Also re-check the route every N minutes. If it fails mid-session, `persona_status` reports it and new actions get refused, so the agent can't keep working on a leaking route.

## Phase 2: Shared crate + slim CLI

### 2.1 `taboom-core` crate

Persona types, `check_name`, consistency checks, admin protocol types. Both taboomd and the CLI depend on it. Removes the copy-pasted `PersonaConfig`, `derive_persona` and `SpeedClass`.

### 2.2 Admin protocol: JSON lines over the unix socket

Replace the `splitn(' ')` text protocol, which truncates any secret containing a space. Each request is `{"cmd":"vault.add","name":..,"value":..,"domains":[..]}`, typed with serde in core.

### 2.3 CLI commands

Binary in the image, run as `docker compose exec taboom taboom ...`. Why a CLI at all: some operations must never go through MCP, because anything sent over MCP passes through the model.

```
 vault   init | unlock | lock | add NAME --type --domains | rm | ls | status
 persona new NAME --country DE   (coherent draw from hardware table + country)
         show | check            (declared vs applied, route health)
 status                          (lease, session, recording, route)
 logs [-n]  recordings [ls|share ID]
 call TOOL '{json}'              (drive any MCP tool by hand, no LLM)
 gauntlet                        (Phase 5)
```

Passphrase and secret prompts read with echo off. `persona new` writes the file and then says to restart the container, since settings apply at boot.

## Phase 3: Vault over MCP

```
 agent: type "<secret>github</secret>"
   │
 handler: vault unlocked? ── no ─► error "vault locked"
   │
 current URL host ∈ allowed_domains? ── no ─► refuse + audit.secret_refused
   │ yes
 value = TOTP ? generate_totp(now) : password
   │
 pause VINPUT_TRACE ─► type (zero typos) ─► resume ─► audit.secret_used
 recording stores the placeholder only, never the value
```

### How the vault learns the current domain (decided: A, cross-checked by B)

| Option | Trust | Detectability |
|---|---|---|
| A. `--remote-debugging-pipe`, browser target only (`Target.getTargets` for the active tab URL, never attach to a page, never `Runtime.enable`) | Exact URL | The known CDP tells come from attaching to pages. A pipe has no port and never touches page targets. Verify with the gauntlet |
| B. OCR the address bar (the `ShowFullUrlsInAddressBar` policy is already set) | Can misread (rn vs m), and a fullscreen page can fake the bar | None |

Recommended: A, because this is a security gate and a misread domain means a leaked password. Keep B as a cross-check. Refuse secrets while the window is fullscreen (sway tree `fullscreen_mode`).

### Fixes

- `vault init` actually age-encrypts, instead of the CLI writing an empty file.
- `--domains` is actually saved.
- A secret with an empty domain list is refused for `type`. Today an empty list means "any domain", which is the wrong default for a password.
- The proxy password is a vault reference. At boot the vault is unlocked through `TABOOM_VAULT_PASSPHRASE_FILE` pointing at a Docker secret.

## Phase 4: OCR perception

- `find_text {text, region?}` returns `[{text, x, y, w, h, conf}]` in screenshot pixel space, reusing the `View` mapping. `read_text {region?}` returns lines.
- `wait {until_text}` next to `until_settled`, so the agent can wait for a page to finish loading without a screenshot every poll.
- Implementation: `grim` region at native scale, then `tesseract - - tsv` with language packs matching the persona locale (`tesseract-ocr-deu` and so on, chosen from `languages`).
- Why: undetectable (never touches the page), and on text-heavy pages it costs far fewer tokens than screenshots. It also backs up option B in Phase 3.
- Cost: roughly 0.3 to 1s at 1080p. Cache by `diff_frame` hash so repeated calls on an unchanged screen are free.

## Phase 5: Lab gate + gauntlet

### 5.1 The problem

`lab/taboom_lab/regression.py` re-implements the humanizer math in numpy. The tests check a copy, not the code that ships, and the two will drift.

### 5.2 Trace gate in CI

```
 crates/taboom-humanizer/examples/trace_export.rs
   N seeds × {move, click, type, scroll, idle} ─► traces.jsonl
                    │
 lab: load_rust_traces() ─► features ─► detector + regression
                    │
 AUC vs human datasets ≤ threshold?  ── no ─► CI fails
```

- Same JSONL format as `VINPUT_TRACE` output, so live container traces run through the same pipeline.
- Delete the numpy generators (`_generate_mouse_trace` and friends) once Rust traces feed the tests. Keep `_generate_bot_trace` as the negative control.
- Add `.github/workflows/ci.yml`: `cargo test`, `cargo run --example trace_export`, `pytest lab`. There is no CI today.

### 5.3 Gauntlet (manual or nightly)

`taboom gauntlet` starts a session, opens a fixed list of detector pages (CreepJS, sannysoft, BrowserScan, pixelscan), saves a screenshot and OCR text per page, and appends to the lab `scoreboard.json` with the persona, engine and commit. It's a record for spotting regressions, not a pass/fail gate, because live sites are too flaky for CI.

## Phase 6: Pluggable engine (fortress opt-in)

Comes after Phase 5 so the gauntlet can measure whether fortress beats stock Chrome for your setup.

- `taboom-browser` branches on `engine`. For fortress, map persona fields to `--uxr-*`, pin the seed to `humanizer.seed`, pin the country to `persona.country`, and never pass `--user-agent` (fortress docs warn it desyncs the UA from Client-Hints).
- `persona check` refuses `engine = "fortress"` on arm64 hosts. Fortress ships Linux x64 and Windows x64 only.
- Image: a build arg (`TABOOM_ENGINES=chrome,fortress`) so the default image stays small and multi-arch.
- Things to know going in:
  - Fortress's persona generator is closed source, and its binary will type your vault secrets.
  - Its default is a new identity every launch, which a logged-in account sees as a new device. Always pin the seed.
  - Stock Chrome in a container renders WebGL on SwiftShader, which is a strong tell. On a Linux host, passing `/dev/dri` through gives a real Mesa renderer. On macOS that isn't possible, which is the main reason to consider fortress.

## Also worth doing

| Item | Why |
|---|---|
| Recording retention (`TABOOM_RECORDINGS_MAX_GB`, oldest deleted first) | Recordings and video grow forever today |
| `/healthz` that checks vinput, sway and Chrome, not just the TCP port | The compose healthcheck says healthy while Chrome is dead |
| Audit every MCP tool call's name and client to `audit/taboomd.jsonl` | Audit is append-only and survives recording cleanup |

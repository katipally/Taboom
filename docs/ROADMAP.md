# Current roadmap


The first release is experimental. Implemented behavior is documented in
[the README](../README.md) and [the architecture](PLAN.md); this file tracks the work still needed
to validate/publish it and improve the research. The obsolete phase-by-phase build plan has been
replaced because its missing-feature descriptions no longer match the code.

## Implemented release inventory


| Area | Included in the implementation |
|---|---|
| Desktop/runtime | Docker Compose, headless Sway, Chrome, virtual input, health probes and live view |
| Agent interface | 39 MCP tools, three resources and ten prompts |
| Continuous forms | Ordered action batches with fresh-frame checks, first-error stop and final screenshot |
| Input models | Bundled human mouse geometry; heuristic timing, typing, clicks, scroll and idle behavior |
| Personas | Country/locale/layout/screen/font setup, stable seeds, synthetic initial profiles and candidate-file tools |
| Vault | Encrypted local administration, domain-gated exact entry and TOTP generation |
| Perception after secrets | Live screenshots/OCR and saved frames/video resume after entry; visible screen content remains recordable |
| Routes | Direct/SOCKS5/HTTP CONNECT configuration, credential references, verification and periodic failure gating |
| Recordings | Step logs, frames, joined video, signed replays, searchable dashboard and retention checks |
| Challenges | OCR cues, native input helpers and manual-control links |
| Optional engine | Pinned/checksummed Fortress installer with native AMD64 restrictions |
| Evaluation/release | Seeded trace regression, reserved-group shape diagnostic, all-tool browser CI and GitHub/GHCR workflow |

“Included” describes source implementation, not a claim that every runtime configuration has been
validated. The [dated verification record](PLAN.md#experimental-release-verification-2026-10-12)
separates local evidence from pending hosted evidence.

## Release gates


1. Push the intended commit for hosted CI and inspect the Rust/lab, native AMD64 Chrome and native
   AMD64 Fortress results. Native Fortress startup/input/profile behavior remains unverified locally.

2. Resolve any hosted failures and keep both browser smoke jobs required by the release workflow.
   The local ARM64 Chrome fixture already exercised all 39 tools; hosted smoke does not currently
   run an ARM64 desktop job.

3. Decide the experimental release version and update Cargo workspace/lockfile versions together.
   The pushed `v*` tag must match the workspace version.

4. Publish through the existing GitHub/GHCR workflow after successful verification. Confirm both
   Chrome image architectures are available and the GHCR package is publicly pullable.

5. Verify a clean deployment using the published pinned tag and the documented client connection.
   For a prerelease, confirm that GitHub marks it as such and `latest` remains unchanged.

The local audit did not push or publish. Publishing instructions are in
[the README](../README.md#release-workflow).

## Research follow-up


| Priority | Next evidence/work | Current limitation |
|---|---|---|
| 1 | Collect fine-grained input data with valid consent, provenance and action boundaries | Current source movement is coarse; timing is heuristic |
| 2 | Gather press/release typing, scroll, click-hold and pause data; design validated runtime calibrations | Offline typing fits are not consumed by the runtime |
| 3 | Reserve a new untouched evaluation corpus and define the protocol before tuning | p07–p08 diagnostics are reused during development |
| 4 | Compare empirical/heuristic modes across task, device, distance and sampling conditions | A shape-only AUC on task-wide logs is not click-target or task-success evidence |
| 5 | Evaluate external-agent decisions: observation, batching, uncertainty, recovery and task completion | Prompts guide clients; they do not enforce or prove human-like reasoning |
| 6 | Measure real-site/manual-gauntlet outcomes with explicit task definitions and repeated runs | A detector screenshot or successful fixture is not a general acceptance claim |

Dataset adaptations must retain attribution, license, revision, integrity checks and separation from
runtime private data. The code's AGPL license does not replace the dataset's CC BY 4.0 terms.

## Operational validation follow-up


- Expand real desktop coverage beyond the default fixture: keyboard layouts/languages, high-DPI
  scaling, resizing, multi-window navigation and both engine profiles.

- Exercise configured proxy routes with supplied credentials/databases and route changes, in
  addition to the existing protocol/unit checks.

- Make the three desktop-only Rust tests practical to run in CI alongside the public MCP smoke.
- Evaluate interrupted-session metadata, unfinished clips and recovery after unexpected container
  termination; distinguish live recording state from abandoned artifacts.

- Evaluate recording retention/finalization under storage pressure and long sessions.
- Check client behavior when prompts/resources are not loaded automatically, and keep connection
  examples aligned with official client documentation.

These are follow-up validations and research improvements, not hidden guarantees of the current
experimental release. Additional feature scope should be decided with the project owner.

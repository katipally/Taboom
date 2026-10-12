# Taboom Eval Lab


Evaluates Taboom's input models with regression tests and comparisons to human interaction data.
The results describe these specific protocols; they do not prove general human equivalence.

## Install


Use Python 3.10 or later (CI uses 3.12). Run these commands from the repository root:

```bash
python3 -m venv .venv
. .venv/bin/activate
python3 -m pip install -e "./lab[dev]"
```

For the evdev recorder (Linux only):

```bash
python3 -m pip install -e "./lab[dev,recorder]"
```

## Run tests


```bash
cargo run --locked -p taboom-humanizer --example trace_export -- lab/traces.jsonl
python3 -m pytest lab
```

The regression and detector suites consume these seeded Rust traces. The generated
`lab/traces.jsonl` file is ignored by Git. `VINPUT_TRACE` also writes JSONL in the same event
format, and the loader accepts the older whitespace-delimited trace records as well. Exported
traces carry seed and action IDs. Live `VINPUT_TRACE` has no action-boundary records, so all live
pointer positions are grouped as one stream-level movement trace; pointer drift during idle and
clicks cannot be separated after capture. Use the seeded exporter for individual move, idle, and
click-position samples.

The detector CI test requires AUC >= 0.90 when trained on 20 Rust movement traces and evaluated
on 10 held-out Rust movement traces against matched-endpoint, linear constant-interval negative
controls. This guards that synthetic regression pair; it is separate from the human-derived shape
fixture diagnostic below and is not a benchmark claim for general human-vs-bot detection.

## Modules


- `recorder.py` -- evdev event capture and task-page generator
- `datasets.py` -- loaders for Balabit, SapiMouse, BOUN, Aalto public datasets
- `features.py` -- feature extraction from mouse/keyboard/scroll traces
- `detector.py` -- RandomForest human-vs-bot classifier
- `fitting.py` -- population distribution fitting, parameter export
- `regression.py` -- Rust trace loader and one test per known bot failure mode
- `scoreboard.py` -- track scores over time
- `visualize.py` -- matplotlib plots for traces and distributions
- `shape_eval.py` -- shape-only comparison to the pinned A11y-CUA sighted-user desktop logs
- `calibration.py` -- reproducible normalized runtime mouse-path extraction from the same pinned logs

## Real-human shape evaluation


The pinned data is licensed CC BY 4.0 and used only through its documented adaptations. Runtime
mouse geometry comes from p01–p06; p07–p08 feed the reused shape diagnostic. Timing and other input
behaviors remain heuristic. See [the project overview](../README.md#human-data-research).

The pinned SU event logs and reproducible extraction protocol are documented in
[`data/README.md`](data/README.md). The checked-in fixture contains only pseudonymous participant
groups, rounded endpoint distances, and normalized shape features; source event logs stay outside
the repository.

Regenerate the seeded Rust traces, download the pinned A11y-CUA snapshot, and derive the fixture:

```bash
cargo run --locked -p taboom-humanizer --example trace_export -- lab/traces.jsonl
hf download berkeley-hci/Reduced-A11y-CUA --repo-type dataset \
  --include README.md 'SU/**/*.exe.json' \
  --revision 26ee1103f74436cb7051543ba65d0c57902c0eb6 --local-dir /tmp/reduced-a11y-cua
python3 -m taboom_lab.shape_eval \
  --human-root /tmp/reduced-a11y-cua/SU \
  --rust-traces lab/traces.jsonl \
  --write-fixture lab/data/a11y_cua_su_shape_features.json
```

To reproduce the CI diagnostic from the committed feature fixture:

```bash
python3 -m taboom_lab.shape_eval \
  --human-fixture lab/data/a11y_cua_su_shape_features.json \
  --rust-traces lab/traces.jsonl --human-groups p07 p08 --max-mean-auc 0.97
```

Rust paths are thinned using only their own timestamps at 200, 250, and 330 ms intervals before
both sides are geometrically normalized. The output reports held-out participant/seed AUC for
each cadence. CI fails when any cadence's mean AUC exceeds `--max-mean-auc 0.97`. That is a
ratchet, not a human-equivalence pass mark. The recorded 2026-10-12 results were 0.9514, 0.9433 and
0.9374 at 200, 250 and 330 ms. Chance discrimination for this classifier/protocol is 0.5; even a
chance score would not prove general human equivalence. The ceiling catches regressions; lower it
when independent evidence supports an improvement. These task-wide mouse-move runs do not
establish click-to-target equivalence.

## Runtime human-data connection


The default `empirical` mouse model loads a bundled, versioned path library compiled into the Rust
humanizer. It draws normalized human shapes of comparable distance, rotates/scales them to the
requested target, optionally mirrors them, and applies heuristic velocity and tremor. The dataset
provides geometry; it does not supply fine-grained motor timing. Keyboard, scroll, idle and
click-hold models remain heuristic. Offline typing fits are analysis outputs, not runtime models.

The library is derived from the pinned SU snapshot credited in [data/README.md](data/README.md).
Only groups p01–p06 contribute paths; p07–p08 are reserved. We retain 16 normalized points per
bounded movement of 50–800 px, exclude task detours with path/chord length over 1.2, and select up
to 64 runs per training participant by a deterministic hash order. Short runtime moves prefer
paths with path/chord length at most 1.08 to limit integer-pixel rounding detours. Absolute source
positions, timestamps, screen contents and original participant identifiers are not bundled.

Regenerate the library after downloading the pinned event logs:

```bash
PYTHONPATH=lab python3 -m taboom_lab.calibration \
  --human-root /tmp/reduced-a11y-cua/SU \
  --output crates/taboom-humanizer/data/human_mouse.json
```

The exporter checks the complete source digest before extraction. Rust validates the artifact's
schema, endpoints, finite coordinates and geometry bounds. `persona_status` exposes model and
provenance. Select `mouse_model = "heuristic"` under `[humanizer]` for the previous baseline.

CI exports the default runtime model and evaluates only reserved groups:

```bash
python3 -m taboom_lab.shape_eval \
  --human-fixture lab/data/a11y_cua_su_shape_features.json \
  --rust-traces lab/traces.jsonl --human-groups p07 p08 --max-mean-auc 0.97
```

This requires all 10 reserved-participant/seed folds at each cadence. Scores on p01–p06 are not
independent evidence for the empirical model. Export the baseline for the same comparison with
`cargo run --locked -p taboom-humanizer --example trace_export -- /tmp/heuristic.jsonl heuristic`.

These reserved-group scores are reused during development as a regression diagnostic. They are
not an untouched final benchmark; claims about general human-like behavior require a new corpus
and a preregistered evaluation protocol.

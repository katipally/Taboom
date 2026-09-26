# Taboom Eval Lab

Proves the humanizer works. Nothing ships on "looks human".

## Install

```bash
cd lab
pip install -e ".[dev]"
```

For the evdev recorder (Linux only):

```bash
pip install -e ".[dev,recorder]"
```

## Run tests

```bash
cargo run -p taboom-humanizer --example trace_export -- lab/traces.jsonl
pytest
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
controls. This guards that synthetic regression pair; CI does not contain a real-human corpus, so
the threshold is not a benchmark claim for general human-vs-bot detection.

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

## Real-human shape evaluation

The pinned SU event logs and reproducible extraction protocol are documented in
[`data/README.md`](data/README.md). The checked-in fixture contains only pseudonymous participant
groups, rounded endpoint distances, and normalized shape features; source event logs stay outside
the repository.

Regenerate the seeded Rust traces, download the pinned A11y-CUA snapshot, and derive the fixture:

```bash
cargo run -p taboom-humanizer --example trace_export -- lab/traces.jsonl
hf download berkeley-hci/Reduced-A11y-CUA --repo-type dataset \
  --revision 26ee1103f74436cb7051543ba65d0c57902c0eb6 --local-dir /tmp/reduced-a11y-cua
cd lab
python -m taboom_lab.shape_eval \
  --human-root /tmp/reduced-a11y-cua/SU \
  --rust-traces traces.jsonl \
  --write-fixture data/a11y_cua_su_shape_features.json
```

To reproduce the CI diagnostic from the committed feature fixture:

```bash
python -m taboom_lab.shape_eval \
  --human-fixture data/a11y_cua_su_shape_features.json \
  --rust-traces traces.jsonl
```

Rust paths are thinned using only their own timestamps at 200, 250, and 330 ms intervals before
both sides are geometrically normalized. The output reports held-out participant/seed AUC for
each cadence. CI fails when any cadence's mean AUC exceeds `--max-mean-auc 0.97`. That is a
ratchet, not a pass mark: today's humanizer sits near 0.95 (0.5 would mean indistinguishable), so
the ceiling only catches regressions. Lower it whenever the humanizer improves. These task-wide
mouse-move runs do not establish click-to-target equivalence.

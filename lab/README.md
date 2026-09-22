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
pytest
```

## Modules

- `recorder.py` -- evdev event capture and task-page generator
- `datasets.py` -- loaders for Balabit, SapiMouse, BOUN, Aalto public datasets
- `features.py` -- feature extraction from mouse/keyboard/scroll traces
- `detector.py` -- RandomForest human-vs-bot classifier
- `fitting.py` -- population distribution fitting, parameter export
- `regression.py` -- one test per known bot failure mode
- `scoreboard.py` -- track scores over time
- `visualize.py` -- matplotlib plots for traces and distributions

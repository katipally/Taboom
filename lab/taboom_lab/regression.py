"""Regression checks against traces exported by the Rust humanizer.

The exporter and live ``VINPUT_TRACE`` share the same JSONL event fields. Exported traces include
seed/action metadata so this loader can recover individual samples. A live file without those
optional fields is accepted too and is grouped by its event type.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path
from typing import TypeAlias

import numpy as np

from .datasets import KeystrokeTrace, MouseTrace
from .features import ScrollTrace, extract_mouse_features, extract_scroll_features


@dataclass
class ButtonTrace:
    """Sequence of (timestamp_us, button index, pressed) events."""

    events: list[tuple[int, int, bool]]
    positions: list[tuple[int, float, float]]


TraceData: TypeAlias = MouseTrace | KeystrokeTrace | ScrollTrace | ButtonTrace


@dataclass
class LoadedTrace:
    trace_id: str
    action: str
    seed: int | None
    data: TraceData


def load_rust_traces(path: str | Path | None = None) -> list[LoadedTrace]:
    """Read Rust exporter or live vinput JSONL and return typed traces.

    The default path is ``lab/traces.jsonl``. ``TABOOM_RUST_TRACES`` can point tests at another
    Rust or live trace file. Missing and malformed files fail explicitly so CI cannot silently
    fall back to generated Python human traces.

    Tagged exporter records preserve each seeded action. Live ``VINPUT_TRACE`` records have no
    action-boundary metadata: their ``pos`` events are grouped into one stream-level ``move``
    trace, including pointer drift that may have happened during idle or around a click. The live
    stream cannot recover individual human actions; use the seeded exporter for per-action data.
    """
    if path is None:
        configured = os.environ.get("TABOOM_RUST_TRACES")
        path = Path(configured) if configured else Path(__file__).resolve().parents[1] / "traces.jsonl"
    return _load_rust_traces(str(Path(path).expanduser().resolve()))


@lru_cache(maxsize=4)
def _load_rust_traces(path: str) -> list[LoadedTrace]:
    source = Path(path)
    if not source.is_file():
        raise FileNotFoundError(f"Rust humanizer traces not found at {source}; run the trace_export example first")

    grouped: dict[tuple[str, str], LoadedTrace] = {}
    with source.open(encoding="utf-8") as stream:
        for line_number, line in enumerate(stream, start=1):
            if not line.strip():
                continue
            if line.lstrip().startswith("{"):
                try:
                    record = json.loads(line)
                except json.JSONDecodeError as error:
                    raise ValueError(f"invalid JSONL record at {source}:{line_number}: {error.msg}") from error
            else:
                # Read pre-Phase-5 vinput records (`ms event a b`) so an existing trace file can
                # remain usable after the daemon starts appending the new JSONL representation.
                fields = line.split()
                if len(fields) != 4:
                    raise ValueError(f"invalid legacy trace record at {source}:{line_number}")
                try:
                    record = {
                        "timestamp_us": int(float(fields[0]) * 1000.0),
                        "event": fields[1],
                        "a": int(fields[2]),
                        "b": int(fields[3]),
                    }
                except ValueError as error:
                    raise ValueError(f"invalid legacy trace fields at {source}:{line_number}") from error
            if not isinstance(record, dict):
                raise ValueError(f"expected an event object at {source}:{line_number}")

            try:
                timestamp_us = int(record["timestamp_us"])
                event = str(record["event"])
                a = int(record.get("a", 0))
                b = int(record.get("b", 0))
            except (KeyError, TypeError, ValueError) as error:
                raise ValueError(f"invalid input event fields at {source}:{line_number}") from error
            if timestamp_us < 0:
                raise ValueError(f"negative timestamp at {source}:{line_number}")

            action = record.get("action")
            if not isinstance(action, str):
                action = {"pos": "move", "btn": "click", "key": "type", "wheel": "scroll"}.get(event)
            if action not in {"move", "click", "type", "scroll", "idle"}:
                raise ValueError(f"unknown action {action!r} at {source}:{line_number}")
            trace_id = record.get("trace_id")
            if not isinstance(trace_id, str) or not trace_id:
                trace_id = f"live:{action}"
            seed_value = record.get("seed")
            seed = int(seed_value) if isinstance(seed_value, int) and not isinstance(seed_value, bool) else None

            key = (trace_id, action)
            trace = grouped.get(key)
            if trace is None:
                data: TraceData
                if action in {"move", "idle"}:
                    data = MouseTrace()
                elif action == "click":
                    data = ButtonTrace(events=[], positions=[])
                elif action == "type":
                    data = KeystrokeTrace()
                else:
                    data = ScrollTrace(events=[])
                trace = LoadedTrace(trace_id=trace_id, action=action, seed=seed, data=data)
                grouped[key] = trace
            elif trace.seed is not None and seed is not None and trace.seed != seed:
                raise ValueError(f"trace {trace_id!r} contains more than one seed")

            if event == "pos" and isinstance(trace.data, MouseTrace):
                trace.data.points.append((timestamp_us, float(a), float(b)))
            elif event == "pos" and isinstance(trace.data, ButtonTrace):
                trace.data.positions.append((timestamp_us, float(a), float(b)))
            elif event == "btn" and isinstance(trace.data, ButtonTrace):
                trace.data.events.append((timestamp_us, a, bool(b)))
            elif event == "key" and isinstance(trace.data, KeystrokeTrace):
                trace.data.events.append((timestamp_us, a, bool(b)))
            elif event == "wheel" and isinstance(trace.data, ScrollTrace):
                if a:
                    trace.data.events.append((timestamp_us, a))
                if b:
                    trace.data.events.append((timestamp_us, b))
            else:
                raise ValueError(f"event {event!r} does not belong to action {action!r} at {source}:{line_number}")

    traces = list(grouped.values())
    if not traces:
        raise ValueError(f"no input events found in {source}")
    return traces


def _action(action: str) -> list[LoadedTrace]:
    return [trace for trace in load_rust_traces() if trace.action == action]


def _mouse(action: str = "move") -> list[MouseTrace]:
    return [trace.data for trace in _action(action) if isinstance(trace.data, MouseTrace)]


def test_rust_export_has_all_action_types() -> None:
    traces = load_rust_traces()
    actions = {trace.action for trace in traces}
    assert {"move", "click", "type", "scroll", "idle"} <= actions
    assert len(_action("move")) >= 30


def test_velocity_peak_not_at_start() -> None:
    for trace in _mouse():
        if len(trace.points) < 3 or np.hypot(
            trace.points[-1][1] - trace.points[0][1], trace.points[-1][2] - trace.points[0][2]
        ) < 150.0:
            continue
        feat = extract_mouse_features(trace)
        pos = feat["velocity_peak_position"]
        assert 0.15 <= pos <= 0.85, f"velocity peak at {pos:.2f}, expected 0.15-0.85"


def test_fitts_law_holds() -> None:
    groups: dict[int, list[tuple[float, float]]] = {}
    for loaded in _action("move"):
        assert isinstance(loaded.data, MouseTrace)
        if loaded.seed is None or len(loaded.data.points) < 3:
            continue
        points = loaded.data.points
        distance = float(np.hypot(points[-1][1] - points[0][1], points[-1][2] - points[0][2]))
        groups.setdefault(loaded.seed, []).append((np.log2(distance / 20.0 + 1.0), extract_mouse_features(loaded.data)["duration"]))

    correlations = []
    for samples in groups.values():
        if len(samples) < 5:
            continue
        ids, durations = np.array(samples).T
        correlations.append(float(np.corrcoef(ids, durations)[0, 1]))
    assert correlations and float(np.nanmean(correlations)) > 0.7, "movement time did not follow Fitts' law"


def test_no_constant_dt() -> None:
    for trace in _mouse():
        feat = extract_mouse_features(trace)
        assert feat["dt_variance"] > 0, "dt variance is zero (constant intervals)"


def test_no_zero_delta_moves() -> None:
    for trace in _mouse():
        for first, second in zip(trace.points, trace.points[1:]):
            assert (second[1], second[2]) != (first[1], first[2]), "zero-delta move in Rust trace"


def test_no_single_axis_noise() -> None:
    eligible = []
    with_perpendicular_noise = 0
    for trace in _mouse():
        if len(trace.points) < 4:
            continue
        distance = np.hypot(
            trace.points[-1][1] - trace.points[0][1], trace.points[-1][2] - trace.points[0][2]
        )
        if distance < 150.0:
            continue
        eligible.append(trace)
        y_values = [point[2] for point in trace.points]
        with_perpendicular_noise += max(y_values) - min(y_values) > 0.0

    # Integer pixel reports can quantize small perpendicular offsets to zero for one sample.
    # Check the deterministic corpus as a whole rather than making every trace independently
    # sensitive to that rounding.
    assert eligible, "no long Rust movement traces available for perpendicular-noise check"
    ratio = with_perpendicular_noise / len(eligible)
    assert ratio >= 0.95, f"only {ratio:.1%} of long movement traces varied off-axis"


def test_no_loopy_paths() -> None:
    for trace in _mouse():
        feat = extract_mouse_features(trace)
        assert feat["straightness"] > 0.78, f"straightness {feat['straightness']:.3f}"


def test_overshoot_then_correct() -> None:
    overshot = 0
    for trace in _mouse():
        if len(trace.points) < 3:
            continue
        start, final = trace.points[0], trace.points[-1]
        dx, dy = final[1] - start[1], final[2] - start[2]
        distance = float(np.hypot(dx, dy))
        if distance == 0.0:
            continue
        ux, uy = dx / distance, dy / distance
        furthest = max((point[1] - start[1]) * ux + (point[2] - start[2]) * uy for point in trace.points)
        if furthest > distance + 2.0:
            overshot += 1
    assert overshot > 0, "no overshoot observed in the seeded Rust movement set"


def test_one_continuous_motion() -> None:
    for trace in _mouse():
        # vinput reports integer pixel positions only after a nonzero relative move. The first
        # report also follows the plan's subpixel warm-up, so inspect later observed steps.
        for first, second in zip(trace.points[1:], trace.points[2:]):
            assert second[0] - first[0] < 500_000, f"stalled {second[0] - first[0]} us"
            assert np.hypot(second[1] - first[1], second[2] - first[2]) < 60.0, "teleport-sized step"


def test_moves_vary() -> None:
    durations = {round(trace.points[-1][0] / 1000) for trace in _mouse() if trace.points}
    assert len(durations) > 10, f"durations barely vary: {sorted(durations)}"


def test_idle_hand_rests_and_stays_near() -> None:
    traces = _mouse("idle")
    assert traces, "Rust idle traces are missing"
    for trace in traces:
        assert len(trace) > 20, "idle hand never moved"
        anchor = trace.points[0][1:]
        # Integer vinput coordinates can add up to sqrt(2) px beyond the fractional radius.
        assert all(np.hypot(point[1] - anchor[0], point[2] - anchor[1]) <= 42.0 for point in trace.points)
        gaps = [second[0] - first[0] for first, second in zip(trace.points, trace.points[1:])]
        assert max(gaps) > 300_000, "idle trace never rested"


def test_click_hold_nonzero() -> None:
    traces = [trace for trace in _action("click") if isinstance(trace.data, ButtonTrace)]
    assert traces, "Rust click traces are missing"
    for trace in traces:
        presses: dict[int, list[int]] = {}
        for timestamp_us, button, pressed in trace.data.events:
            if pressed:
                presses.setdefault(button, []).append(timestamp_us)
            else:
                assert presses.get(button), "click released without a press"
                assert timestamp_us > presses[button].pop(0), "click hold was not positive"


def test_no_fixed_start() -> None:
    starts = {(trace.points[0][1], trace.points[0][2]) for trace in _mouse() if trace.points}
    assert len(starts) > 5, "all Rust movement traces start at the same point"


def test_typing_bigram_variation() -> None:
    common_times: list[int] = []
    rare_times: list[int] = []
    for loaded in _action("type"):
        assert isinstance(loaded.data, KeystrokeTrace)
        presses = [(t, key) for t, key, down in loaded.data.events if down]
        # The sample text begins with "the" and ends with "zxq". These are adjacent physical
        # keys in the exported sequence; compare their press spans across seeded styles.
        first = [t for t, key in presses if key in {20, 35, 18}][:3]
        last = [t for t, key in presses if key in {44, 45, 16}][-3:]
        if len(first) == 3:
            common_times.append(first[-1] - first[0])
        if len(last) == 3:
            rare_times.append(last[-1] - last[0])
    assert common_times and rare_times
    assert np.mean(common_times) < np.mean(rare_times), "common bigrams were not typed faster than rare ones"


def test_typing_rhythm_varies_and_rolls_over() -> None:
    traces = [trace for trace in _action("type") if isinstance(trace.data, KeystrokeTrace)]
    spans = [trace.data.events[-1][0] - trace.data.events[0][0] for trace in traces if trace.data.events]
    assert spans and max(spans) > min(spans) * 1.15, f"same speed every time: {spans}"
    held: set[int] = set()
    peak = 0
    for _, key, down in traces[0].data.events:
        if down:
            held.add(key)
        else:
            held.discard(key)
        peak = max(peak, len(held))
    assert peak >= 2, "no key rollover in the Rust typing trace"


def test_scroll_has_bursts() -> None:
    traces = [trace.data for trace in _action("scroll") if isinstance(trace.data, ScrollTrace)]
    assert traces, "Rust scroll traces are missing"
    for trace in traces:
        feat = extract_scroll_features(trace)
        assert feat["burst_count"] >= 1, "scroll events did not form bursts"


def _generate_bot_trace(
    rng: np.random.Generator,
    from_pt: tuple[float, float] = (100.0, 100.0),
    to_pt: tuple[float, float] = (500.0, 400.0),
    steps: int = 50,
) -> MouseTrace:
    """Negative control: a naive bot follows a linear path with a constant event interval."""
    trace = MouseTrace()
    for i in range(steps + 1):
        frac = i / steps
        x = from_pt[0] + (to_pt[0] - from_pt[0]) * frac
        y = from_pt[1] + (to_pt[1] - from_pt[1]) * frac
        trace.points.append((i * 8_000, x, y))
    return trace

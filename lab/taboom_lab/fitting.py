"""Population distribution fitting for humanizer parameters."""

from __future__ import annotations

import json
from dataclasses import dataclass, field, asdict
from pathlib import Path

import numpy as np
from numpy.typing import NDArray
from scipy import optimize, stats


def fit_lognormal(samples: NDArray[np.float64] | list[float]) -> tuple[float, float]:
    """Fit a lognormal distribution to positive samples. Returns (mu, sigma) of the underlying normal."""
    arr = np.asarray(samples, dtype=np.float64)
    arr = arr[arr > 0]
    if len(arr) < 3:
        raise ValueError("Need at least 3 positive samples")
    log_arr = np.log(arr)
    mu = float(np.mean(log_arr))
    sigma = float(np.std(log_arr, ddof=1))
    return mu, sigma


def fit_fitts_params(
    distances: NDArray[np.float64] | list[float],
    widths: NDArray[np.float64] | list[float],
    times_ms: NDArray[np.float64] | list[float],
) -> tuple[float, float, float]:
    """Fit Fitts' law: MT = a + b * log2(D/W + 1).

    Returns (a, b, r_squared).
    """
    d = np.asarray(distances, dtype=np.float64)
    w = np.asarray(widths, dtype=np.float64)
    mt = np.asarray(times_ms, dtype=np.float64)

    idx_of_difficulty = np.log2(d / w + 1.0)

    A = np.vstack([np.ones_like(idx_of_difficulty), idx_of_difficulty]).T
    result, residuals, _, _ = np.linalg.lstsq(A, mt, rcond=None)
    a, b = float(result[0]), float(result[1])

    predicted = a + b * idx_of_difficulty
    ss_res = float(np.sum((mt - predicted) ** 2))
    ss_tot = float(np.sum((mt - np.mean(mt)) ** 2))
    r_squared = 1.0 - ss_res / ss_tot if ss_tot > 0 else 0.0

    return a, b, r_squared


@dataclass
class TypingParams:
    """Per-bigram flight times, hold times, rollover probability."""
    bigram_flight_ms: dict[str, tuple[float, float]] = field(default_factory=dict)
    hold_mean_ms: float = 70.0
    hold_std_ms: float = 15.0
    rollover_probability: float = 0.2


def fit_typing_params(keystroke_data: list[tuple[int, int, bool]]) -> TypingParams:
    """Fit typing parameters from raw keystroke events.

    Input: list of (timestamp_us, key_code, is_press)
    """
    params = TypingParams()
    active: dict[int, int] = {}
    strokes: list[tuple[int, int, int]] = []
    for timestamp, key, pressed in sorted(keystroke_data, key=lambda event: event[0]):
        if pressed:
            # Ignore key-repeat events while the same physical key is held.
            active.setdefault(key, timestamp)
        elif key in active:
            start = active.pop(key)
            if 0 < timestamp - start < 500_000:
                strokes.append((start, timestamp, key))
    strokes.sort()
    if strokes:
        holds = np.asarray([(release - press) / 1000 for press, release, _ in strokes])
        params.hold_mean_ms = float(np.mean(holds))
        params.hold_std_ms = float(np.std(holds))
    flights_by_bigram: dict[str, list[float]] = {}
    overlaps = total = 0
    for previous, current in zip(strokes, strokes[1:]):
        flight = (current[0] - previous[1]) / 1000
        # Flight is release-to-next-press, and can be negative for rollover.
        if -500 < flight < 2000:
            flights_by_bigram.setdefault(f"{previous[2]}_{current[2]}", []).append(flight)
            overlaps += flight < 0
            total += 1
    for bigram, flights in flights_by_bigram.items():
        params.bigram_flight_ms[bigram] = (float(np.mean(flights)), float(np.std(flights)))
    if total:
        params.rollover_probability = overlaps / total
    return params


def export_params(params: TypingParams | dict, output_path: str | Path) -> None:
    """Export fitted typing statistics as JSON for offline analysis; this schema is not a runtime model."""
    if isinstance(params, TypingParams):
        data = asdict(params)
    else:
        data = params
    with open(output_path, "w") as f:
        json.dump(data, f, indent=2)

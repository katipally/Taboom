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

    presses: dict[int, int] = {}
    releases: list[tuple[int, int]] = []
    hold_times: list[float] = []

    for t_us, key, is_press in keystroke_data:
        if is_press:
            presses[key] = t_us
        else:
            if key in presses:
                hold_ms = (t_us - presses[key]) / 1000.0
                if 0 < hold_ms < 500:
                    hold_times.append(hold_ms)
                releases.append((t_us, key))
                del presses[key]

    if hold_times:
        params.hold_mean_ms = float(np.mean(hold_times))
        params.hold_std_ms = float(np.std(hold_times))

    releases.sort()
    flight_by_bigram: dict[str, list[float]] = {}
    for i in range(1, len(releases)):
        t_prev, k_prev = releases[i - 1]
        t_curr, k_curr = releases[i]
        flight_ms = (t_curr - t_prev) / 1000.0
        if 0 < flight_ms < 2000:
            bigram = f"{k_prev}_{k_curr}"
            flight_by_bigram.setdefault(bigram, []).append(flight_ms)

    for bigram, flights in flight_by_bigram.items():
        arr = np.array(flights)
        params.bigram_flight_ms[bigram] = (float(np.mean(arr)), float(np.std(arr)))

    overlaps = 0
    total = 0
    press_list = sorted(
        [(t, k) for t, k, p in keystroke_data if p], key=lambda x: x[0]
    )
    release_map: dict[int, int] = {}
    for t, k, p in keystroke_data:
        if not p and k in presses:
            pass
        if not p:
            release_map[k] = t

    for i in range(1, len(press_list)):
        t_press, k = press_list[i]
        _, k_prev = press_list[i - 1]
        if k_prev in release_map and release_map[k_prev] > t_press:
            overlaps += 1
        total += 1

    params.rollover_probability = overlaps / total if total > 0 else 0.2

    return params


def export_params(params: TypingParams | dict, output_path: str | Path) -> None:
    """Export fitted parameters as JSON for the Rust humanizer to load."""
    if isinstance(params, TypingParams):
        data = asdict(params)
    else:
        data = params
    with open(output_path, "w") as f:
        json.dump(data, f, indent=2)
